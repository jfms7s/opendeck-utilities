//! Carries out one `Operation` on a resolved target through a backend.

use super::backend::{AudioBackend, BackendError, Mute, Node};
use super::gesture::Operation;
use super::model::{DeviceKind, Snapshot, Stream};
use super::ops::{adjust_volume, cycle_candidates};
use super::settings::AudioSettings;
use super::target::Resolved;
use crate::cycle::step_in;

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
    let max = settings.max_volume();
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
    let max = settings.max_volume();
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
}
