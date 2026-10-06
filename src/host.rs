//! Events sent to the OpenDeck host itself. Stock OpenDeck drops
//! `switchProfile`/`deviceBrightness` sent over a plugin's websocket unless
//! the plugin is the Starter Pack (src-tauri/src/events/inbound/mod.rs), but
//! `opendeck --process-message <json>` hands the event to the running
//! instance without that check. `crate::dispatch` sends through `Host` and
//! confirms the effect by re-reading OpenDeck's state.

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

pub const CONFIRM_TIMEOUT: Duration = Duration::from_millis(1500);
/// Safety net while waiting for a confirmation: the state watcher normally
/// wakes the wait as soon as OpenDeck writes its files.
const CONFIRM_RECHECK: Duration = Duration::from_millis(500);

static WARNED_IGNORED: AtomicBool = AtomicBool::new(false);
static WARNED_ONCE: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
static LOGGED_PROGRAM: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrightnessChange {
    Set,
    Increase,
    Decrease,
}

pub fn brightness_event(change: BrightnessChange, value: u8) -> Value {
    json!({ "event": "deviceBrightness", "action": change, "value": value.min(100) })
}

pub fn switch_profile_event(device: &str, profile: &str) -> Value {
    json!({ "event": "switchProfile", "device": device, "profile": profile })
}

/// Where host events go. `OpenDeckHost` in the plugin; a recording fake in
/// tests.
#[async_trait]
pub trait Host: Send + Sync {
    async fn send(&self, event: Value) -> Result<(), String>;
}

pub struct OpenDeckHost;

#[async_trait]
impl Host for OpenDeckHost {
    async fn send(&self, event: Value) -> Result<(), String> {
        let program = opendeck_program(parent_exe());
        LOGGED_PROGRAM.call_once(|| {
            log::info!("sending host events through {}", program.display());
        });
        send_via(&program, event, openaction::send_arbitrary_json)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Delivers `event` through `program --process-message`; only if that
/// can't run does it fall back to `websocket` (honoured by patched hosts only).
async fn send_via<F, Fut>(
    program: &Path,
    event: Value,
    websocket: F,
) -> openaction::OpenActionResult<()>
where
    F: FnOnce(Value) -> Fut,
    Fut: Future<Output = openaction::OpenActionResult<()>>,
{
    match process_message(program, &event).await {
        Ok(()) => Ok(()),
        Err(why) => {
            log::warn!(
                "{} --process-message failed ({why}); sending over the websocket instead",
                program.display()
            );
            websocket(event).await
        }
    }
}

async fn process_message(program: &Path, event: &Value) -> Result<(), String> {
    let status = tokio::process::Command::new(program)
        .args(process_message_args(event))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(status.to_string())
    }
}

/// The executable of the process that started the plugin (normally OpenDeck).
#[cfg(target_os = "linux")]
fn parent_exe() -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{}/exe", std::os::unix::process::parent_id())).ok()
}

/// macOS has no `/proc`; `proc_pidpath` gives the same answer.
#[cfg(target_os = "macos")]
fn parent_exe() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is valid for `buf.len()` bytes, which is what we pass.
    let len = unsafe {
        libc::proc_pidpath(
            std::os::unix::process::parent_id() as libc::c_int,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
        )
    };
    if len <= 0 {
        return None;
    }
    buf.truncate(len as usize);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn parent_exe() -> Option<PathBuf> {
    None
}

/// OpenDeck starts the plugin, so its parent is normally the OpenDeck binary
/// (`opendeck` on Linux, `OpenDeck.app/Contents/MacOS/opendeck` on macOS).
fn opendeck_program(parent_exe: Option<PathBuf>) -> PathBuf {
    parent_exe
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("opendeck"))
        })
        .unwrap_or_else(|| PathBuf::from("opendeck"))
}

fn process_message_args(event: &Value) -> [String; 2] {
    ["--process-message".to_string(), event.to_string()]
}

/// Waits until `done` returns true or `timeout` passes. It re-checks
/// whenever `changes` ticks (OpenDeck's files changed) and, as a safety
/// net, every `CONFIRM_RECHECK`.
pub async fn wait_until(
    mut done: impl FnMut() -> bool,
    changes: &mut watch::Receiver<u64>,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut watching = true;
    loop {
        if done() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let recheck = (now + CONFIRM_RECHECK).min(deadline);
        tokio::select! {
            changed = changes.changed(), if watching => watching = changed.is_ok(),
            _ = tokio::time::sleep_until(recheck) => {}
        }
    }
}

/// For a host request whose effect never showed up although OpenDeck's state
/// could be read - the one case that points at the sending route.
pub fn warn_ignored_once() {
    if !WARNED_IGNORED.swap(true, Ordering::SeqCst) {
        log::warn!(
            "OpenDeck ignored a brightness/profile request: check that `opendeck --process-message` \
             works on this install (see README)"
        );
    }
}

/// Logs `message` the first time `key` is seen in this run.
pub fn warn_once(key: &'static str, message: impl FnOnce() -> String) {
    let mut seen = WARNED_ONCE.lock().unwrap_or_else(|e| e.into_inner());
    if seen.get_or_insert_with(HashSet::new).insert(key) {
        let message = message();
        log::warn!("{message}");
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Arc;

    type OnSend = Box<dyn Fn(&Value) + Send + Sync>;

    /// Records every event; `on_send` plays OpenDeck's part (e.g. writes
    /// the state file the request should change).
    pub struct FakeHost {
        pub sent: Mutex<Vec<Value>>,
        pub fail: AtomicBool,
        on_send: OnSend,
    }

    impl FakeHost {
        pub fn new(on_send: impl Fn(&Value) + Send + Sync + 'static) -> Arc<Self> {
            Arc::new(Self {
                sent: Mutex::new(Vec::new()),
                fail: AtomicBool::new(false),
                on_send: Box::new(on_send),
            })
        }

        /// OpenDeck receives the event but does nothing.
        pub fn ignoring() -> Arc<Self> {
            Self::new(|_| {})
        }

        pub fn sent(&self) -> Vec<Value> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Host for FakeHost {
        async fn send(&self, event: Value) -> Result<(), String> {
            if self.fail.load(Ordering::SeqCst) {
                return Err("opendeck not runnable".to_string());
            }
            (self.on_send)(&event);
            self.sent.lock().unwrap().push(event);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Arc;

    #[test]
    fn event_shapes_match_opendeck() {
        assert_eq!(
            brightness_event(BrightnessChange::Increase, 5),
            json!({"event": "deviceBrightness", "action": "increase", "value": 5})
        );
        assert_eq!(brightness_event(BrightnessChange::Set, 200)["value"], 100);
        assert_eq!(
            switch_profile_event("sd-1", "gaming"),
            json!({"event": "switchProfile", "device": "sd-1", "profile": "gaming"})
        );
    }

    #[test]
    fn program_is_the_parent_when_it_is_opendeck() {
        assert_eq!(
            opendeck_program(Some(PathBuf::from("/usr/bin/opendeck"))),
            PathBuf::from("/usr/bin/opendeck")
        );
    }

    #[test]
    fn program_falls_back_to_path_lookup() {
        assert_eq!(opendeck_program(None), PathBuf::from("opendeck"));
        assert_eq!(
            opendeck_program(Some(PathBuf::from("/usr/bin/bash"))),
            PathBuf::from("opendeck")
        );
    }

    #[test]
    fn message_is_one_argument() {
        let e = switch_profile_event("sd-1", "my profile");
        let args = process_message_args(&e);
        assert_eq!(args[0], "--process-message");
        assert_eq!(args.len(), 2);
        assert_eq!(serde_json::from_str::<Value>(&args[1]).ok(), Some(e));
    }

    /// Runs `send_via` against `program`, reporting whether the websocket
    /// fallback was used.
    async fn used_websocket(program: &str) -> bool {
        let used = Arc::new(AtomicBool::new(false));
        let flag = used.clone();
        send_via(Path::new(program), json!({"event": "x"}), |_| async move {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        used.load(Ordering::SeqCst)
    }

    /// The e3ec46b route: a command that exits 0 delivered the event, so the
    /// websocket (which stock OpenDeck drops) must not be used.
    #[tokio::test]
    async fn a_successful_command_does_not_touch_the_websocket() {
        assert!(!used_websocket("/usr/bin/true").await);
    }

    /// The test runner has a parent (cargo) with a real executable: proves
    /// the per-platform lookup (`/proc` on Linux, `proc_pidpath` on macOS).
    #[test]
    fn the_parent_executable_is_found() {
        let exe = parent_exe().expect("parent executable");
        assert!(exe.is_absolute() && exe.exists(), "{exe:?}");
    }

    #[tokio::test]
    async fn a_failing_or_missing_command_falls_back_to_the_websocket() {
        assert!(used_websocket("/usr/bin/false").await);
        assert!(used_websocket("/nonexistent/opendeck").await);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_until_wakes_on_a_change_before_the_timeout() {
        let (tx, mut rx) = watch::channel(0u64);
        let calls = Cell::new(0);
        let start = Instant::now();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send_modify(|n| *n += 1);
            // Keep the sender alive past the wait.
            tokio::time::sleep(CONFIRM_TIMEOUT).await;
        });
        let ok = wait_until(
            || {
                calls.set(calls.get() + 1);
                calls.get() >= 2
            },
            &mut rx,
            CONFIRM_TIMEOUT,
        )
        .await;
        assert!(ok);
        assert_eq!(calls.get(), 2, "one check up front, one on the change");
        assert!(start.elapsed() < CONFIRM_RECHECK);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_until_gives_up_after_the_timeout_with_few_reads() {
        let (_tx, mut rx) = watch::channel(0u64);
        let calls = Cell::new(0);
        let start = Instant::now();
        let ok = wait_until(
            || {
                calls.set(calls.get() + 1);
                false
            },
            &mut rx,
            CONFIRM_TIMEOUT,
        )
        .await;
        assert!(!ok);
        assert!(start.elapsed() >= CONFIRM_TIMEOUT);
        assert!(calls.get() <= 5, "{} reads", calls.get());
    }

    #[tokio::test(start_paused = true)]
    async fn wait_until_survives_a_closed_change_channel() {
        let (tx, mut rx) = watch::channel(0u64);
        drop(tx);
        assert!(!wait_until(|| false, &mut rx, CONFIRM_TIMEOUT).await);
    }

    #[test]
    fn warn_once_runs_the_message_once_per_key() {
        let built = Cell::new(0);
        for _ in 0..3 {
            warn_once("host::tests", || {
                built.set(built.get() + 1);
                "x".to_string()
            });
        }
        assert_eq!(built.get(), 1);
    }
}
