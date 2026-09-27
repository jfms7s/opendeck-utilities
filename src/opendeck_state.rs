//! Read-only view of OpenDeck's own config files: current brightness,
//! the profiles per device, and which one is active. Never written - these
//! files belong to OpenDeck.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::sync::watch;

const CANDIDATES: [&str; 2] = [
    ".config/opendeck",
    ".var/app/me.amankhanna.opendeck/config/opendeck",
];
const POLL: Duration = Duration::from_millis(250);

pub fn find_config_dir(home: &Path) -> Option<PathBuf> {
    CANDIDATES.iter().map(|p| home.join(p)).find(|p| p.is_dir())
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

/// Polls `fingerprint` every second and bumps `tx` when it changes.
pub fn spawn_watcher(state: OpenDeckState, tx: watch::Sender<u64>) {
    tokio::spawn(async move {
        let mut last = state.fingerprint();
        let mut tick = tokio::time::interval(POLL);
        loop {
            tick.tick().await;
            let now = state.fingerprint();
            if now != last {
                last = now;
                tx.send_modify(|n| *n = n.wrapping_add(1));
            }
        }
    });
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

    #[test]
    fn finds_native_then_flatpak_config() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(find_config_dir(tmp.path()), None);
        let flatpak = tmp.path().join(CANDIDATES[1]);
        fs::create_dir_all(&flatpak).unwrap();
        assert_eq!(find_config_dir(tmp.path()), Some(flatpak));
        let native = tmp.path().join(CANDIDATES[0]);
        fs::create_dir_all(&native).unwrap();
        assert_eq!(find_config_dir(tmp.path()), Some(native));
    }
}
