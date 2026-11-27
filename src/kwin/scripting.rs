//! `org.kde.KWin` on the session bus: `loadScript` on `/Scripting`, `run`
//! on `/Scripting/Script<id>`, then `unloadScript`. Only this file talks
//! to KWin.
//!
//! KWin loads scripts from files, so each run writes its source to a new
//! owner-only file under `$XDG_RUNTIME_DIR/opendeck-utilities/` and
//! removes it afterwards. Every run has its own file and plugin name:
//! loading a name that is still loaded returns -1. `run` executes the
//! script to completion before it returns (checked live on KWin 6.8.2),
//! so unloading straight after is safe.

use super::KwinPort;
use async_trait::async_trait;
use std::ffi::{OsStr, OsString};
use std::fs::{DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::OnceCell;
use zbus::proxy::CacheProperties;
use zbus::{Connection, proxy};

/// Every call gives up after this (connecting included): a KWin that is
/// alive but not answering must not hold a press forever.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// Under the runtime dir; owner-only.
const DIR_NAME: &str = "opendeck-utilities";

/// Numbers the runs of this process (file and plugin names).
static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

// KWin's method names are camelCase, so each one is named explicitly.
#[proxy(
    interface = "org.kde.kwin.Scripting",
    default_service = "org.kde.KWin",
    default_path = "/Scripting"
)]
trait Scripting {
    #[zbus(name = "loadScript")]
    fn load_script(&self, file_path: &str, plugin_name: &str) -> zbus::Result<i32>;

    #[zbus(name = "unloadScript")]
    fn unload_script(&self, plugin_name: &str) -> zbus::Result<bool>;
}

// The path is per script (`/Scripting/Script<id>`), set on the builder.
#[proxy(interface = "org.kde.kwin.Script", default_service = "org.kde.KWin")]
trait Script {
    #[zbus(name = "run")]
    fn run(&self) -> zbus::Result<()>;
}

pub struct KwinScripting {
    /// `None`: the session bus (`DBUS_SESSION_BUS_ADDRESS`).
    address: Option<String>,
    /// The raw `$XDG_RUNTIME_DIR`, checked on each run (`runtime_dir`).
    runtime_dir: Option<OsString>,
    connection: OnceCell<Connection>,
}

impl KwinScripting {
    /// Connects lazily, on the first run; a failed connect is retried on
    /// the next one.
    pub fn new(address: Option<String>, runtime_dir: Option<OsString>) -> Self {
        Self {
            address,
            runtime_dir,
            connection: OnceCell::new(),
        }
    }

    /// The session bus and `$XDG_RUNTIME_DIR`.
    pub fn session() -> Self {
        Self::new(None, std::env::var_os("XDG_RUNTIME_DIR"))
    }

    async fn connection(&self) -> Result<&Connection, String> {
        self.connection
            .get_or_try_init(|| async {
                let builder = match &self.address {
                    Some(address) => zbus::connection::Builder::address(address.as_str()),
                    None => zbus::connection::Builder::session(),
                };
                match tokio::time::timeout(
                    CALL_TIMEOUT,
                    builder.method_timeout(CALL_TIMEOUT).build(),
                )
                .await
                {
                    Ok(built) => built.map_err(|e| e.to_string()),
                    Err(_) => Err(format!("no answer in {CALL_TIMEOUT:?}")),
                }
            })
            .await
            .map_err(|e| format!("KWin: can't connect to D-Bus: {e}"))
    }
}

/// `$XDG_RUNTIME_DIR`, which must be set and absolute: no fallback to a
/// shared directory such as `/tmp`.
fn runtime_dir(value: Option<&OsStr>) -> Result<PathBuf, String> {
    match value {
        None => Err("KWin: XDG_RUNTIME_DIR is not set".into()),
        Some(value) if !Path::new(value).is_absolute() => Err(format!(
            "KWin: XDG_RUNTIME_DIR is not an absolute path: {value:?}"
        )),
        Some(value) => Ok(PathBuf::from(value)),
    }
}

/// The script file's name and KWin's plugin name for run `n` of process
/// `pid`.
fn run_names(pid: u32, n: u64) -> (String, String) {
    (
        format!("layout-{pid}-{n}.js"),
        format!("opendeck-utilities-layout-{pid}-{n}"),
    )
}

/// `<runtime>/opendeck-utilities`, created 0700. An existing one must be
/// a real directory (not a symlink) that only its owner can use.
fn private_dir(runtime: &Path) -> Result<PathBuf, String> {
    let dir = runtime.join(DIR_NAME);
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("KWin: can't create {}: {e}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(&dir)
        .map_err(|e| format!("KWin: can't read {}: {e}", dir.display()))?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "KWin: {} is not a private directory",
            dir.display()
        ));
    }
    Ok(dir)
}

/// Writes `source` to a new 0600 file `dir/file_name`. `create_new`: an
/// existing file or symlink there is an error, never written through.
fn create_script_file(dir: &Path, file_name: &str, source: &str) -> Result<PathBuf, String> {
    let path = dir.join(file_name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("KWin: can't create {}: {e}", path.display()))?;
    if let Err(e) = file.write_all(source.as_bytes()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(format!("KWin: can't write {}: {e}", path.display()));
    }
    Ok(path)
}

async fn load(scripting: &ScriptingProxy<'_>, path: &Path, name: &str) -> Result<i32, String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("KWin: loadScript: path is not UTF-8: {}", path.display()))?;
    match scripting.load_script(path_str, name).await {
        Ok(id) if id >= 0 => Ok(id),
        Ok(id) => Err(format!("KWin: loadScript refused {name} (id {id})")),
        Err(e) => Err(format!("KWin: loadScript failed: {e}")),
    }
}

async fn run(connection: &Connection, id: i32) -> Result<(), String> {
    let script = ScriptProxy::builder(connection)
        .path(format!("/Scripting/Script{id}"))
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .map_err(|e| format!("KWin: run (script {id}) failed: {e}"))?;
    script
        .run()
        .await
        .map_err(|e| format!("KWin: run (script {id}) failed: {e}"))
}

async fn unload(scripting: &ScriptingProxy<'_>, name: &str) -> Result<(), String> {
    match scripting.unload_script(name).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("KWin: unloadScript found no script {name}")),
        Err(e) => Err(format!("KWin: unloadScript {name} failed: {e}")),
    }
}

#[async_trait]
impl KwinPort for KwinScripting {
    fn supported(&self) -> bool {
        true
    }

    async fn run_script(&self, source: &str) -> Result<(), String> {
        let runtime = runtime_dir(self.runtime_dir.as_deref())?;
        let connection = self.connection().await?;
        let scripting = ScriptingProxy::builder(connection)
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(|e| format!("KWin: can't reach /Scripting: {e}"))?;
        let dir = private_dir(&runtime)?;
        let (file_name, name) =
            run_names(std::process::id(), NEXT_RUN.fetch_add(1, Ordering::SeqCst));
        let path = create_script_file(&dir, &file_name, source)?;

        // Whatever fails, a loaded script is unloaded and the file removed;
        // the first failure leads the message.
        let mut errors = Vec::new();
        match load(&scripting, &path, &name).await {
            Ok(id) => {
                if let Err(e) = run(connection, id).await {
                    errors.push(e);
                }
                if let Err(e) = unload(&scripting, &name).await {
                    errors.push(e);
                }
            }
            Err(e) => errors.push(e),
        }
        if let Err(e) = std::fs::remove_file(&path) {
            errors.push(format!("KWin: can't remove {}: {e}", path.display()));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn names_carry_the_pid_and_the_run_number() {
        assert_eq!(
            run_names(4242, 7),
            (
                "layout-4242-7.js".to_string(),
                "opendeck-utilities-layout-4242-7".to_string()
            )
        );
        assert_ne!(run_names(4242, 7), run_names(4242, 8));
    }

    #[test]
    fn the_runtime_dir_must_be_set_and_absolute() {
        assert_eq!(
            runtime_dir(Some(OsStr::new("/run/user/1000"))),
            Ok(PathBuf::from("/run/user/1000"))
        );
        for bad in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("run/user/1000")),
        ] {
            let err = runtime_dir(bad).unwrap_err();
            assert!(err.contains("XDG_RUNTIME_DIR"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn the_private_dir_is_created_owner_only() {
        let runtime = tempfile::tempdir().unwrap();
        let dir = private_dir(runtime.path()).unwrap();
        assert_eq!(dir, runtime.path().join("opendeck-utilities"));
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        // A second run reuses it.
        assert_eq!(private_dir(runtime.path()).unwrap(), dir);
    }

    #[test]
    fn a_shared_or_symlinked_private_dir_is_refused() {
        let runtime = tempfile::tempdir().unwrap();
        let dir = runtime.path().join("opendeck-utilities");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(runtime.path()).is_err());

        let other = tempfile::tempdir().unwrap();
        std::fs::remove_dir(&dir).unwrap();
        symlink(other.path(), &dir).unwrap();
        assert!(private_dir(runtime.path()).is_err());
    }

    #[test]
    fn the_script_file_is_new_and_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_script_file(dir.path(), "a.js", "source").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "source");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn an_existing_file_or_symlink_is_never_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("a.js");
        std::fs::write(&existing, "theirs").unwrap();
        assert!(create_script_file(dir.path(), "a.js", "ours").is_err());
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "theirs");

        let target = dir.path().join("target");
        std::fs::write(&target, "theirs").unwrap();
        symlink(&target, dir.path().join("b.js")).unwrap();
        assert!(create_script_file(dir.path(), "b.js", "ours").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "theirs");
    }
}
