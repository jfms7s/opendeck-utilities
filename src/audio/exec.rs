//! Carries out one `Operation` on a resolved target through a backend.

use super::backend::{AudioBackend, BackendError, Mute, Node};
use super::gesture::Operation;
use super::model::{Device, DeviceKind, Snapshot, Stream};
use super::ops::{adjust_volume, cycle_candidates};
use super::settings::AudioSettings;
use super::target::{Resolved, all_muted};
use crate::cycle::step_in;

/// `max_volume`, but never above what the audio system accepts.
fn effective_max(settings: &AudioSettings, cap: Option<u16>) -> u16 {
    settings.max_volume().min(cap.unwrap_or(u16::MAX))
}

pub async fn execute(
    backend: &dyn AudioBackend,
    snap: &Snapshot,
    resolved: &Resolved,
    settings: &AudioSettings,
    op: &Operation,
) -> Result<(), BackendError> {
    if *op == Operation::None {
        return Ok(());
    }
    match resolved {
        Resolved::Device { kind, device } => {
            let node = match kind {
                DeviceKind::Output => Node::Sink(device.name.clone()),
                DeviceKind::Input => Node::Source(device.name.clone()),
            };
            execute_on_device(backend, snap, *kind, &node, device.volume, settings, op).await
        }
        Resolved::App { streams } => execute_on_app(backend, snap, streams, settings, op).await,
        Resolved::Inputs { devices, default } => {
            execute_on_inputs(backend, snap, devices, default, settings, op).await
        }
        Resolved::Unavailable { .. } | Resolved::NotPlaying { .. } => Err(BackendError::NoTarget),
    }
}

async fn execute_on_device(
    backend: &dyn AudioBackend,
    snap: &Snapshot,
    kind: DeviceKind,
    node: &Node,
    volume: u16,
    settings: &AudioSettings,
    op: &Operation,
) -> Result<(), BackendError> {
    let max = effective_max(settings, snap.volume_cap);
    match op {
        Operation::None => Ok(()),
        Operation::ToggleMute => backend.set_mute(node, Mute::Toggle).await,
        Operation::SetMute(m) => {
            backend
                .set_mute(node, if *m { Mute::On } else { Mute::Off })
                .await
        }
        Operation::AdjustVolume(delta) => {
            backend
                .set_volume(node, adjust_volume(volume, *delta, max))
                .await
        }
        Operation::SetVolume(v) => backend.set_volume(node, (*v).min(max)).await,
        Operation::CycleDevice(dir) => {
            let current = snap.default_name(kind);
            let list = cycle_candidates(snap.devices(kind), &settings.cycle_devices);
            match step_in(&list, current, i64::from(*dir)) {
                Some(next) if next != current => backend.set_default(kind, &next).await,
                Some(_) => Ok(()),
                None => Err(BackendError::NoTarget),
            }
        }
        Operation::SetDefaultDevice(name) => {
            if !snap.devices(kind).iter().any(|d| &d.name == name) {
                return Err(BackendError::NoTarget);
            }
            backend.set_default(kind, name).await
        }
    }
}

async fn execute_on_app(
    backend: &dyn AudioBackend,
    snap: &Snapshot,
    streams: &[Stream],
    settings: &AudioSettings,
    op: &Operation,
) -> Result<(), BackendError> {
    let Some(first) = streams.first() else {
        return Err(BackendError::NoTarget);
    };
    let max = effective_max(settings, snap.volume_cap);
    match op {
        Operation::None => Ok(()),
        // Streams are set to one explicit state rather than each toggled,
        // so an app whose streams disagree ends up consistent.
        Operation::ToggleMute => set_all_mute(backend, streams, !first.muted).await,
        Operation::SetMute(m) => set_all_mute(backend, streams, *m).await,
        Operation::AdjustVolume(delta) => {
            set_all_volume(backend, streams, adjust_volume(first.volume, *delta, max)).await
        }
        Operation::SetVolume(v) => set_all_volume(backend, streams, (*v).min(max)).await,
        Operation::CycleDevice(dir) => {
            let current = snap
                .sink_by_index(first.sink)
                .map(|d| d.name.as_str())
                .unwrap_or_default();
            let list = cycle_candidates(&snap.sinks, &settings.cycle_devices);
            match step_in(&list, current, i64::from(*dir)) {
                Some(next) if next != current => move_all(backend, streams, &next).await,
                Some(_) => Ok(()),
                None => Err(BackendError::NoTarget),
            }
        }
        Operation::SetDefaultDevice(name) => {
            if !snap.sinks.iter().any(|d| &d.name == name) {
                return Err(BackendError::NoTarget);
            }
            move_all(backend, streams, name).await
        }
    }
}

/// Mute acts on every input; volume and device switching act on the
/// default one, exactly like a "Default input" target.
async fn execute_on_inputs(
    backend: &dyn AudioBackend,
    snap: &Snapshot,
    devices: &[Device],
    default: &Device,
    settings: &AudioSettings,
    op: &Operation,
) -> Result<(), BackendError> {
    match op {
        // One explicit state for all, never a per-device flip: a mixed
        // state mutes everything (only "all muted" unmutes).
        Operation::ToggleMute => set_devices_mute(backend, devices, !all_muted(devices)).await,
        Operation::SetMute(m) => set_devices_mute(backend, devices, *m).await,
        _ => {
            let node = Node::Source(default.name.clone());
            execute_on_device(
                backend,
                snap,
                DeviceKind::Input,
                &node,
                default.volume,
                settings,
                op,
            )
            .await
        }
    }
}

/// What `op` leaves a level at, given its current `volume` and `muted`;
/// `None` for operations that switch devices instead. Mirrors the
/// arithmetic `execute` sends to the backend.
fn predicted(
    volume: u16,
    muted: bool,
    settings: &AudioSettings,
    cap: Option<u16>,
    op: &Operation,
) -> Option<(u16, bool)> {
    let max = effective_max(settings, cap);
    match op {
        Operation::None => Some((volume, muted)),
        Operation::ToggleMute => Some((volume, !muted)),
        Operation::SetMute(m) => Some((volume, *m)),
        Operation::AdjustVolume(delta) => Some((adjust_volume(volume, *delta, max), muted)),
        Operation::SetVolume(v) => Some(((*v).min(max), muted)),
        Operation::CycleDevice(_) | Operation::SetDefaultDevice(_) => None,
    }
}

/// Updates `snap` to what a successful `execute` of `op` on `resolved` just
/// did, so the next dial tick builds on it without re-reading pactl.
/// Returns false when it can't tell (device switching): refresh instead.
pub fn apply_locally(
    snap: &mut Snapshot,
    resolved: &Resolved,
    settings: &AudioSettings,
    op: &Operation,
) -> bool {
    match resolved {
        Resolved::Device { kind, device } => {
            let Some((volume, muted)) =
                predicted(device.volume, device.muted, settings, snap.volume_cap, op)
            else {
                return false;
            };
            let list = match kind {
                DeviceKind::Output => &mut snap.sinks,
                DeviceKind::Input => &mut snap.sources,
            };
            let Some(d) = list.iter_mut().find(|d| d.name == device.name) else {
                return false;
            };
            (d.volume, d.muted) = (volume, muted);
            true
        }
        Resolved::App { streams } => {
            let Some(first) = streams.first() else {
                return true;
            };
            let Some((volume, muted)) =
                predicted(first.volume, first.muted, settings, snap.volume_cap, op)
            else {
                return false;
            };
            for s in snap
                .streams
                .iter_mut()
                .filter(|s| streams.iter().any(|x| x.index == s.index))
            {
                (s.volume, s.muted) = (volume, muted);
            }
            true
        }
        Resolved::Inputs { devices, default } => {
            let muted = match op {
                Operation::ToggleMute => !all_muted(devices),
                Operation::SetMute(m) => *m,
                _ => {
                    let as_device = Resolved::Device {
                        kind: DeviceKind::Input,
                        device: default.clone(),
                    };
                    return apply_locally(snap, &as_device, settings, op);
                }
            };
            for d in snap
                .sources
                .iter_mut()
                .filter(|d| devices.iter().any(|x| x.name == d.name))
            {
                d.muted = muted;
            }
            true
        }
        Resolved::Unavailable { .. } | Resolved::NotPlaying { .. } => true,
    }
}

async fn set_all_mute(
    backend: &dyn AudioBackend,
    streams: &[Stream],
    muted: bool,
) -> Result<(), BackendError> {
    let mute = if muted { Mute::On } else { Mute::Off };
    for s in streams {
        backend.set_mute(&Node::SinkInput(s.index), mute).await?;
    }
    Ok(())
}

/// Stops at the first failure, like the stream loops: the caller alerts
/// and re-reads, so a partly muted state shows as such.
async fn set_devices_mute(
    backend: &dyn AudioBackend,
    devices: &[Device],
    muted: bool,
) -> Result<(), BackendError> {
    let mute = if muted { Mute::On } else { Mute::Off };
    for d in devices {
        backend
            .set_mute(&Node::Source(d.name.clone()), mute)
            .await?;
    }
    Ok(())
}

async fn set_all_volume(
    backend: &dyn AudioBackend,
    streams: &[Stream],
    percent: u16,
) -> Result<(), BackendError> {
    for s in streams {
        backend
            .set_volume(&Node::SinkInput(s.index), percent)
            .await?;
    }
    Ok(())
}

async fn move_all(
    backend: &dyn AudioBackend,
    streams: &[Stream],
    sink: &str,
) -> Result<(), BackendError> {
    for s in streams {
        backend.move_stream(s.index, sink).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::FakeBackend;
    use crate::audio::model::test_support::fixture_snapshot;
    use crate::audio::target::{TargetKind, resolve};

    const RAZER: &str = "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo";
    const HDMI: &str = "alsa_output.pci-0000_2f_00.4.iec958-stereo";

    async fn run(
        kind: TargetKind,
        name: &str,
        settings: AudioSettings,
        op: Operation,
    ) -> (Result<(), BackendError>, Vec<String>) {
        let snap = fixture_snapshot();
        let fake = FakeBackend::new(snap.clone());
        let resolved = resolve(kind, name, &snap);
        let result = execute(&fake, &snap, &resolved, &settings, &op).await;
        (result, fake.calls())
    }

    #[tokio::test]
    async fn device_volume_adjusts_from_current_level() {
        let (r, calls) = run(
            TargetKind::DefaultOutput,
            "",
            AudioSettings::default(),
            Operation::AdjustVolume(10),
        )
        .await;
        r.unwrap();
        assert_eq!(calls, vec![format!("volume Sink({RAZER:?}) 57")]);
    }

    /// CoreAudio has no boost: a +10 step from 95 % with max_volume 150
    /// must send and predict 100, or the shown level drifts past the real
    /// one and stays there.
    #[tokio::test]
    async fn the_platform_volume_cap_beats_max_volume() {
        let mut snap = fixture_snapshot();
        snap.volume_cap = Some(100);
        for d in &mut snap.sinks {
            d.volume = 95;
        }
        let settings = AudioSettings {
            max_volume: 150,
            ..AudioSettings::default()
        };
        let fake = FakeBackend::new(snap.clone());
        let resolved = resolve(TargetKind::DefaultOutput, "", &snap);
        let op = Operation::AdjustVolume(10);
        execute(&fake, &snap, &resolved, &settings, &op)
            .await
            .unwrap();
        assert_eq!(fake.calls(), vec![format!(r#"volume Sink("{RAZER}") 100"#)]);
        assert!(apply_locally(&mut snap, &resolved, &settings, &op));
        let razer = snap.sinks.iter().find(|d| d.name == RAZER).unwrap();
        assert_eq!(razer.volume, 100);
        let set = Operation::SetVolume(140);
        assert!(apply_locally(&mut snap, &resolved, &settings, &set));
        assert_eq!(
            snap.sinks.iter().find(|d| d.name == RAZER).unwrap().volume,
            100
        );
    }

    #[tokio::test]
    async fn set_volume_is_capped_at_max() {
        let (_, calls) = run(
            TargetKind::DefaultInput,
            "",
            AudioSettings::default(),
            Operation::SetVolume(140),
        )
        .await;
        assert_eq!(
            calls,
            vec![r#"volume Source("alsa_input.usb-webcam-02.mono-fallback") 100"#.to_string()]
        );
    }

    #[tokio::test]
    async fn toggle_mute_on_a_device() {
        let (_, calls) = run(
            TargetKind::DefaultOutput,
            "",
            AudioSettings::default(),
            Operation::ToggleMute,
        )
        .await;
        assert_eq!(calls, vec![format!("mute Sink({RAZER:?}) Toggle")]);
    }

    #[tokio::test]
    async fn cycle_device_moves_the_default_to_the_next_sink() {
        let (_, calls) = run(
            TargetKind::DefaultOutput,
            "",
            AudioSettings::default(),
            Operation::CycleDevice(1),
        )
        .await;
        assert_eq!(calls, vec![format!("default Output {HDMI}")]);
    }

    #[tokio::test]
    async fn cycling_a_single_device_list_does_nothing() {
        let settings = AudioSettings {
            cycle_devices: vec![RAZER.to_string()],
            ..AudioSettings::default()
        };
        let (r, calls) = run(
            TargetKind::DefaultOutput,
            "",
            settings,
            Operation::CycleDevice(1),
        )
        .await;
        r.unwrap();
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn set_default_device_must_exist_in_that_kind() {
        let (r, calls) = run(
            TargetKind::DefaultOutput,
            "",
            AudioSettings::default(),
            Operation::SetDefaultDevice("alsa_input.usb-webcam-02.mono-fallback".into()),
        )
        .await;
        assert!(matches!(r, Err(BackendError::NoTarget)));
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn app_mute_sets_every_stream_to_the_same_state() {
        let (_, calls) = run(
            TargetKind::App,
            "chrome",
            AudioSettings::default(),
            Operation::ToggleMute,
        )
        .await;
        assert_eq!(
            calls,
            vec![
                "mute SinkInput(100) On".to_string(),
                "mute SinkInput(101) On".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn app_volume_follows_the_first_stream() {
        let (_, calls) = run(
            TargetKind::App,
            "chrome",
            AudioSettings::default(),
            Operation::AdjustVolume(-10),
        )
        .await;
        assert_eq!(
            calls,
            vec![
                "volume SinkInput(100) 90".to_string(),
                "volume SinkInput(101) 90".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn app_cycle_moves_its_streams_to_the_next_sink() {
        let (_, calls) = run(
            TargetKind::App,
            "chrome",
            AudioSettings::default(),
            Operation::CycleDevice(1),
        )
        .await;
        assert_eq!(
            calls,
            vec![format!("move 100 {HDMI}"), format!("move 101 {HDMI}")]
        );
    }

    #[tokio::test]
    async fn unavailable_target_is_an_error_and_touches_nothing() {
        let (r, calls) = run(
            TargetKind::App,
            "firefox",
            AudioSettings::default(),
            Operation::ToggleMute,
        )
        .await;
        assert!(matches!(r, Err(BackendError::NoTarget)));
        assert!(calls.is_empty());
    }

    /// Partial application is not rolled back: the error is returned (the
    /// action alerts and re-reads the real state) and the calls made stay.
    #[tokio::test]
    async fn a_failure_mid_app_stops_and_reports() {
        let snap = fixture_snapshot();
        let fake = FakeBackend::new(snap.clone());
        fake.fail("mute", 1);
        let resolved = resolve(TargetKind::App, "chrome", &snap);
        let r = execute(
            &fake,
            &snap,
            &resolved,
            &AudioSettings::default(),
            &Operation::SetMute(true),
        )
        .await;
        assert!(matches!(r, Err(BackendError::Failed { .. })), "{r:?}");
        assert_eq!(
            fake.calls(),
            vec!["mute SinkInput(100) On", "mute SinkInput(101) On"]
        );
    }

    #[test]
    fn local_updates_mirror_what_was_sent() {
        let mut snap = fixture_snapshot();
        let resolved = resolve(TargetKind::DefaultOutput, "", &snap);
        let s = AudioSettings::default();
        assert!(apply_locally(
            &mut snap,
            &resolved,
            &s,
            &Operation::AdjustVolume(10)
        ));
        let razer = snap.sinks.iter().find(|d| d.name == RAZER).unwrap();
        assert_eq!((razer.volume, razer.muted), (57, false));
        let resolved = resolve(TargetKind::App, "chrome", &snap);
        assert!(apply_locally(
            &mut snap,
            &resolved,
            &s,
            &Operation::ToggleMute
        ));
        assert!(
            snap.streams
                .iter()
                .filter(|x| x.binary == "chrome")
                .all(|x| x.muted)
        );
        assert!(!apply_locally(
            &mut snap,
            &resolved,
            &s,
            &Operation::CycleDevice(1)
        ));
    }

    #[tokio::test]
    async fn none_is_a_no_op_even_without_a_target() {
        let (r, calls) = run(
            TargetKind::App,
            "firefox",
            AudioSettings::default(),
            Operation::None,
        )
        .await;
        r.unwrap();
        assert!(calls.is_empty());
    }

    const WEBCAM: &str = "alsa_input.usb-webcam-02.mono-fallback";
    const ANALOG: &str = "alsa_input.pci-analog-stereo";

    /// The fixture's webcam mic (muted, default) plus a live analog mic.
    fn two_inputs() -> Snapshot {
        let mut snap = fixture_snapshot();
        let mut other = snap.sources[0].clone();
        other.index = 71;
        other.name = ANALOG.into();
        other.muted = false;
        snap.sources.push(other);
        snap
    }

    async fn run_inputs(
        snap: &Snapshot,
        op: Operation,
    ) -> (Result<(), BackendError>, Vec<String>, Snapshot) {
        let fake = FakeBackend::new(snap.clone());
        let resolved = resolve(TargetKind::AllInputs, "", snap);
        let settings = AudioSettings::default();
        let result = execute(&fake, snap, &resolved, &settings, &op).await;
        let mut after = snap.clone();
        // Device switching can't be predicted (false): the caller re-reads.
        let predicted = result.is_ok() && apply_locally(&mut after, &resolved, &settings, &op);
        assert_eq!(
            predicted,
            result.is_ok() && !matches!(op, Operation::CycleDevice(_))
        );
        (result, fake.calls(), after)
    }

    #[tokio::test]
    async fn toggling_mixed_inputs_mutes_every_one() {
        let (r, calls, after) = run_inputs(&two_inputs(), Operation::ToggleMute).await;
        r.unwrap();
        assert_eq!(
            calls,
            vec![
                format!("mute Source({WEBCAM:?}) On"),
                format!("mute Source({ANALOG:?}) On"),
            ]
        );
        assert!(after.sources.iter().all(|d| d.muted));
    }

    #[tokio::test]
    async fn toggling_all_muted_inputs_unmutes_every_one() {
        let mut snap = two_inputs();
        snap.sources.iter_mut().for_each(|d| d.muted = true);
        let (_, calls, after) = run_inputs(&snap, Operation::ToggleMute).await;
        assert_eq!(
            calls,
            vec![
                format!("mute Source({WEBCAM:?}) Off"),
                format!("mute Source({ANALOG:?}) Off"),
            ]
        );
        assert!(after.sources.iter().all(|d| !d.muted));
    }

    #[tokio::test]
    async fn a_failure_on_one_input_stops_and_reports_it() {
        let snap = two_inputs();
        let fake = FakeBackend::new(snap.clone());
        fake.fail("mute", 1);
        let resolved = resolve(TargetKind::AllInputs, "", &snap);
        let r = execute(
            &fake,
            &snap,
            &resolved,
            &AudioSettings::default(),
            &Operation::SetMute(true),
        )
        .await;
        assert!(r.is_err());
        assert_eq!(fake.calls().len(), 2);
    }

    #[tokio::test]
    async fn volume_and_switching_act_on_the_default_input_only() {
        let (_, calls, after) = run_inputs(&two_inputs(), Operation::AdjustVolume(5)).await;
        let webcam = after.sources.iter().find(|d| d.name == WEBCAM).unwrap();
        assert_eq!(
            calls,
            vec![format!("volume Source({WEBCAM:?}) {}", webcam.volume)]
        );
        let analog = after.sources.iter().find(|d| d.name == ANALOG).unwrap();
        assert_eq!(analog.volume, two_inputs().sources[1].volume);
        let (_, calls, _) = run_inputs(&two_inputs(), Operation::CycleDevice(1)).await;
        assert_eq!(calls, vec![format!("default Input {ANALOG}")]);
    }
}
