//! Which device or app streams a control acts on, resolved against a
//! snapshot. Pure - no I/O.

use super::model::{Device, DeviceKind, Snapshot, Stream, is_monitor};
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
    /// Every microphone: mute acts on all of them, volume and device
    /// switching on the default one.
    AllInputs,
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
    /// Every non-monitor input (never empty); `default` is the default
    /// source, or the first input when the default is missing.
    Inputs {
        devices: Vec<Device>,
        default: Device,
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
            Resolved::Inputs { .. } => ALL_INPUTS.to_string(),
            Resolved::Unavailable { label } | Resolved::NotPlaying { label } => label.clone(),
        }
    }
}

const ALL_INPUTS: &str = "All inputs";

/// Every input is muted: an `Inputs` target reads as muted only then.
pub fn all_muted(devices: &[Device]) -> bool {
    devices.iter().all(|d| d.muted)
}

/// The apps recording from the input(s) a control acts on, each named
/// once, in snapshot order. Empty for outputs, apps and missing targets.
pub fn recorders_for(resolved: &Resolved, snap: &Snapshot) -> Vec<String> {
    let on = |index: u32| match resolved {
        Resolved::Device {
            kind: DeviceKind::Input,
            device,
        } => device.index == index,
        Resolved::Inputs { devices, .. } => devices.iter().any(|d| d.index == index),
        _ => false,
    };
    let mut apps: Vec<String> = Vec::new();
    for r in snap.recorders.iter().filter(|r| on(r.source)) {
        if !apps.contains(&r.app) {
            apps.push(r.app.clone());
        }
    }
    apps
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
        TargetKind::App if snap.apps_unsupported => Resolved::Unavailable {
            label: "not on macOS".to_string(),
        },
        TargetKind::App if name.is_empty() => Resolved::Unavailable {
            label: "pick an app".to_string(),
        },
        TargetKind::AllInputs => {
            let devices: Vec<Device> = snap
                .sources
                .iter()
                .filter(|d| !is_monitor(&d.name))
                .cloned()
                .collect();
            let default = devices
                .iter()
                .find(|d| d.name == snap.default_source)
                .or_else(|| devices.first())
                .cloned();
            match default {
                Some(default) => Resolved::Inputs { devices, default },
                None => Resolved::Unavailable {
                    label: "no inputs".to_string(),
                },
            }
        }
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

    #[test]
    fn an_app_target_without_per_app_audio_is_unavailable() {
        let mut snap = crate::audio::model::test_support::fixture_snapshot();
        assert!(matches!(
            resolve(TargetKind::App, "chrome", &snap),
            Resolved::App { .. }
        ));
        snap.apps_unsupported = true;
        assert_eq!(
            resolve(TargetKind::App, "chrome", &snap),
            Resolved::Unavailable {
                label: "not on macOS".to_string()
            }
        );
    }
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

    fn two_inputs() -> Snapshot {
        let mut snap = fixture_snapshot();
        let mut other = snap.sources[0].clone();
        other.index = 71;
        other.name = "alsa_input.pci-analog-stereo".into();
        other.description = "Analog mic".into();
        other.muted = false;
        snap.sources.push(other);
        snap
    }

    #[test]
    fn all_inputs_lists_every_input_with_the_default_first_choice() {
        let r = resolve(TargetKind::AllInputs, "ignored", &two_inputs());
        let Resolved::Inputs { devices, default } = &r else {
            panic!("{r:?}")
        };
        assert_eq!(
            devices.iter().map(|d| d.index).collect::<Vec<_>>(),
            vec![70, 71]
        );
        assert_eq!(default.index, 70);
        assert_eq!(r.label(), "All inputs");
    }

    #[test]
    fn all_inputs_never_includes_a_monitor() {
        let mut snap = two_inputs();
        let mut monitor = snap.sources[0].clone();
        monitor.index = 72;
        monitor.name = "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo.monitor".into();
        snap.sources.push(monitor);
        let Resolved::Inputs { devices, .. } = resolve(TargetKind::AllInputs, "", &snap) else {
            panic!()
        };
        assert!(devices.iter().all(|d| !d.name.ends_with(".monitor")));
        assert_eq!(devices.len(), 2);
    }

    #[test]
    fn all_inputs_falls_back_to_the_first_input_and_needs_one() {
        let mut snap = two_inputs();
        snap.default_source = "gone".into();
        let Resolved::Inputs { default, .. } = resolve(TargetKind::AllInputs, "", &snap) else {
            panic!()
        };
        assert_eq!(default.index, 70);
        snap.sources.clear();
        assert_eq!(
            resolve(TargetKind::AllInputs, "", &snap),
            Resolved::Unavailable {
                label: "no inputs".into()
            }
        );
    }

    #[test]
    fn all_inputs_round_trips_as_snake_case() {
        let k: TargetKind = serde_json::from_str("\"all_inputs\"").unwrap();
        assert_eq!(k, TargetKind::AllInputs);
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"all_inputs\"");
    }

    fn recording(snap: &mut Snapshot, source: u32, app: &str) {
        snap.recorders.push(crate::audio::model::Recorder {
            source,
            app: app.into(),
        });
    }

    #[test]
    fn recorders_are_named_once_and_only_for_inputs() {
        let mut snap = two_inputs();
        recording(&mut snap, 70, "Firefox");
        recording(&mut snap, 70, "Firefox");
        recording(&mut snap, 71, "Zoom");
        recording(&mut snap, 61, "OBS Studio");
        let mic = resolve(TargetKind::DefaultInput, "", &snap);
        assert_eq!(recorders_for(&mic, &snap), vec!["Firefox"]);
        let all = resolve(TargetKind::AllInputs, "", &snap);
        assert_eq!(recorders_for(&all, &snap), vec!["Firefox", "Zoom"]);
        let speakers = resolve(TargetKind::DefaultOutput, "", &snap);
        assert!(recorders_for(&speakers, &snap).is_empty());
        let app = resolve(TargetKind::App, "chrome", &snap);
        assert!(recorders_for(&app, &snap).is_empty());
    }
}
