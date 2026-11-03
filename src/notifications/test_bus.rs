//! Test harness for the D-Bus side: a private `dbus-daemon` per test and a
//! fake notification server on it. Nothing here touches the session bus or
//! the desktop's real Do Not Disturb. The bus is opendeck-power-profile's
//! harness (`src/test_support.rs` there), trimmed.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::fs::{OpenOptions, Permissions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use tempfile::TempDir;
use zbus::Connection;
use zbus::names::InterfaceName;
use zbus::object_server::SignalEmitter;
use zbus::{OwnedVariant, Variant};

/// A throwaway message bus, killed on drop.
pub struct PrivateBus {
    child: Child,
    /// Kills `child` and removes `dir` if this process dies without dropping
    /// the bus.
    watchdog: Child,
    /// Removed on drop, after the daemon is gone.
    _dir: TempDir,
    pub address: String,
}

impl PrivateBus {
    pub fn start() -> Self {
        // A fresh, owner-only directory with a random name.
        let dir = tempfile::Builder::new()
            .prefix("opendeck-utilities-test-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let config = dir.path().join("bus.conf");
        // `create_new`: an existing file or symlink here is an error.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&config)
            .unwrap();
        write!(
            file,
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
            dir.path().display()
        )
        .unwrap();
        drop(file);
        let mut child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("these tests need `dbus-daemon` on PATH (package: dbus-daemon / dbus)");
        let watchdog = spawn_watchdog(child.id(), dir.path());
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self {
            child,
            watchdog,
            _dir: dir,
            address: address.trim().to_string(),
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        // The watchdog goes first, so it can never fire at a reused pid.
        let _ = self.watchdog.kill();
        let _ = self.watchdog.wait();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A `sh` that kills `pid` and removes `dir` once its stdin reaches EOF,
/// which happens only when this process dies (std's pipes are
/// close-on-exec): a killed test run leaves no daemon behind.
fn spawn_watchdog(pid: u32, dir: &Path) -> Child {
    Command::new("sh")
        .args([
            "-c",
            r#"read _; kill -9 "$1" 2>/dev/null; rm -rf -- "$2""#,
            "private-bus-watchdog",
        ])
        .arg(pid.to_string())
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("these tests need `sh` on PATH")
}

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
