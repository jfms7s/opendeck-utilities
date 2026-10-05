use super::backend::{AudioBackend, BackendError, Mute, Node};
use super::model::{DeviceKind, Snapshot};
use async_trait::async_trait;
use std::sync::Mutex;
use tokio::sync::watch;

/// Records every call as a readable string. `snapshot` returns the stored
/// snapshot (or `snapshot_error`), and `fail_on` makes calls fail, so the
/// error paths run under test too.
#[derive(Default)]
pub struct FakeBackend {
    pub calls: Mutex<Vec<String>>,
    pub snapshot: Mutex<Snapshot>,
    /// When set, `snapshot` fails with `BackendError::Failed`.
    pub snapshot_error: Mutex<Option<String>>,
    /// Calls whose recorded string starts with the first element fail, after
    /// skipping the first `.1` of them; `.2` is how many fail (0 = all).
    pub fail_on: Mutex<Option<(String, usize, usize)>>,
    pub subscribed: Mutex<Option<watch::Sender<u64>>>,
    /// How long `snapshot` takes (a slow `pactl`).
    pub snapshot_delay: Mutex<Option<std::time::Duration>>,
}

impl FakeBackend {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot: Mutex::new(snapshot),
            ..Self::default()
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Calls starting with `prefix` fail, after the first `skip` of them.
    pub fn fail(&self, prefix: &str, skip: usize) {
        *self.fail_on.lock().unwrap() = Some((prefix.to_string(), skip, 0));
    }

    /// Like `fail`, but only `times` calls fail; later ones succeed.
    pub fn fail_times(&self, prefix: &str, times: usize) {
        *self.fail_on.lock().unwrap() = Some((prefix.to_string(), 0, times));
    }

    fn record(&self, call: String) -> Result<(), BackendError> {
        let mut fail_on = self.fail_on.lock().unwrap();
        let failing = match fail_on.as_mut() {
            Some((prefix, skip, _)) if call.starts_with(prefix.as_str()) && *skip > 0 => {
                *skip -= 1;
                false
            }
            Some((prefix, _, times)) if call.starts_with(prefix.as_str()) => {
                if *times == 1 {
                    *fail_on = None;
                } else if *times > 1 {
                    *times -= 1;
                }
                true
            }
            _ => false,
        };
        drop(fail_on);
        self.calls.lock().unwrap().push(call.clone());
        if failing {
            Err(BackendError::Failed {
                args: call,
                stderr: "fake failure".into(),
            })
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl AudioBackend for FakeBackend {
    /// What `pactl` reports when the read starts; it returns after
    /// `snapshot_delay`.
    async fn snapshot(&self) -> Result<Snapshot, BackendError> {
        let delay = *self.snapshot_delay.lock().unwrap();
        let error = self.snapshot_error.lock().unwrap().clone();
        let result = match error {
            Some(stderr) => Err(BackendError::Failed {
                args: "snapshot".into(),
                stderr,
            }),
            None => Ok(self.snapshot.lock().unwrap().clone()),
        };
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        result
    }
    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError> {
        self.record(format!("volume {node:?} {percent}"))
    }
    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError> {
        self.record(format!("mute {node:?} {mute:?}"))
    }
    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError> {
        self.record(format!("default {kind:?} {name}"))
    }
    async fn move_stream(&self, stream: u32, sink: &str) -> Result<(), BackendError> {
        self.record(format!("move {stream} {sink}"))
    }
    fn subscribe(&self, tx: watch::Sender<u64>) {
        *self.subscribed.lock().unwrap() = Some(tx);
    }
}
