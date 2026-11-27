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

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    /// KWin in memory: records every script it is given, in order.
    pub struct FakeKwin {
        supported: bool,
        scripts: Mutex<Vec<String>>,
        /// Every run fails with this message.
        pub fail: Mutex<Option<String>>,
        /// Hangs every run, after recording it, until released (a KWin
        /// that takes its time).
        pub hang: tokio::sync::Mutex<()>,
    }

    impl Default for FakeKwin {
        fn default() -> Self {
            Self::with_support(true)
        }
    }

    impl FakeKwin {
        /// `false`: a platform without KWin.
        pub fn with_support(supported: bool) -> Self {
            Self {
                supported,
                scripts: Mutex::default(),
                fail: Mutex::new(None),
                hang: tokio::sync::Mutex::new(()),
            }
        }

        pub fn scripts(&self) -> Vec<String> {
            self.scripts.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl KwinPort for FakeKwin {
        fn supported(&self) -> bool {
            self.supported
        }
        async fn run_script(&self, source: &str) -> Result<(), String> {
            self.scripts.lock().unwrap().push(source.to_string());
            drop(self.hang.lock().await);
            match self.fail.lock().unwrap().clone() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }
}
