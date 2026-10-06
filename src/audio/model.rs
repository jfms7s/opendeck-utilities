//! Plain data types for the audio system, and parsing of `pactl -f json`
//! output into them. Nothing here runs `pactl` - see `backend.rs`.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub default_sink: String,
    pub default_source: String,
    pub sinks: Vec<Device>,
    /// Inputs only - monitor sources are excluded.
    pub sources: Vec<Device>,
    pub streams: Vec<Stream>,
    /// The audio system has no per-app streams (CoreAudio on macOS), so App
    /// targets can't work at all - not just "not playing" right now.
    pub apps_unsupported: bool,
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

#[derive(Debug, thiserror::Error)]
#[error("invalid pactl json: {0}")]
pub struct ParseError(#[from] serde_json::Error);

#[derive(Deserialize)]
struct RawChannel {
    value_percent: String,
}

#[derive(Deserialize)]
struct RawDevice {
    index: u32,
    name: String,
    #[serde(default)]
    description: String,
    mute: bool,
    #[serde(default)]
    volume: HashMap<String, RawChannel>,
}

#[derive(Deserialize)]
struct RawStream {
    index: u32,
    sink: u32,
    mute: bool,
    #[serde(default)]
    volume: HashMap<String, RawChannel>,
    #[serde(default)]
    properties: HashMap<String, Value>,
}

#[derive(Deserialize)]
struct RawInfo {
    #[serde(default)]
    default_sink_name: String,
    #[serde(default)]
    default_source_name: String,
}

fn percent(volume: &HashMap<String, RawChannel>) -> u16 {
    volume
        .values()
        .filter_map(|c| {
            c.value_percent
                .trim()
                .trim_end_matches('%')
                .parse::<u16>()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

fn prop(properties: &HashMap<String, Value>, key: &str) -> String {
    properties
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn parse_devices(json: &str) -> Result<Vec<Device>, ParseError> {
    let raw: Vec<RawDevice> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .map(|d| Device {
            volume: percent(&d.volume),
            description: if d.description.is_empty() {
                d.name.clone()
            } else {
                d.description
            },
            index: d.index,
            name: d.name,
            muted: d.mute,
        })
        .collect())
}

fn parse_streams(json: &str) -> Result<Vec<Stream>, ParseError> {
    let raw: Vec<RawStream> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .map(|s| Stream {
            index: s.index,
            sink: s.sink,
            app_name: prop(&s.properties, "application.name"),
            binary: prop(&s.properties, "application.process.binary"),
            volume: percent(&s.volume),
            muted: s.mute,
        })
        .collect())
}

pub fn is_monitor(name: &str) -> bool {
    name.ends_with(".monitor")
}

/// Combines the four `pactl -f json` outputs (`info`, `list sinks`,
/// `list sources`, `list sink-inputs`) into one snapshot. Unparseable
/// server info or device lists are an error; unparseable streams are not.
pub fn build_snapshot(
    info: &str,
    sinks: &str,
    sources: &str,
    sink_inputs: &str,
) -> Result<Snapshot, ParseError> {
    let info: RawInfo = serde_json::from_str(info)?;
    let mut sources = parse_devices(sources)?;
    sources.retain(|d| !is_monitor(&d.name));
    // One odd app stream must not blank the device controls: streams only
    // matter to App controls, which then show "not playing".
    let streams = parse_streams(sink_inputs).unwrap_or_else(|e| {
        log::warn!("ignoring unparseable app streams: {e}");
        Vec::new()
    });
    Ok(Snapshot {
        default_sink: info.default_sink_name,
        default_source: info.default_source_name,
        sinks: parse_devices(sinks)?,
        sources,
        streams,
        apps_unsupported: false,
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn fixture_snapshot() -> Snapshot {
        build_snapshot(
            include_str!("fixtures/info.json"),
            include_str!("fixtures/sinks.json"),
            include_str!("fixtures/sources.json"),
            include_str!("fixtures/sink_inputs.json"),
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::fixture_snapshot;
    use super::*;

    #[test]
    fn parses_defaults_from_info() {
        let s = fixture_snapshot();
        assert_eq!(
            s.default_sink,
            "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo"
        );
        assert_eq!(s.default_source, "alsa_input.usb-webcam-02.mono-fallback");
    }

    #[test]
    fn device_volume_is_the_loudest_channel() {
        let s = fixture_snapshot();
        assert_eq!(s.sinks.len(), 2);
        assert_eq!(s.sinks[1].volume, 47);
        assert_eq!(s.sinks[1].description, "Razer Leviathan V2 Analog Stereo");
    }

    #[test]
    fn monitor_sources_are_excluded() {
        let s = fixture_snapshot();
        assert_eq!(s.sources.len(), 1);
        assert!(s.sources[0].muted);
        assert_eq!(s.sources[0].volume, 80);
    }

    #[test]
    fn streams_carry_app_identity_and_sink() {
        let s = fixture_snapshot();
        assert_eq!(s.streams.len(), 4);
        assert_eq!(s.streams[0].binary, "chrome");
        assert_eq!(s.streams[0].app_name, "Google Chrome");
        assert_eq!(s.streams[2].sink, 60);
        assert!(s.streams[2].muted);
    }

    #[test]
    fn stream_without_properties_gets_empty_identity() {
        let s = fixture_snapshot();
        assert_eq!(s.streams[3].app_name, "");
        assert_eq!(s.streams[3].binary, "");
    }

    #[test]
    fn missing_description_falls_back_to_name() {
        let devices = parse_devices(r#"[{"index":1,"name":"n","mute":false}]"#).unwrap();
        assert_eq!(devices[0].description, "n");
        assert_eq!(devices[0].volume, 0);
    }

    #[test]
    fn malformed_info_or_devices_is_an_error() {
        assert!(build_snapshot("{", "[]", "[]", "[]").is_err());
        assert!(build_snapshot("{}", "not json", "[]", "[]").is_err());
        assert!(build_snapshot("{}", "[]", "not json", "[]").is_err());
    }

    #[test]
    fn malformed_streams_leave_the_devices_usable() {
        let s = build_snapshot(
            include_str!("fixtures/info.json"),
            include_str!("fixtures/sinks.json"),
            include_str!("fixtures/sources.json"),
            r#"[{"index": "not a number"}]"#,
        )
        .unwrap();
        assert_eq!(s.sinks.len(), 2);
        assert!(s.streams.is_empty());
    }

    /// Real `pactl -f json` output (pactl 17.0, PipeWire 1.6.9), scrubbed of
    /// serials, user, host and addresses. The hand-written fixtures above
    /// keep the tests readable; this one proves the parser copes with the
    /// full property blocks.
    #[test]
    fn parses_real_pactl_17_output() {
        let s = build_snapshot(
            include_str!("fixtures/pactl-17/info.json"),
            include_str!("fixtures/pactl-17/sinks.json"),
            include_str!("fixtures/pactl-17/sources.json"),
            include_str!("fixtures/pactl-17/sink_inputs.json"),
        )
        .unwrap();
        assert_eq!(s.sinks.len(), 4);
        assert_eq!(s.sources.len(), 2, "monitors dropped");
        assert_eq!(s.streams.len(), 1);
        assert_eq!(s.streams[0].binary, "chrome");
        let default = s.sinks.iter().find(|d| d.name == s.default_sink).unwrap();
        assert_eq!(default.description, "Razer Leviathan V2 Analog Stereo");
        assert!(s.sources.iter().any(|d| d.name == s.default_source));
    }
}
