//! The desktop's notification inhibition (Do Not Disturb), behind a port
//! so the action runs under test with a fake. Linux talks to
//! `org.freedesktop.Notifications` on the session bus (`dbus.rs`); other
//! platforms have no such API and get `Unsupported`.

#[cfg(target_os = "linux")]
pub mod dbus;
#[cfg(all(test, target_os = "linux"))]
mod dbus_tests;
#[cfg(all(test, target_os = "linux"))]
pub mod test_bus;

use crate::dnd::Inhibition;
use async_trait::async_trait;
use tokio::sync::watch;

#[async_trait]
pub trait NotificationsPort: Send + Sync {
    /// The inhibition as the notification server last reported it; changes
    /// whoever caused them.
    fn inhibition(&self) -> watch::Receiver<Inhibition>;
    /// Starts an inhibition held by this process; returns its cookie. It
    /// ends with `uninhibit(cookie)` or when the plugin's bus connection
    /// closes (the plugin exits).
    async fn inhibit(&self) -> Result<u32, String>;
    async fn uninhibit(&self, cookie: u32) -> Result<(), String>;
}

/// No notification inhibition on this platform: the key says "Linux only".
#[cfg(not(target_os = "linux"))]
pub struct Unsupported {
    state: watch::Sender<Inhibition>,
}

#[cfg(not(target_os = "linux"))]
impl Default for Unsupported {
    fn default() -> Self {
        Self {
            state: watch::Sender::new(Inhibition::Unsupported),
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[async_trait]
impl NotificationsPort for Unsupported {
    fn inhibition(&self) -> watch::Receiver<Inhibition> {
        self.state.subscribe()
    }
    async fn inhibit(&self) -> Result<u32, String> {
        Err("notification inhibition is Linux only".into())
    }
    async fn uninhibit(&self, _cookie: u32) -> Result<(), String> {
        Err("notification inhibition is Linux only".into())
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A notification server in memory: `inhibit` turns it on and hands
    /// out cookies 1, 2, …; `uninhibit` of the live cookie turns it off.
    /// `set` is someone else (Plasma's own DND) changing it.
    pub struct FakeNotifications {
        state: watch::Sender<Inhibition>,
        pub calls: Mutex<Vec<String>>,
        pub fail: Mutex<bool>,
        next: Mutex<u32>,
    }

    impl FakeNotifications {
        pub fn new(initial: Inhibition) -> Self {
            Self {
                state: watch::Sender::new(initial),
                calls: Mutex::default(),
                fail: Mutex::new(false),
                next: Mutex::new(1),
            }
        }

        pub fn set(&self, inhibition: Inhibition) {
            self.state.send_replace(inhibition);
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl NotificationsPort for FakeNotifications {
        fn inhibition(&self) -> watch::Receiver<Inhibition> {
            self.state.subscribe()
        }
        async fn inhibit(&self) -> Result<u32, String> {
            self.calls.lock().unwrap().push("inhibit".into());
            if *self.fail.lock().unwrap() {
                return Err("fake failure".into());
            }
            let mut next = self.next.lock().unwrap();
            let cookie = *next;
            *next += 1;
            self.state.send_replace(Inhibition::On);
            Ok(cookie)
        }
        async fn uninhibit(&self, cookie: u32) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("uninhibit {cookie}"));
            if *self.fail.lock().unwrap() {
                return Err("fake failure".into());
            }
            self.state.send_replace(Inhibition::Off);
            Ok(())
        }
    }
}
