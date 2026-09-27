//! The only code that runs `pactl`. Commands are spawned directly (never
//! through a shell), and every name argument is checked by `is_safe_name`
//! first: pactl parses options anywhere on its command line, so a
//! free-text name like `--server=x` would otherwise be taken as an option.

use super::model::{self, DeviceKind, ParseError, Snapshot};
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

const PACTL: &str = "pactl";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

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
    #[error("pactl not found")]
    NotInstalled,
    #[error("pactl {args} failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("pactl timed out")]
    Timeout,
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("nothing to act on")]
    NoTarget,
    #[error("refusing unsafe name {0:?}")]
    UnsafeName(String),
    #[error("io: {0}")]
    Io(std::io::Error),
}

#[async_trait]
pub trait AudioBackend: Send + Sync {
    async fn snapshot(&self) -> Result<Snapshot, BackendError>;
    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError>;
    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError>;
    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError>;
    async fn move_stream(&self, stream: u32, sink: &str) -> Result<(), BackendError>;
}

pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && !name.contains('\0')
}

fn check(name: &str) -> Result<(), BackendError> {
    if is_safe_name(name) {
        Ok(())
    } else {
        Err(BackendError::UnsafeName(name.to_string()))
    }
}

fn check_node(node: &Node) -> Result<(), BackendError> {
    match node {
        Node::Sink(n) | Node::Source(n) => check(n),
        Node::SinkInput(_) => Ok(()),
    }
}

fn node_parts<'a>(
    node: &Node,
    sink: &'a str,
    source: &'a str,
    input: &'a str,
) -> (&'a str, String) {
    match node {
        Node::Sink(n) => (sink, n.clone()),
        Node::Source(n) => (source, n.clone()),
        Node::SinkInput(i) => (input, i.to_string()),
    }
}

pub fn volume_args(node: &Node, percent: u16) -> Vec<String> {
    let (cmd, target) = node_parts(
        node,
        "set-sink-volume",
        "set-source-volume",
        "set-sink-input-volume",
    );
    vec![cmd.to_string(), target, format!("{percent}%")]
}

pub fn mute_args(node: &Node, mute: Mute) -> Vec<String> {
    let (cmd, target) = node_parts(
        node,
        "set-sink-mute",
        "set-source-mute",
        "set-sink-input-mute",
    );
    let value = match mute {
        Mute::On => "1",
        Mute::Off => "0",
        Mute::Toggle => "toggle",
    };
    vec![cmd.to_string(), target, value.to_string()]
}

pub fn default_args(kind: DeviceKind, name: &str) -> Vec<String> {
    let cmd = match kind {
        DeviceKind::Output => "set-default-sink",
        DeviceKind::Input => "set-default-source",
    };
    vec![cmd.to_string(), name.to_string()]
}

pub fn move_args(stream: u32, sink: &str) -> Vec<String> {
    vec![
        "move-sink-input".to_string(),
        stream.to_string(),
        sink.to_string(),
    ]
}

fn spawn_error(e: std::io::Error) -> BackendError {
    if e.kind() == std::io::ErrorKind::NotFound {
        BackendError::NotInstalled
    } else {
        BackendError::Io(e)
    }
}

/// Every `pactl` invocation starts here. Its messages (notably the
/// `pactl subscribe` event lines) are gettext-translated, so force the C
/// locale to keep them parseable.
fn pactl_command() -> Command {
    let mut cmd = Command::new(PACTL);
    cmd.env("LC_ALL", "C").env("LANGUAGE", "C");
    cmd
}

pub struct PactlBackend;

impl PactlBackend {
    async fn run(args: &[String]) -> Result<String, BackendError> {
        let output = tokio::time::timeout(
            COMMAND_TIMEOUT,
            pactl_command().args(args).kill_on_drop(true).output(),
        )
        .await
        .map_err(|_| BackendError::Timeout)?
        .map_err(spawn_error)?;
        if !output.status.success() {
            return Err(BackendError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn json(what: &[&str]) -> Result<String, BackendError> {
        let mut args = vec!["-f".to_string(), "json".to_string()];
        args.extend(what.iter().map(|s| s.to_string()));
        Self::run(&args).await
    }
}

#[async_trait]
impl AudioBackend for PactlBackend {
    async fn snapshot(&self) -> Result<Snapshot, BackendError> {
        let (info, sinks, sources, inputs) = tokio::try_join!(
            Self::json(&["info"]),
            Self::json(&["list", "sinks"]),
            Self::json(&["list", "sources"]),
            Self::json(&["list", "sink-inputs"]),
        )?;
        Ok(model::build_snapshot(&info, &sinks, &sources, &inputs)?)
    }

    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError> {
        check_node(node)?;
        Self::run(&volume_args(node, percent)).await.map(|_| ())
    }

    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError> {
        check_node(node)?;
        Self::run(&mute_args(node, mute)).await.map(|_| ())
    }

    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError> {
        check(name)?;
        Self::run(&default_args(kind, name)).await.map(|_| ())
    }

    async fn move_stream(&self, stream: u32, sink: &str) -> Result<(), BackendError> {
        check(sink)?;
        Self::run(&move_args(stream, sink)).await.map(|_| ())
    }
}

/// `pactl subscribe` prints e.g. `Event 'change' on sink #60`. Only
/// devices, app streams and the server (default-device changes) matter;
/// clients and source-outputs (apps opening the mic) are noise.
pub fn is_relevant_event(line: &str) -> bool {
    let Some((_, rest)) = line.split_once(" on ") else {
        return false;
    };
    let facility = rest.split(" #").next().unwrap_or_default().trim();
    matches!(facility, "sink" | "source" | "sink-input" | "server")
}

pub fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

fn bump(tx: &watch::Sender<u64>) {
    tx.send_modify(|n| *n = n.wrapping_add(1));
}

/// Runs one `pactl subscribe` until it exits. Returns whether it printed
/// anything (a healthy run resets the restart backoff).
async fn run_subscribe(tx: &watch::Sender<u64>) -> Result<bool, BackendError> {
    let mut child = pactl_command()
        .arg("subscribe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(spawn_error)?;
    let stdout = child.stdout.take().ok_or(BackendError::NoTarget)?;
    let mut lines = BufReader::new(stdout).lines();
    let mut saw_output = false;
    while let Some(line) = lines.next_line().await.map_err(BackendError::Io)? {
        saw_output = true;
        if is_relevant_event(&line) {
            bump(tx);
        }
    }
    let _ = child.wait().await;
    Ok(saw_output)
}

/// Keeps one `pactl subscribe` alive forever, bumping `tx` on every
/// relevant change and once after each (re)start so listeners resync.
pub fn spawn_subscriber(tx: watch::Sender<u64>) {
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            match run_subscribe(&tx).await {
                Ok(true) => backoff = Duration::from_secs(1),
                Ok(false) => log::warn!("pactl subscribe exited without output"),
                Err(e) => log::warn!("pactl subscribe failed: {e}"),
            }
            bump(&tx);
            tokio::time::sleep(backoff).await;
            backoff = next_backoff(backoff);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn volume_args_per_node() {
        assert_eq!(
            volume_args(&Node::Sink("s".into()), 55),
            v(&["set-sink-volume", "s", "55%"])
        );
        assert_eq!(
            volume_args(&Node::Source("m".into()), 0),
            v(&["set-source-volume", "m", "0%"])
        );
        assert_eq!(
            volume_args(&Node::SinkInput(7), 120),
            v(&["set-sink-input-volume", "7", "120%"])
        );
    }

    #[test]
    fn mute_args_per_mode() {
        assert_eq!(
            mute_args(&Node::Sink("s".into()), Mute::Toggle),
            v(&["set-sink-mute", "s", "toggle"])
        );
        assert_eq!(
            mute_args(&Node::Source("m".into()), Mute::On),
            v(&["set-source-mute", "m", "1"])
        );
        assert_eq!(
            mute_args(&Node::SinkInput(3), Mute::Off),
            v(&["set-sink-input-mute", "3", "0"])
        );
    }

    #[test]
    fn default_and_move_args() {
        assert_eq!(
            default_args(DeviceKind::Output, "s"),
            v(&["set-default-sink", "s"])
        );
        assert_eq!(
            default_args(DeviceKind::Input, "m"),
            v(&["set-default-source", "m"])
        );
        assert_eq!(move_args(9, "s"), v(&["move-sink-input", "9", "s"]));
    }

    #[test]
    fn names_starting_with_dash_are_refused() {
        assert!(!is_safe_name("-h"));
        assert!(!is_safe_name("--server=evil"));
        assert!(!is_safe_name(""));
        assert!(!is_safe_name("a\0b"));
        assert!(is_safe_name("alsa_output.usb-Razer-00.analog-stereo"));
    }

    #[tokio::test]
    async fn unsafe_names_never_reach_pactl() {
        let err = PactlBackend
            .set_default(DeviceKind::Output, "-h")
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::UnsafeName(_)), "{err:?}");
        let err = PactlBackend
            .set_volume(&Node::Sink("--x".into()), 5)
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::UnsafeName(_)), "{err:?}");
        let err = PactlBackend.move_stream(1, "-s").await.unwrap_err();
        assert!(matches!(err, BackendError::UnsafeName(_)), "{err:?}");
    }

    #[test]
    fn pactl_runs_in_the_c_locale() {
        let cmd = pactl_command();
        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert_eq!(cmd.as_std().get_program(), PACTL);
        for key in ["LC_ALL", "LANGUAGE"] {
            assert!(
                envs.contains(&(std::ffi::OsStr::new(key), Some(std::ffi::OsStr::new("C")))),
                "{key} not set to C: {envs:?}"
            );
        }
    }

    #[test]
    fn relevant_events() {
        assert!(is_relevant_event("Event 'change' on sink #60"));
        assert!(is_relevant_event("Event 'new' on sink-input #12"));
        assert!(is_relevant_event("Event 'change' on source #70"));
        assert!(is_relevant_event("Event 'change' on server #-1"));
        assert!(!is_relevant_event("Event 'new' on source-output #5"));
        assert!(!is_relevant_event("Event 'change' on client #90"));
        assert!(!is_relevant_event("garbage"));
    }

    #[test]
    fn backoff_doubles_to_a_cap() {
        assert_eq!(next_backoff(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(
            next_backoff(Duration::from_secs(16)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }
}
