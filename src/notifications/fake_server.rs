//! A fake notification server on a private bus (`crate::test_bus`).

use crate::test_bus::PrivateBus;
use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use zbus::Connection;
use zbus::names::InterfaceName;
use zbus::object_server::SignalEmitter;
use zbus::{OwnedVariant, Variant};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// What the fake server holds: live cookies, an inhibition from "someone
/// else" (Plasma's own DND), and every call it received.
#[derive(Default)]
pub struct ServerState {
    cookies: Mutex<BTreeSet<u32>>,
    next: Mutex<u32>,
    other: Mutex<bool>,
    pub calls: Mutex<Vec<String>>,
    conn: OnceLock<Connection>,
}

impl ServerState {
    fn inhibited(&self) -> bool {
        !self.cookies.lock().unwrap().is_empty() || *self.other.lock().unwrap()
    }

    async fn announce(&self) {
        let emitter = SignalEmitter::new(self.conn.get().unwrap(), PATH).unwrap();
        zbus::fdo::Properties::properties_changed(
            &emitter,
            InterfaceName::try_from(SERVICE).unwrap(),
            HashMap::from([("Inhibited", Variant::from(self.inhibited()))]),
            Cow::Borrowed(&[]),
        )
        .await
        .unwrap();
    }

    /// Someone else (the desktop's own DND) turns inhibition on or off.
    pub async fn set_other(&self, on: bool) {
        *self.other.lock().unwrap() = on;
        self.announce().await;
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

struct FakeServer(Arc<ServerState>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FakeServer {
    async fn inhibit(
        &self,
        desktop_entry: String,
        reason: String,
        _hints: HashMap<String, OwnedVariant>,
    ) -> u32 {
        let cookie = {
            let mut next = self.0.next.lock().unwrap();
            *next += 1;
            *next
        };
        self.0
            .calls
            .lock()
            .unwrap()
            .push(format!("Inhibit {desktop_entry} {reason:?} -> {cookie}"));
        self.0.cookies.lock().unwrap().insert(cookie);
        self.0.announce().await;
        cookie
    }

    #[zbus(name = "UnInhibit")]
    async fn un_inhibit(&self, cookie: u32) -> zbus::fdo::Result<()> {
        self.0
            .calls
            .lock()
            .unwrap()
            .push(format!("UnInhibit {cookie}"));
        if !self.0.cookies.lock().unwrap().remove(&cookie) {
            return Err(zbus::fdo::Error::InvalidArgs("unknown cookie".into()));
        }
        self.0.announce().await;
        Ok(())
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn inhibited(&self) -> bool {
        self.0.inhibited()
    }
}

/// Serves a fake notification server on `bus`; returns its state.
pub async fn serve_notifications(bus: &PrivateBus) -> Arc<ServerState> {
    let state = Arc::new(ServerState::default());
    let conn = zbus::connection::Builder::address(bus.address.as_str())
        .name(SERVICE)
        .serve_at(PATH, FakeServer(state.clone()))
        .build()
        .await
        .unwrap();
    state.conn.set(conn).unwrap();
    state
}
