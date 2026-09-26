//! Which device or app streams a control acts on, resolved against a
//! snapshot. Pure - no I/O.

use super::model::{Device, DeviceKind, Snapshot, Stream};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    #[default]
    DefaultOutput,
    DefaultInput,
    Output,
    Input,
    App,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Device {
        kind: DeviceKind,
        device: Device,
    },
    /// Never empty.
    App {
        streams: Vec<Stream>,
    },
    Unavailable {
        label: String,
    },
    NotPlaying {
        label: String,
    },
}

impl Resolved {
    pub fn label(&self) -> String {
        match self {
            Resolved::Device { device, .. } => device.description.clone(),
            Resolved::App { streams } => streams
                .first()
                .map(|s| {
                    if s.app_name.is_empty() {
                        s.binary.clone()
                    } else {
                        s.app_name.clone()
                    }
                })
                .unwrap_or_default(),
            Resolved::Unavailable { label } | Resolved::NotPlaying { label } => label.clone(),
        }
    }
}

pub fn app_matches(stream: &Stream, name: &str) -> bool {
    stream.binary.eq_ignore_ascii_case(name) || stream.app_name.eq_ignore_ascii_case(name)
}

fn by_name(kind: DeviceKind, name: &str, snap: &Snapshot, fallback: &str) -> Resolved {
    snap.devices(kind)
        .iter()
        .find(|d| d.name == name)
        .map(|d| Resolved::Device {
            kind,
            device: d.clone(),
        })
        .unwrap_or_else(|| Resolved::Unavailable {
            label: fallback.to_string(),
        })
}

pub fn resolve(kind: TargetKind, name: &str, snap: &Snapshot) -> Resolved {
    let name = name.trim();
    match kind {
        TargetKind::DefaultOutput => by_name(
            DeviceKind::Output,
            &snap.default_sink,
            snap,
            "Default output",
        ),
        TargetKind::DefaultInput => by_name(
            DeviceKind::Input,
            &snap.default_source,
            snap,
            "Default input",
        ),
        TargetKind::Output | TargetKind::Input if name.is_empty() => Resolved::Unavailable {
            label: "pick a device".to_string(),
        },
        TargetKind::Output => by_name(DeviceKind::Output, name, snap, name),
        TargetKind::Input => by_name(DeviceKind::Input, name, snap, name),
        TargetKind::App if name.is_empty() => Resolved::Unavailable {
            label: "pick an app".to_string(),
        },
        TargetKind::App => {
            let streams: Vec<Stream> = snap
                .streams
                .iter()
                .filter(|s| app_matches(s, name))
                .cloned()
                .collect();
            if streams.is_empty() {
                Resolved::NotPlaying {
                    label: name.to_string(),
                }
            } else {
                Resolved::App { streams }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::test_support::fixture_snapshot;

    #[test]
    fn default_output_follows_the_default_sink() {
        let r = resolve(TargetKind::DefaultOutput, "", &fixture_snapshot());
        let Resolved::Device { kind, device } = &r else {
            panic!("{r:?}")
        };
        assert_eq!(*kind, DeviceKind::Output);
        assert_eq!(device.index, 61);
    }

    #[test]
    fn default_input_follows_the_default_source() {
        let r = resolve(TargetKind::DefaultInput, "", &fixture_snapshot());
        assert!(
            matches!(r, Resolved::Device { kind: DeviceKind::Input, ref device } if device.index == 70)
        );
    }

    #[test]
    fn specific_output_by_node_name() {
        let r = resolve(
            TargetKind::Output,
            " alsa_output.pci-0000_2f_00.4.iec958-stereo ",
            &fixture_snapshot(),
        );
        assert!(matches!(r, Resolved::Device { ref device, .. } if device.index == 60));
    }

    #[test]
    fn absent_device_is_unavailable_without_fallback() {
        let r = resolve(TargetKind::Output, "unplugged-headset", &fixture_snapshot());
        assert_eq!(
            r,
            Resolved::Unavailable {
                label: "unplugged-headset".into()
            }
        );
    }

    #[test]
    fn a_monitor_is_not_a_valid_input() {
        let r = resolve(
            TargetKind::Input,
            "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo.monitor",
            &fixture_snapshot(),
        );
        assert!(matches!(r, Resolved::Unavailable { .. }));
    }

    #[test]
    fn empty_specific_name_asks_for_a_device() {
        let r = resolve(TargetKind::Input, "  ", &fixture_snapshot());
        assert_eq!(
            r,
            Resolved::Unavailable {
                label: "pick a device".into()
            }
        );
    }

    #[test]
    fn app_matches_binary_or_name_case_insensitively_and_groups_streams() {
        for name in ["chrome", "CHROME", "google chrome"] {
            let r = resolve(TargetKind::App, name, &fixture_snapshot());
            let Resolved::App { streams } = &r else {
                panic!("{name}: {r:?}")
            };
            assert_eq!(
                streams.iter().map(|s| s.index).collect::<Vec<_>>(),
                vec![100, 101]
            );
        }
    }

    #[test]
    fn app_with_no_stream_is_not_playing() {
        let r = resolve(TargetKind::App, "firefox", &fixture_snapshot());
        assert_eq!(
            r,
            Resolved::NotPlaying {
                label: "firefox".into()
            }
        );
    }

    #[test]
    fn empty_app_name_never_matches_nameless_streams() {
        let r = resolve(TargetKind::App, "", &fixture_snapshot());
        assert_eq!(
            r,
            Resolved::Unavailable {
                label: "pick an app".into()
            }
        );
    }

    #[test]
    fn labels() {
        let s = fixture_snapshot();
        assert_eq!(
            resolve(TargetKind::DefaultOutput, "", &s).label(),
            "Razer Leviathan V2 Analog Stereo"
        );
        assert_eq!(resolve(TargetKind::App, "spotify", &s).label(), "Spotify");
    }
}
