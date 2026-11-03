//! Test harness for the D-Bus side: a private `dbus-daemon` per test, so
//! nothing touches the session bus, the desktop's real Do Not Disturb or
//! its media players. opendeck-power-profile's harness
//! (`src/test_support.rs` there), trimmed. The fake services live next to
//! the code they stand in for.

use std::fs::{OpenOptions, Permissions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use tempfile::TempDir;

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
