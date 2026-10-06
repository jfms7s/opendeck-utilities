//! Read-only view of OpenDeck's own config files: current brightness,
//! the profiles per device, and which one is active. Never written - these
//! files belong to OpenDeck.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::{Notify, watch};

/// Where OpenDeck keeps its config, relative to `$HOME`, in search order.
const CANDIDATES_LINUX: [&str; 2] = [
    ".config/opendeck",
    ".var/app/me.amankhanna.opendeck/config/opendeck",
];
/// Tauri's app config dir for the identifier `opendeck`.
const CANDIDATES_MACOS: [&str; 1] = ["Library/Application Support/opendeck"];

fn candidates(macos: bool) -> &'static [&'static str] {
    if macos {
        &CANDIDATES_MACOS
    } else {
        &CANDIDATES_LINUX
    }
}
/// With inotify the watcher only wakes when OpenDeck writes; this slow
/// re-check is a safety net for missed events.
const SAFETY_POLL: Duration = Duration::from_secs(30);
/// Without inotify (unsupported filesystem, watch limit reached) it polls.
const FALLBACK_POLL: Duration = Duration::from_secs(1);
/// OpenDeck's writes come as a few events; settle before re-reading.
const SETTLE: Duration = Duration::from_millis(30);

pub fn find_config_dir(home: &Path) -> Option<PathBuf> {
    find_config_dir_in(home, candidates(cfg!(target_os = "macos")))
}

fn find_config_dir_in(home: &Path, candidates: &[&str]) -> Option<PathBuf> {
    candidates.iter().map(|p| home.join(p)).find(|p| p.is_dir())
}

/// Device ids come from settings (user-editable) and are joined into
/// paths, so anything that could leave `profiles/` is refused.
pub fn is_safe_device_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', '\0'])
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn no_config_dir(macos: bool) -> &'static str {
    if macos {
        "OpenDeck config directory not found (~/Library/Application Support/opendeck)"
    } else {
        "OpenDeck config directory not found (~/.config/opendeck or the Flatpak path)"
    }
}

#[derive(Debug, Clone)]
pub struct OpenDeckState {
    dir: Option<PathBuf>,
}

impl OpenDeckState {
    pub fn discover() -> Self {
        let dir = std::env::var_os("HOME").and_then(|h| find_config_dir(Path::new(&h)));
        if dir.is_none() {
            log::warn!("OpenDeck config directory not found; brightness/profile state unknown");
        }
        Self { dir }
    }

    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Self {
        Self { dir: Some(dir) }
    }

    /// Why the brightness can't be read, for a one-time log line.
    pub fn brightness_unreadable_reason(&self) -> String {
        match &self.dir {
            Some(dir) => format!(
                "can't read OpenDeck's brightness from {}",
                dir.join("settings.json").display()
            ),
            None => no_config_dir(cfg!(target_os = "macos")).to_string(),
        }
    }

    /// Why `device`'s active profile can't be read, for a one-time log line.
    pub fn profile_unreadable_reason(&self, device: &str) -> String {
        match &self.dir {
            Some(dir) => format!(
                "can't read the active profile from {}",
                dir.join("profiles")
                    .join(format!("{device}.json"))
                    .display()
            ),
            None => no_config_dir(cfg!(target_os = "macos")).to_string(),
        }
    }

    pub fn brightness(&self) -> Option<u8> {
        let settings = read_json(&self.dir.as_ref()?.join("settings.json"))?;
        let value = settings.get("brightness")?.as_u64()?;
        u8::try_from(value.min(100)).ok()
    }

    pub fn profiles(&self, device: &str) -> Vec<String> {
        let Some(dir) = self.dir.as_ref().filter(|_| is_safe_device_id(device)) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir.join("profiles").join(device)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
            .collect();
        names.sort();
        names
    }

    pub fn active_profile(&self, device: &str) -> Option<String> {
        if !is_safe_device_id(device) {
            return None;
        }
        let path = self
            .dir
            .as_ref()?
            .join("profiles")
            .join(format!("{device}.json"));
        Some(
            read_json(&path)?
                .get("selected_profile")?
                .as_str()?
                .to_string(),
        )
    }

    /// Modification times of everything the readers above look at; any
    /// difference between two calls means something changed.
    pub fn fingerprint(&self) -> Vec<(PathBuf, Option<SystemTime>)> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let settings = dir.join("settings.json");
        let mut out = vec![(settings.clone(), mtime(&settings))];
        if let Ok(entries) = std::fs::read_dir(dir.join("profiles")) {
            out.extend(entries.filter_map(Result::ok).map(|e| {
                let p = e.path();
                let t = mtime(&p);
                (p, t)
            }));
        }
        out.sort();
        out
    }
}

/// Watches OpenDeck's config files and ticks the returned receiver whenever
/// `fingerprint` changes. Uses inotify, so it costs nothing while OpenDeck
/// writes nothing; falls back to polling every `FALLBACK_POLL`.
pub fn spawn_watcher(state: OpenDeckState) -> watch::Receiver<u64> {
    let (tx, rx) = watch::channel(0u64);
    tokio::spawn(async move {
        let Some(dir) = state.dir.clone() else {
            // Nothing to watch; keep the channel open for its receivers.
            tx.closed().await;
            return;
        };
        let wake = Arc::new(Notify::new());
        let alive = Arc::new(AtomicBool::new(false));
        let watcher = match inotify::Inotify::new() {
            Ok(w) => Some(Arc::new(w)),
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
                log::info!("polling OpenDeck's config every {FALLBACK_POLL:?} ({e})");
                None
            }
            Err(e) => {
                log::warn!("inotify unavailable ({e}); polling OpenDeck's config instead");
                None
            }
        };
        if let Some(w) = &watcher {
            watch_tree(w, &dir);
            alive.store(true, Ordering::SeqCst);
            inotify::spawn_reader(w.clone(), wake.clone(), alive.clone());
        }
        let mut last = state.fingerprint();
        loop {
            let poll = if alive.load(Ordering::SeqCst) {
                SAFETY_POLL
            } else {
                FALLBACK_POLL
            };
            tokio::select! {
                _ = wake.notified() => tokio::time::sleep(SETTLE).await,
                _ = tokio::time::sleep(poll) => {}
            }
            if let Some(w) = &watcher {
                // New device folders appear at runtime.
                watch_tree(w, &dir);
            }
            let now = state.fingerprint();
            if now != last {
                last = now;
                tx.send_modify(|n| *n = n.wrapping_add(1));
            }
        }
    });
    rx
}

/// The config dir, `profiles/` and every device folder in it.
fn watch_tree(w: &inotify::Inotify, dir: &Path) {
    let profiles = dir.join("profiles");
    let mut dirs = vec![dir.to_path_buf(), profiles.clone()];
    if let Ok(entries) = std::fs::read_dir(&profiles) {
        dirs.extend(
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir()),
        );
    }
    for d in dirs {
        // A missing folder is fine: the parent's watch reports its creation.
        let _ = w.add(&d);
    }
}

/// The few inotify calls the watcher needs, straight from libc.
/// No inotify outside Linux: the watcher polls every `FALLBACK_POLL`.
#[cfg(not(target_os = "linux"))]
mod inotify {
    use std::io;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::Notify;

    pub struct Inotify;

    impl Inotify {
        pub fn new() -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "inotify is Linux-only",
            ))
        }

        pub fn add(&self, _path: &Path) -> io::Result<()> {
            Ok(())
        }
    }

    pub fn spawn_reader(_w: Arc<Inotify>, _wake: Arc<Notify>, _alive: Arc<AtomicBool>) {}
}

#[cfg(target_os = "linux")]
mod inotify {
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    const MASK: u32 = libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_MODIFY
        | libc::IN_CLOSE_WRITE
        | libc::IN_MOVED_FROM
        | libc::IN_MOVED_TO
        | libc::IN_ATTRIB;

    pub struct Inotify {
        fd: OwnedFd,
    }

    impl Inotify {
        pub fn new() -> io::Result<Self> {
            // SAFETY: plain syscall; a non-negative result is a new fd we own.
            let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `fd` is a freshly created, valid descriptor.
            Ok(Self {
                fd: unsafe { OwnedFd::from_raw_fd(fd) },
            })
        }

        /// Adds (or refreshes) a watch on `path`.
        pub fn add(&self, path: &Path) -> io::Result<()> {
            let c = CString::new(path.as_os_str().as_bytes())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            // SAFETY: valid fd and NUL-terminated path for the call's duration.
            let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), MASK) };
            if wd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        /// Blocks until at least one event is queued, then discards them;
        /// the watcher only needs to know that something changed.
        fn wait(&self) -> io::Result<()> {
            let mut buf = [0u8; 4096];
            loop {
                // SAFETY: `buf` is valid for `buf.len()` bytes.
                let n =
                    unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n > 0 {
                    return Ok(());
                }
                let e = io::Error::last_os_error();
                if n < 0 && e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(if n == 0 {
                    io::Error::from(io::ErrorKind::UnexpectedEof)
                } else {
                    e
                });
            }
        }
    }

    /// A thread blocked in `read`: zero wakeups while nothing changes.
    pub fn spawn_reader(w: Arc<Inotify>, wake: Arc<Notify>, alive: Arc<AtomicBool>) {
        let spawned = std::thread::Builder::new()
            .name("opendeck-inotify".into())
            .spawn(move || {
                while w.wait().is_ok() {
                    wake.notify_one();
                }
                log::warn!("inotify stopped; polling OpenDeck's config instead");
                alive.store(false, Ordering::SeqCst);
                wake.notify_one();
            });
        if let Err(e) = spawned {
            log::warn!("can't start the inotify thread ({e}); polling instead");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, OpenDeckState) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::write(
            dir.join("settings.json"),
            r#"{"brightness": 50, "language": "en"}"#,
        )
        .unwrap();
        fs::create_dir_all(dir.join("profiles/sd-1")).unwrap();
        for p in ["Default", "gaming", "claude"] {
            fs::write(dir.join(format!("profiles/sd-1/{p}.json")), "{}").unwrap();
        }
        fs::write(dir.join("profiles/sd-1/notes.txt"), "x").unwrap();
        fs::write(
            dir.join("profiles/sd-1.json"),
            r#"{"selected_profile": "gaming"}"#,
        )
        .unwrap();
        (tmp, OpenDeckState::at(dir))
    }

    #[test]
    fn reads_brightness() {
        let (_tmp, s) = fixture();
        assert_eq!(s.brightness(), Some(50));
    }

    #[test]
    fn lists_json_profiles_sorted() {
        let (_tmp, s) = fixture();
        assert_eq!(s.profiles("sd-1"), vec!["Default", "claude", "gaming"]);
    }

    #[test]
    fn reads_the_active_profile() {
        let (_tmp, s) = fixture();
        assert_eq!(s.active_profile("sd-1").as_deref(), Some("gaming"));
    }

    #[test]
    fn missing_or_malformed_files_are_unknown() {
        let (tmp, s) = fixture();
        fs::write(tmp.path().join("settings.json"), "{ not json").unwrap();
        assert_eq!(s.brightness(), None);
        assert!(s.profiles("sd-2").is_empty());
        assert_eq!(s.active_profile("sd-2"), None);
        let none = OpenDeckState { dir: None };
        assert_eq!(none.brightness(), None);
        assert!(none.fingerprint().is_empty());
    }

    #[test]
    fn unsafe_device_ids_read_nothing() {
        let (tmp, s) = fixture();
        // A profile-shaped file one level above profiles/ that a traversal would reach.
        fs::write(
            tmp.path().join("secret.json"),
            r#"{"selected_profile": "leaked"}"#,
        )
        .unwrap();
        for bad in ["..", "../secret", "a/b", "", ".", "sd-1\\x"] {
            assert!(!is_safe_device_id(bad), "{bad}");
            assert!(s.profiles(bad).is_empty(), "{bad}");
            assert_eq!(s.active_profile(bad), None, "{bad}");
        }
        assert_eq!(s.active_profile("../secret"), None);
    }

    #[test]
    fn fingerprint_changes_when_a_file_changes() {
        let (tmp, s) = fixture();
        let before = s.fingerprint();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(
            tmp.path().join("profiles/sd-1.json"),
            r#"{"selected_profile": "Default"}"#,
        )
        .unwrap();
        assert_ne!(before, s.fingerprint());
    }

    #[cfg(target_os = "linux")]
    async fn ticks(rx: &mut watch::Receiver<u64>) -> bool {
        tokio::time::timeout(Duration::from_secs(5), rx.changed())
            .await
            .is_ok()
    }

    /// Well under the 30 s safety poll, so these only pass through inotify.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_watcher_ticks_on_writes_without_polling() {
        let (tmp, s) = fixture();
        let mut rx = spawn_watcher(s);
        tokio::time::sleep(Duration::from_millis(100)).await;
        fs::write(tmp.path().join("settings.json"), r#"{"brightness": 70}"#).unwrap();
        assert!(ticks(&mut rx).await, "settings.json write");
        fs::write(tmp.path().join("profiles/sd-1/new.json"), "{}").unwrap();
        assert!(ticks(&mut rx).await, "profile added");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_watcher_follows_device_folders_created_later() {
        let (tmp, s) = fixture();
        let mut rx = spawn_watcher(s);
        tokio::time::sleep(Duration::from_millis(100)).await;
        fs::create_dir_all(tmp.path().join("profiles/sd-2")).unwrap();
        assert!(ticks(&mut rx).await, "device folder created");
        tokio::time::sleep(Duration::from_millis(100)).await;
        fs::write(tmp.path().join("profiles/sd-2/Default.json"), "{}").unwrap();
        assert!(ticks(&mut rx).await, "profile in the new folder");
    }

    #[test]
    fn unreadable_reasons_name_the_path() {
        let (tmp, s) = fixture();
        assert!(
            s.brightness_unreadable_reason()
                .contains(&tmp.path().join("settings.json").display().to_string())
        );
        assert!(s.profile_unreadable_reason("sd-1").ends_with("sd-1.json"));
        let none = OpenDeckState { dir: None };
        assert!(none.brightness_unreadable_reason().contains("not found"));
    }

    /// `settings.json` and `profiles/<device>.json` are copied from OpenDeck
    /// 2.14.0 (native, Fedora) with the device serial scrubbed; the profile
    /// files themselves are empty placeholders (only their names are read).
    /// A format change upstream shows up as a fixture update.
    #[test]
    fn reads_real_opendeck_2_14_files() {
        let s = OpenDeckState::at(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/opendeck-2.14.0"),
        );
        assert_eq!(s.brightness(), Some(60));
        assert_eq!(s.active_profile("sd-SERIAL").as_deref(), Some("claude"));
        assert_eq!(
            s.profiles("sd-SERIAL"),
            vec!["Default", "claude", "gaming", "media", "programing"]
        );
    }

    #[test]
    fn finds_native_then_flatpak_config() {
        let tmp = tempfile::tempdir().unwrap();
        let linux = candidates(false);
        assert_eq!(find_config_dir_in(tmp.path(), linux), None);
        let flatpak = tmp.path().join(CANDIDATES_LINUX[1]);
        fs::create_dir_all(&flatpak).unwrap();
        assert_eq!(find_config_dir_in(tmp.path(), linux), Some(flatpak));
        let native = tmp.path().join(CANDIDATES_LINUX[0]);
        fs::create_dir_all(&native).unwrap();
        assert_eq!(find_config_dir_in(tmp.path(), linux), Some(native));
    }

    #[test]
    fn macos_looks_in_application_support() {
        assert_eq!(candidates(true), ["Library/Application Support/opendeck"]);
        assert_eq!(candidates(false), CANDIDATES_LINUX);
        assert!(no_config_dir(true).contains("~/Library/Application Support/opendeck"));
        assert!(no_config_dir(false).contains("~/.config/opendeck"));
    }
}
