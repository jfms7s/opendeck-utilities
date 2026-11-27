//! A fake KWin scripting service on a private bus (`crate::test_bus`):
//! `org.kde.kwin.Scripting` on `/Scripting` and `org.kde.kwin.Script` on
//! `/Scripting/Script<id>`, with KWin's camelCase method names.

use crate::test_bus::PrivateBus;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use zbus::Connection;
use zbus::object_server::ObjectServer;

const SERVICE: &str = "org.kde.KWin";

/// What `loadScript` found at the path it was given.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub path: String,
    pub name: String,
    pub source: String,
    /// Permission bits of the file and of the directory holding it.
    pub file_mode: u32,
    pub dir_mode: u32,
}

/// Every call the fake received, in order, plus what it was told to do.
#[derive(Default)]
pub struct KwinState {
    pub calls: Mutex<Vec<String>>,
    pub loaded: Mutex<Vec<Loaded>>,
    /// `loadScript` returns -1 (KWin's answer for a name still loaded).
    pub refuse_load: Mutex<bool>,
    /// `run` fails with a D-Bus error.
    pub fail_run: Mutex<bool>,
    /// Plugin name to script id, for the scripts still loaded.
    scripts: Mutex<HashMap<String, i32>>,
    next: Mutex<i32>,
    /// Keeps the service on the bus for the test's life.
    conn: OnceLock<Connection>,
}

impl KwinState {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn loaded(&self) -> Vec<Loaded> {
        self.loaded.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

/// Permission bits, or 0 if `path` can't be read (no panic in a handler).
fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).map_or(0, |m| m.permissions().mode() & 0o777)
}

struct FakeScripting(Arc<KwinState>);

#[zbus::interface(name = "org.kde.kwin.Scripting")]
impl FakeScripting {
    #[zbus(name = "loadScript")]
    async fn load_script(
        &self,
        path: String,
        plugin_name: String,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> i32 {
        let file = Path::new(&path);
        let source = std::fs::read_to_string(file).unwrap_or_else(|e| format!("<unreadable: {e}>"));
        let loaded = Loaded {
            path: path.clone(),
            name: plugin_name.clone(),
            source,
            file_mode: mode(file),
            dir_mode: mode(file.parent().unwrap()),
        };
        self.0.loaded.lock().unwrap().push(loaded);
        let refused = *self.0.refuse_load.lock().unwrap()
            || self.0.scripts.lock().unwrap().contains_key(&plugin_name);
        let id = if refused {
            -1
        } else {
            let mut next = self.0.next.lock().unwrap();
            *next += 1;
            *next
        };
        self.0.record(format!("loadScript {plugin_name} -> {id}"));
        if id >= 0 {
            self.0.scripts.lock().unwrap().insert(plugin_name, id);
            server
                .at(
                    format!("/Scripting/Script{id}"),
                    FakeScript {
                        id,
                        state: self.0.clone(),
                    },
                )
                .await
                .unwrap();
        }
        id
    }

    #[zbus(name = "unloadScript")]
    async fn unload_script(
        &self,
        plugin_name: String,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> bool {
        self.0.record(format!("unloadScript {plugin_name}"));
        let id = self.0.scripts.lock().unwrap().remove(&plugin_name);
        match id {
            Some(id) => {
                server
                    .remove::<FakeScript, _>(format!("/Scripting/Script{id}"))
                    .await
                    .unwrap();
                true
            }
            None => false,
        }
    }
}

struct FakeScript {
    id: i32,
    state: Arc<KwinState>,
}

#[zbus::interface(name = "org.kde.kwin.Script")]
impl FakeScript {
    #[zbus(name = "run")]
    async fn run(&self) -> zbus::fdo::Result<()> {
        self.state.record(format!("run {}", self.id));
        if *self.state.fail_run.lock().unwrap() {
            return Err(zbus::fdo::Error::Failed("script error".into()));
        }
        Ok(())
    }
}

/// Serves a fake KWin on `bus`; returns its state.
pub async fn serve_kwin(bus: &PrivateBus) -> Arc<KwinState> {
    let state = Arc::new(KwinState::default());
    let conn = zbus::connection::Builder::address(bus.address.as_str())
        .name(SERVICE)
        .serve_at("/Scripting", FakeScripting(state.clone()))
        .build()
        .await
        .unwrap();
    state.conn.set(conn).unwrap();
    state
}
