use super::backend::{AudioBackend, BackendError, Mute, Node};
use super::model::{DeviceKind, Snapshot};
use async_trait::async_trait;
use std::sync::Mutex;

/// Records every call as a readable string; `snapshot` returns a fixed value.
#[derive(Default)]
pub struct FakeBackend {
    pub calls: Mutex<Vec<String>>,
    pub snapshot: Snapshot,
}

impl FakeBackend {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            snapshot,
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl AudioBackend for FakeBackend {
    async fn snapshot(&self) -> Result<Snapshot, BackendError> {
        Ok(self.snapshot.clone())
    }
    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError> {
        self.record(format!("volume {node:?} {percent}"));
        Ok(())
    }
    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError> {
        self.record(format!("mute {node:?} {mute:?}"));
        Ok(())
    }
    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError> {
        self.record(format!("default {kind:?} {name}"));
        Ok(())
    }
    async fn move_stream(&self, stream: u32, sink: &str) -> Result<(), BackendError> {
        self.record(format!("move {stream} {sink}"));
        Ok(())
    }
}
