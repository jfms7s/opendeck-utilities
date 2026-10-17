//! CoreAudio (HAL) state as plain data, and its conversion into the shared
//! `Snapshot`. Platform-neutral so it is tested everywhere; only
//! `coreaudio.rs` (macOS) reads the real HAL.

use super::model::{Device, Snapshot};
use std::collections::BTreeSet;
use std::time::Duration;

/// Without property listeners, `subscribe` re-reads the HAL this often to
/// notice outside changes.
pub const POLL: Duration = Duration::from_millis(500);
/// With listeners, changes arrive as callbacks; this slow re-read is only a
/// safety net for a missed one.
pub const SAFETY_POLL: Duration = Duration::from_secs(10);

/// `kAudioHardwarePropertyDevices` (`'dev#'`): the system object's device
/// list. Own copies of the selectors so the routing below is tested on
/// every platform.
pub const DEVICES_SELECTOR: u32 = u32::from_be_bytes(*b"dev#");
/// `kAudioDevicePropertyStreamConfiguration` (`'slay'`): a device's
/// channels in a scope, which decide the elements it has listeners on.
pub const STREAM_CONFIG_SELECTOR: u32 = u32::from_be_bytes(*b"slay");

/// What a listener batch asks of `subscribe`'s loop besides re-reading.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Wake {
    /// Run a listener sync pass now.
    pub resync: bool,
    /// The device's channels changed (e.g. a Bluetooth headset gained its
    /// input): re-check which elements it needs listeners on.
    pub recheck: Option<u32>,
}

/// Routes a batch from `object` naming `selectors`: a new device list means
/// devices that appeared need listeners; a new stream configuration means
/// the device's own elements may have changed.
pub fn route(object: u32, selectors: impl IntoIterator<Item = u32>) -> Wake {
    let mut wake = Wake::default();
    for s in selectors {
        if s == DEVICES_SELECTOR {
            wake.resync = true;
        } else if s == STREAM_CONFIG_SELECTOR {
            wake.resync = true;
            wake.recheck = Some(object);
        }
    }
    wake
}

/// One registered listener: object id, selector, scope, element.
pub type Prop = (u32, u32, u32, u32);

/// Which devices are fully synced, and every property already registered,
/// so a retry or a re-check never registers the same listener twice.
#[derive(Debug, Default)]
pub struct Listened {
    synced: BTreeSet<u32>,
    props: BTreeSet<Prop>,
}

impl Listened {
    /// The ids in `current` that need a sync pass, which are recorded as
    /// synced from now on. Ids no longer present are forgotten with their
    /// properties: a removed device's listeners go with it, so if it comes
    /// back under the same id it is registered again from scratch.
    pub fn new_devices(&mut self, current: &[u32]) -> Vec<u32> {
        self.synced.retain(|id| current.contains(id));
        self.props.retain(|p| current.contains(&p.0));
        current
            .iter()
            .copied()
            .filter(|&id| self.synced.insert(id))
            .collect()
    }

    /// Marks `id` for another sync pass (a failed registration, or its
    /// channels changed). Its registered properties are kept and skipped.
    pub fn forget(&mut self, id: u32) {
        self.synced.remove(&id);
    }

    /// Whether `prop` still needs registering.
    pub fn needs(&self, prop: Prop) -> bool {
        !self.props.contains(&prop)
    }

    pub fn record(&mut self, prop: Prop) {
        self.props.insert(prop);
    }
}

/// Why the listeners aren't trusted, for the log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Degraded {
    /// A registration (or detaching notifications from the run loop) failed.
    Registration,
    /// The HAL changed but no callback reported it.
    Silent,
}

/// Whether the listeners are trustworthy, and so how often to re-read.
/// Healthy means registered *and* heard: a safety re-read that finds a
/// change no callback reported marks them silent until a callback arrives.
#[derive(Debug)]
pub struct ListenHealth {
    ok: bool,
    live: bool,
    degraded: bool,
}

impl Default for ListenHealth {
    fn default() -> Self {
        Self {
            ok: false,
            live: true,
            degraded: false,
        }
    }
}

impl ListenHealth {
    /// Records one pass: whether every registration succeeded, whether any
    /// callback arrived since the last pass, and whether the HAL changed
    /// with no callback (`missed`, checked after a grace period). Returns
    /// why on each healthy-to-degraded transition (including the first
    /// pass), so the caller warns once per fallback, not on every retry.
    pub fn pass(&mut self, registered: bool, heard: bool, missed: bool) -> Option<Degraded> {
        if missed {
            self.live = false;
        } else if heard {
            self.live = true;
        }
        self.ok = registered && self.live;
        let why = if !registered {
            Degraded::Registration
        } else {
            Degraded::Silent
        };
        let newly = !self.ok && !self.degraded;
        self.degraded = !self.ok;
        newly.then_some(why)
    }

    pub fn poll_interval(&self) -> Duration {
        if self.ok { SAFETY_POLL } else { POLL }
    }
}

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
        volume_cap: Some(100),
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
        assert_eq!(snap.volume_cap, Some(100));
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
    fn listener_batches_route_to_resyncs_and_rechecks() {
        assert_eq!(DEVICES_SELECTOR, 0x6465_7623);
        assert_eq!(STREAM_CONFIG_SELECTOR, 0x736c_6179);
        let volume = u32::from_be_bytes(*b"volm");
        let mute = u32::from_be_bytes(*b"mute");
        let default_output = u32::from_be_bytes(*b"dOut");
        assert_eq!(route(1, [volume, mute, default_output]), Wake::default());
        assert_eq!(route(1, []), Wake::default());
        assert_eq!(
            route(1, [volume, DEVICES_SELECTOR]),
            Wake {
                resync: true,
                recheck: None
            }
        );
        assert_eq!(
            route(42, [STREAM_CONFIG_SELECTOR]),
            Wake {
                resync: true,
                recheck: Some(42)
            }
        );
    }

    #[test]
    fn new_devices_get_listeners_once() {
        let mut l = Listened::default();
        assert_eq!(l.new_devices(&[10, 11]), [10, 11]);
        assert!(l.new_devices(&[10, 11]).is_empty());
        // A device plugged in: only it needs listeners.
        assert_eq!(l.new_devices(&[10, 11, 12]), [12]);
        // 11 unplugged, then back under the same id: registered again.
        let p11 = (11, 1, 2, 0);
        l.record(p11);
        assert!(l.new_devices(&[10, 12]).is_empty());
        assert!(l.needs(p11), "its listeners went with it");
        assert_eq!(l.new_devices(&[10, 11, 12]), [11]);
        // A failed registration is retried on the next sync.
        l.forget(12);
        assert_eq!(l.new_devices(&[10, 11, 12]), [12]);
    }

    #[test]
    fn a_retry_or_recheck_skips_listeners_already_registered() {
        let mut l = Listened::default();
        assert_eq!(l.new_devices(&[10]), [10]);
        let (vol, mute, input_vol) = ((10, 1, 2, 0), (10, 3, 2, 0), (10, 1, 4, 0));
        l.record(vol);
        // `mute` failed: the device is retried, but `vol` isn't re-added.
        l.forget(10);
        assert_eq!(l.new_devices(&[10]), [10]);
        assert!(!l.needs(vol));
        assert!(l.needs(mute));
        l.record(mute);
        // Its channels changed (an input appeared): only the new one is added.
        l.forget(10);
        assert_eq!(l.new_devices(&[10]), [10]);
        assert_eq!(
            [vol, mute, input_vol].map(|p| l.needs(p)),
            [false, false, true]
        );
    }

    #[test]
    fn listeners_poll_slowly_and_failures_fall_back_warning_per_fallback() {
        let mut h = ListenHealth::default();
        // Before the first pass nothing is trusted yet.
        assert_eq!(h.poll_interval(), POLL);
        assert_eq!(h.pass(true, false, false), None);
        assert_eq!(h.poll_interval(), SAFETY_POLL);
        assert!(SAFETY_POLL >= Duration::from_secs(10));
        assert_eq!(h.pass(false, false, false), Some(Degraded::Registration));
        assert_eq!(h.poll_interval(), POLL);
        assert_eq!(h.pass(false, false, false), None, "once per fallback");
        assert_eq!(h.pass(true, false, false), None);
        assert_eq!(h.poll_interval(), SAFETY_POLL);
        // Recovered, then failed again: warned again.
        assert_eq!(h.pass(false, false, false), Some(Degraded::Registration));
        assert_eq!(POLL, Duration::from_millis(500));
    }

    #[test]
    fn a_change_no_callback_reported_falls_back_until_one_arrives() {
        let mut h = ListenHealth::default();
        assert_eq!(h.pass(true, false, false), None);
        assert_eq!(h.pass(true, false, true), Some(Degraded::Silent));
        assert_eq!(h.poll_interval(), POLL);
        // Still silent: stays on POLL without warning again.
        assert_eq!(h.pass(true, false, true), None);
        assert_eq!(h.pass(true, false, false), None);
        assert_eq!(h.poll_interval(), POLL);
        // A callback arrives: listeners work after all.
        assert_eq!(h.pass(true, true, false), None);
        assert_eq!(h.poll_interval(), SAFETY_POLL);
        // A registration failure while silent reports the registration.
        let mut h = ListenHealth::default();
        assert_eq!(h.pass(true, false, true), Some(Degraded::Silent));
        assert_eq!(h.pass(true, true, false), None);
        assert_eq!(h.pass(false, true, false), Some(Degraded::Registration));
    }

    #[test]
    fn an_unknown_default_id_names_no_device() {
        let snap = build_snapshot(&[device(10, "speakers", 2, 0)], Some(99), None);
        assert_eq!(snap.default_name(DeviceKind::Output), "");
    }
}
