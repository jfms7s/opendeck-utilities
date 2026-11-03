//! The contract every audio backend implements, and its error type.
//! Backend-neutral: the implementations live in `pactl/` (Linux) and
//! `coreaudio/` (macOS).

use super::model::{DeviceKind, Snapshot};
use super::pactl::parse::ParseError;
use async_trait::async_trait;
use tokio::sync::watch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Sink(String),
    Source(String),
    SinkInput(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mute {
    On,
    Off,
    Toggle,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The backend's tool (named) is missing.
    #[error("{0} not found")]
    NotInstalled(&'static str),
    #[error("pactl {args} failed: {stderr}")]
    Failed { args: String, stderr: String },
    /// The backend's tool (named) didn't answer in time.
    #[error("{0} timed out")]
    Timeout(&'static str),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("nothing to act on")]
    NoTarget,
    #[error("refusing unsafe name {0:?}")]
    UnsafeName(String),
    #[error("io: {0}")]
    Io(std::io::Error),
    /// The platform can't do this (macOS: per-app audio, a device without
    /// a volume or mute control).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error("{0}")]
    Unsupported(String),
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error("CoreAudio: {0}")]
    CoreAudio(String),
}

impl BackendError {
    /// The few words a tile has room for: the missing tool by name (the
    /// one error a user can fix), "audio error" for everything else.
    pub fn short(&self) -> String {
        match self {
            BackendError::NotInstalled(tool) => format!("{tool} not found"),
            _ => "audio error".to_string(),
        }
    }
}

/// The audio system: read it, change it, and hear about changes made
/// anywhere (so a second backend or a test fake comes as one object).
#[async_trait]
pub trait AudioBackend: Send + Sync {
    async fn snapshot(&self) -> Result<Snapshot, BackendError>;
    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError>;
    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError>;
    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError>;
    async fn move_stream(&self, stream: u32, sink: &str) -> Result<(), BackendError>;
    /// Starts reporting changes: bumps `tx` on every relevant change.
    fn subscribe(&self, tx: watch::Sender<u64>);
}
