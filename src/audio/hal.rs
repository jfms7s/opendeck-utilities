//! CoreAudio (HAL) state as plain data, and its conversion into the shared
//! `Snapshot`. Platform-neutral so it is tested everywhere; only
//! `coreaudio.rs` (macOS) reads the real HAL.

use super::model::{Device, Snapshot};

/// A device's volume and mute in one direction (output or input). `None`
/// where the device has no such control (common on HDMI outputs).
#[derive(Debug, Clone, PartialEq)]
pub struct Controls {
    /// The loudest channel's volume scalar, 0..1.
    pub volume: Option<f32>,
    pub muted: Option<bool>,
}

/// One HAL audio device. `output`/`input` are `None` when it has no
/// channels in that direction.
#[derive(Debug, Clone, PartialEq)]
pub struct HalDevice {
    /// The `AudioObjectID`; changes across reboots.
    pub id: u32,
    /// `kAudioDevicePropertyDeviceUID`; stable, so it is the saved name.
    pub uid: String,
    pub name: String,
    pub output: Option<Controls>,
    pub input: Option<Controls>,
}

/// CoreAudio volumes are scalars 0..1; the plugin speaks whole percents.
pub fn scalar_to_percent(scalar: f32) -> u16 {
    if scalar.is_nan() {
        return 0;
    }
    (scalar.clamp(0.0, 1.0) * 100.0).round() as u16
}

/// Above 100 % (allowed on Linux) is clamped: the HAL has no boost.
pub fn percent_to_scalar(percent: u16) -> f32 {
    f32::from(percent.min(100)) / 100.0
}

pub fn loudest(channels: &[f32]) -> Option<f32> {
    channels.iter().copied().reduce(f32::max)
}

fn to_device(d: &HalDevice, controls: &Controls) -> Device {
    Device {
        index: d.id,
        name: d.uid.clone(),
        description: d.name.clone(),
        volume: controls.volume.map_or(100, scalar_to_percent),
        muted: controls.muted.unwrap_or(false),
    }
}

/// The shared snapshot: outputs and inputs in HAL order, defaults by UID.
/// macOS has no per-app streams, so `streams` is always empty.
pub fn build_snapshot(
    devices: &[HalDevice],
    default_output: Option<u32>,
    default_input: Option<u32>,
) -> Snapshot {
    let uid_of = |id: Option<u32>| {
        id.and_then(|id| devices.iter().find(|d| d.id == id))
            .map(|d| d.uid.clone())
            .unwrap_or_default()
    };
    Snapshot {
        default_sink: uid_of(default_output),
        default_source: uid_of(default_input),
        sinks: devices
            .iter()
            .filter_map(|d| d.output.as_ref().map(|c| to_device(d, c)))
            .collect(),
        sources: devices
            .iter()
            .filter_map(|d| d.input.as_ref().map(|c| to_device(d, c)))
            .collect(),
        streams: Vec::new(),
        apps_unsupported: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::DeviceKind;

    fn device(id: u32, uid: &str, outs: u32, ins: u32) -> HalDevice {
        HalDevice {
            id,
            uid: uid.to_string(),
            name: format!("Device {id}"),
            output: (outs > 0).then_some(Controls {
                volume: Some(0.5),
                muted: Some(false),
            }),
            input: (ins > 0).then_some(Controls {
                volume: Some(0.25),
                muted: Some(true),
            }),
        }
    }

    #[test]
    fn scalars_become_whole_percents() {
        assert_eq!(scalar_to_percent(0.0), 0);
        assert_eq!(scalar_to_percent(0.5), 50);
        assert_eq!(scalar_to_percent(0.333), 33);
        assert_eq!(scalar_to_percent(0.996), 100);
        assert_eq!(scalar_to_percent(1.2), 100);
        assert_eq!(scalar_to_percent(-0.1), 0);
        assert_eq!(scalar_to_percent(f32::NAN), 0);
    }

    #[test]
    fn percents_become_clamped_scalars() {
        assert_eq!(percent_to_scalar(0), 0.0);
        assert_eq!(percent_to_scalar(50), 0.5);
        assert_eq!(percent_to_scalar(100), 1.0);
        assert_eq!(percent_to_scalar(150), 1.0);
    }

    #[test]
    fn the_loudest_channel_wins() {
        assert_eq!(loudest(&[]), None);
        assert_eq!(loudest(&[0.2, 0.7, 0.4]), Some(0.7));
    }

    #[test]
    fn devices_land_in_the_lists_their_channels_allow() {
        let devices = [
            device(10, "speakers", 2, 0),
            device(11, "headset", 2, 1),
            device(12, "mic", 0, 1),
        ];
        let snap = build_snapshot(&devices, Some(11), Some(12));
        let names = |kind| {
            snap.devices(kind)
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(DeviceKind::Output), ["speakers", "headset"]);
        assert_eq!(names(DeviceKind::Input), ["headset", "mic"]);
        assert_eq!(snap.default_name(DeviceKind::Output), "headset");
        assert_eq!(snap.default_name(DeviceKind::Input), "mic");
        assert!(snap.streams.is_empty());
        assert!(snap.apps_unsupported);
        let headset_in = &snap.devices(DeviceKind::Input)[0];
        assert_eq!(
            (headset_in.index, headset_in.volume, headset_in.muted),
            (11, 25, true)
        );
        assert_eq!(headset_in.description, "Device 11");
    }

    #[test]
    fn a_device_without_controls_reads_full_and_unmuted() {
        let mut d = device(10, "hdmi", 2, 0);
        d.output = Some(Controls {
            volume: None,
            muted: None,
        });
        let snap = build_snapshot(&[d], Some(10), None);
        let hdmi = &snap.devices(DeviceKind::Output)[0];
        assert_eq!((hdmi.volume, hdmi.muted), (100, false));
        assert_eq!(snap.default_name(DeviceKind::Input), "");
    }

    #[test]
    fn an_unknown_default_id_names_no_device() {
        let snap = build_snapshot(&[device(10, "speakers", 2, 0)], Some(99), None);
        assert_eq!(snap.default_name(DeviceKind::Output), "");
    }
}
