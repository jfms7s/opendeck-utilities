//! Plain data types for the audio system. Backend-neutral: each backend
//! builds a `Snapshot` (`pactl/parse.rs`, `coreaudio/hal.rs`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Output,
    Input,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub index: u32,
    /// Stable node name, e.g. `alsa_output.usb-...analog-stereo`.
    pub name: String,
    pub description: String,
    /// Loudest channel, in percent (may exceed 100).
    pub volume: u16,
    pub muted: bool,
}

/// One application playback stream (a pactl "sink-input").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub index: u32,
    /// Index of the sink this stream plays to.
    pub sink: u32,
    pub app_name: String,
    pub binary: String,
    pub volume: u16,
    pub muted: bool,
}

/// An application with a microphone open (a pactl "source-output").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorder {
    /// Index of the source (input device) it records from.
    pub source: u32,
    /// `application.name`, else the process binary, else "unknown".
    pub app: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub default_sink: String,
    pub default_source: String,
    pub sinks: Vec<Device>,
    /// Inputs only - monitor sources are excluded.
    pub sources: Vec<Device>,
    pub streams: Vec<Stream>,
    /// Apps recording from an input; never a monitor source or a level
    /// meter's peak-detect stream. Always empty on macOS.
    pub recorders: Vec<Recorder>,
    /// What the backend that read this snapshot can do. Each backend fills
    /// it from its own constant (`pactl::CAPABILITIES`,
    /// `coreaudio::hal::CAPABILITIES`).
    pub caps: Capabilities,
}

/// What an audio backend can do, and how the deck and the page say what it
/// can't. One struct so a new capability is added here, in each backend's
/// constant and in `pi.js`'s `applyCapabilities`, not threaded field by
/// field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Per-app streams exist. When false (CoreAudio on macOS), App targets
    /// can't work at all - not just "not playing" right now.
    pub per_app: bool,
    /// The highest volume the audio system accepts (CoreAudio: 100, no
    /// boost); `None` when `max_volume` alone decides (PipeWire allows 150).
    pub volume_cap: Option<u16>,
    /// Tile label for an App target the backend can't serve.
    pub unsupported_label: &'static str,
    /// The page's note after "Application" when per-app is unsupported.
    pub unsupported_note: &'static str,
}

impl Capabilities {
    /// Everything supported: per-app streams, and no cap beyond the
    /// control's own `max_volume`.
    pub const FULL: Capabilities = Capabilities {
        per_app: true,
        volume_cap: None,
        unsupported_label: "",
        unsupported_note: "",
    };
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::FULL
    }
}

impl Snapshot {
    pub fn devices(&self, kind: DeviceKind) -> &[Device] {
        match kind {
            DeviceKind::Output => &self.sinks,
            DeviceKind::Input => &self.sources,
        }
    }

    pub fn default_name(&self, kind: DeviceKind) -> &str {
        match kind {
            DeviceKind::Output => &self.default_sink,
            DeviceKind::Input => &self.default_source,
        }
    }

    pub fn sink_by_index(&self, index: u32) -> Option<&Device> {
        self.sinks.iter().find(|d| d.index == index)
    }
}

/// A PipeWire/PulseAudio monitor source (what an output plays), by name.
pub fn is_monitor(name: &str) -> bool {
    name.ends_with(".monitor")
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn fixture_snapshot() -> Snapshot {
        crate::audio::pactl::parse::build_snapshot(
            include_str!("fixtures/info.json"),
            include_str!("fixtures/sinks.json"),
            include_str!("fixtures/sources.json"),
            include_str!("fixtures/sink_inputs.json"),
            "[]",
        )
        .unwrap()
    }
}
