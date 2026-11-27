//! KWin scripting, behind a port so the Window Layout action runs under
//! test with a fake. Linux runs a script inside the compositor through
//! `org.kde.KWin` `/Scripting` on the session bus (`scripting.rs`); other
//! platforms have no KWin and get `Unsupported`. The script itself is
//! `layout.js`, rendered by `crate::layout::script`.

#[cfg(all(test, target_os = "linux"))]
mod dbus_tests;
#[cfg(all(test, target_os = "linux"))]
mod fake_kwin;
#[cfg(target_os = "linux")]
pub mod scripting;

use async_trait::async_trait;

#[async_trait]
pub trait KwinPort: Send + Sync {
    /// This platform can run KWin scripts at all; `false` means the key
    /// says "Linux only" (`crate::layout::view`). KWin missing from a Linux
    /// desktop is not known until a run fails.
    fn supported(&self) -> bool;
    /// Loads `source` into KWin, runs it to completion and unloads it.
    /// `Err` says which step failed.
    async fn run_script(&self, source: &str) -> Result<(), String>;
}

/// No KWin on this platform: the key says "Linux only".
#[cfg(not(target_os = "linux"))]
pub struct Unsupported;

#[cfg(not(target_os = "linux"))]
#[async_trait]
impl KwinPort for Unsupported {
    fn supported(&self) -> bool {
        false
    }
    async fn run_script(&self, _source: &str) -> Result<(), String> {
        Err("KWin scripting is Linux only".into())
    }
}
