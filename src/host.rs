//! Events sent to the OpenDeck host itself. Stock OpenDeck drops
//! `switchProfile`/`deviceBrightness` sent over a plugin's websocket unless
//! the plugin is the Starter Pack (src-tauri/src/events/inbound/mod.rs), but
//! `opendeck --process-message <json>` hands the event to the running
//! instance without that check. Callers still confirm the effect by
//! re-reading OpenDeck's state and warn when it never arrives.

use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::Instant;

pub const CONFIRM_TIMEOUT: Duration = Duration::from_millis(1500);
const CONFIRM_POLL: Duration = Duration::from_millis(100);

static WARNED: AtomicBool = AtomicBool::new(false);

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

/// Delivers `event` through OpenDeck's command line; if that can't run,
/// falls back to the plugin websocket (honoured only by patched hosts).
pub async fn send(event: Value) -> openaction::OpenActionResult<()> {
    let parent =
        std::fs::read_link(format!("/proc/{}/exe", std::os::unix::process::parent_id())).ok();
    let program = opendeck_program(parent);
    let status = tokio::process::Command::new(&program)
        .args(process_message_args(&event))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    match status {
        Ok(s) if s.success() => Ok(()),
        other => {
            log::warn!(
                "{} --process-message failed ({other:?}); sending over the websocket instead",
                program.display()
            );
            openaction::send_arbitrary_json(event).await
        }
    }
}

/// OpenDeck starts the plugin, so its parent is normally the OpenDeck binary.
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

/// Polls `read` until it returns `expected` or `timeout` passes.
pub async fn wait_for<T: PartialEq>(
    expected: &T,
    mut read: impl FnMut() -> Option<T>,
    timeout: Duration,
) -> bool {
    wait_until(|| read().as_ref() == Some(expected), timeout).await
}

/// Polls `done` until it returns true or `timeout` passes.
pub async fn wait_until(mut done: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(CONFIRM_POLL).await;
    }
}

pub fn warn_ignored_once() {
    if !WARNED.swap(true, Ordering::SeqCst) {
        log::warn!(
            "OpenDeck ignored a brightness/profile request: check that `opendeck --process-message` \
             works on this install (see README)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

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

    #[tokio::test(start_paused = true)]
    async fn wait_for_sees_a_change_before_the_timeout() {
        let calls = Cell::new(0);
        let ok = wait_for(
            &7,
            || {
                calls.set(calls.get() + 1);
                (calls.get() >= 3).then_some(7)
            },
            CONFIRM_TIMEOUT,
        )
        .await;
        assert!(ok);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_until_sees_a_change_before_the_timeout() {
        let calls = Cell::new(0);
        let ok = wait_until(
            || {
                calls.set(calls.get() + 1);
                calls.get() >= 3
            },
            CONFIRM_TIMEOUT,
        )
        .await;
        assert!(ok);
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_until_gives_up_after_the_timeout() {
        let start = Instant::now();
        assert!(!wait_until(|| false, CONFIRM_TIMEOUT).await);
        assert!(start.elapsed() >= CONFIRM_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_gives_up_after_the_timeout() {
        let start = Instant::now();
        assert!(!wait_for(&7, || Some(1), CONFIRM_TIMEOUT).await);
        assert!(start.elapsed() >= CONFIRM_TIMEOUT);
    }
}
