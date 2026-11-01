//! What an Audio control shows for a resolved target.

use super::model::DeviceKind;
use super::settings::{AudioSettings, DEFAULT_ACCENT};
use super::target::{Resolved, TargetKind, all_muted};
use crate::render::icons::Icon;
use crate::render::level::{INACTIVE_COLOR, LevelView, MUTED_COLOR};
use crate::render::text::{sanitize_color, shorten};

const TITLE_CHARS: usize = 20;

fn title(settings: &AudioSettings, fallback: &str) -> String {
    let label = settings.label.trim();
    shorten(if label.is_empty() { fallback } else { label }, TITLE_CHARS)
}

fn initial(label: &str) -> char {
    label
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('?')
}

fn target_icon(settings: &AudioSettings, fallback_label: &str) -> Icon {
    match settings.target {
        TargetKind::DefaultInput | TargetKind::Input | TargetKind::AllInputs => Icon::Mic,
        TargetKind::App => Icon::Letter(initial(fallback_label)),
        TargetKind::DefaultOutput | TargetKind::Output => Icon::Speaker,
    }
}

fn level(
    settings: &AudioSettings,
    fallback: &str,
    volume: u16,
    muted: bool,
    icon: Icon,
) -> LevelView {
    let value_text = if muted {
        "muted".to_string()
    } else if settings.show_percent {
        format!("{volume}%")
    } else {
        String::new()
    };
    LevelView {
        title: title(settings, fallback),
        value_text,
        bar: (f64::from(volume) * 100.0 / f64::from(settings.max_volume())).clamp(0.0, 100.0),
        color: if muted {
            MUTED_COLOR.to_string()
        } else {
            sanitize_color(&settings.accent, DEFAULT_ACCENT)
        },
        icon,
        struck: muted,
    }
}

fn inactive(settings: &AudioSettings, label: &str, status: &str) -> LevelView {
    LevelView {
        title: title(settings, label),
        value_text: status.to_string(),
        bar: 0.0,
        color: INACTIVE_COLOR.to_string(),
        icon: target_icon(settings, label),
        struck: false,
    }
}

/// `recorders`: the apps recording from the target's input(s)
/// (`target::recorders_for`). While any records, the title names them in
/// place of the label, muted or not: the point is knowing who has the mic.
pub fn audio_view(
    resolved: &Resolved,
    settings: &AudioSettings,
    recorders: &[String],
) -> LevelView {
    let mut view = target_view(resolved, settings);
    match recorders {
        [] => {}
        [app] => view.title = shorten(&format!("● {app}"), TITLE_CHARS),
        apps => view.title = format!("● {} apps", apps.len()),
    }
    view
}

fn target_view(resolved: &Resolved, settings: &AudioSettings) -> LevelView {
    let label = resolved.label();
    match resolved {
        Resolved::Device { kind, device } => {
            let icon = match kind {
                DeviceKind::Output => Icon::Speaker,
                DeviceKind::Input => Icon::Mic,
            };
            level(settings, &label, device.volume, device.muted, icon)
        }
        Resolved::App { streams } => match streams.first() {
            Some(first) => level(
                settings,
                &label,
                first.volume,
                first.muted,
                Icon::Letter(initial(&label)),
            ),
            None => inactive(settings, &label, "not playing"),
        },
        Resolved::Inputs { devices, default } => {
            // Muted only when every mic is: a half-muted set must not
            // read as silence.
            let muted = all_muted(devices);
            let mut view = level(settings, &label, default.volume, muted, Icon::Mic);
            let live = devices.iter().filter(|d| !d.muted).count();
            if !muted && live < devices.len() {
                view.value_text = format!("{live}/{} live", devices.len());
            }
            view
        }
        Resolved::Unavailable { .. } => inactive(settings, &label, "unavailable"),
        Resolved::NotPlaying { .. } => inactive(settings, &label, "not playing"),
    }
}

/// Shown when no snapshot could be read at all (e.g. `pactl` missing).
pub fn error_view(settings: &AudioSettings, message: &str) -> LevelView {
    LevelView {
        title: shorten(message, TITLE_CHARS),
        value_text: "—".to_string(),
        bar: 0.0,
        color: INACTIVE_COLOR.to_string(),
        icon: target_icon(settings, "?"),
        struck: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::test_support::fixture_snapshot;
    use crate::audio::target::resolve;

    fn view_for(settings: &AudioSettings) -> LevelView {
        audio_view(
            &resolve(settings.target, &settings.target_name, &fixture_snapshot()),
            settings,
            &[],
        )
    }

    #[test]
    fn device_shows_percent_in_accent() {
        let v = view_for(&AudioSettings::default());
        assert_eq!(v.value_text, "47%");
        assert_eq!(v.color, DEFAULT_ACCENT);
        assert_eq!(v.icon, Icon::Speaker);
        assert_eq!(v.title, "Razer Leviathan V2 …");
    }

    #[test]
    fn muted_mic_is_red_and_struck() {
        let v = view_for(&AudioSettings {
            target: TargetKind::DefaultInput,
            ..AudioSettings::default()
        });
        assert_eq!(v.value_text, "muted");
        assert_eq!(v.color, MUTED_COLOR);
        assert!(v.struck);
        assert_eq!(v.icon, Icon::Mic);
    }

    #[test]
    fn app_uses_its_initial_and_label_override() {
        let v = view_for(&AudioSettings {
            target: TargetKind::App,
            target_name: "chrome".into(),
            label: "Browser".into(),
            ..AudioSettings::default()
        });
        assert_eq!(v.icon, Icon::Letter('G'));
        assert_eq!(v.title, "Browser");
        assert_eq!(v.value_text, "100%");
    }

    #[test]
    fn not_playing_and_unavailable_are_grey() {
        let v = view_for(&AudioSettings {
            target: TargetKind::App,
            target_name: "firefox".into(),
            ..AudioSettings::default()
        });
        assert_eq!(
            (v.value_text.as_str(), v.color.as_str()),
            ("not playing", INACTIVE_COLOR)
        );
        let v = view_for(&AudioSettings {
            target: TargetKind::Output,
            target_name: "gone".into(),
            ..AudioSettings::default()
        });
        assert_eq!(v.value_text, "unavailable");
    }

    #[test]
    fn bar_is_relative_to_max_volume_and_percent_can_be_hidden() {
        let v = view_for(&AudioSettings {
            max_volume: 150,
            show_percent: false,
            ..AudioSettings::default()
        });
        assert!((v.bar - 47.0 * 100.0 / 150.0).abs() < 1e-9);
        assert_eq!(v.value_text, "");
    }

    #[test]
    fn bad_accent_falls_back() {
        let v = view_for(&AudioSettings {
            accent: "javascript:x".into(),
            ..AudioSettings::default()
        });
        assert_eq!(v.color, DEFAULT_ACCENT);
    }

    #[test]
    fn error_view_names_the_problem() {
        let v = error_view(&AudioSettings::default(), "pactl not found");
        assert_eq!(
            (v.title.as_str(), v.value_text.as_str()),
            ("pactl not found", "—")
        );
    }

    fn all_inputs_view(muted: [bool; 2]) -> LevelView {
        let mut snap = fixture_snapshot();
        let mut other = snap.sources[0].clone();
        other.name = "alsa_input.pci-analog-stereo".into();
        snap.sources.push(other);
        for (d, m) in snap.sources.iter_mut().zip(muted) {
            d.muted = m;
        }
        let settings = AudioSettings {
            target: TargetKind::AllInputs,
            ..AudioSettings::default()
        };
        audio_view(&resolve(settings.target, "", &snap), &settings, &[])
    }

    #[test]
    fn all_inputs_read_as_muted_only_when_every_one_is() {
        let v = all_inputs_view([true, true]);
        assert_eq!(
            (v.title.as_str(), v.value_text.as_str(), v.struck),
            ("All inputs", "muted", true)
        );
        assert_eq!(v.icon, Icon::Mic);
        let v = all_inputs_view([true, false]);
        assert_eq!((v.value_text.as_str(), v.struck), ("1/2 live", false));
        assert_eq!(v.color, DEFAULT_ACCENT);
        let v = all_inputs_view([false, false]);
        assert!(!v.struck);
        assert!(v.value_text.ends_with('%'), "{}", v.value_text);
    }

    fn mic_view(recorders: &[&str]) -> LevelView {
        let settings = AudioSettings {
            target: TargetKind::DefaultInput,
            label: "Mic".into(),
            ..AudioSettings::default()
        };
        let resolved = resolve(settings.target, "", &fixture_snapshot());
        let recorders: Vec<String> = recorders.iter().map(|s| s.to_string()).collect();
        audio_view(&resolved, &settings, &recorders)
    }

    #[test]
    fn a_recording_app_replaces_the_title_even_when_muted() {
        let v = mic_view(&["Firefox"]);
        assert_eq!(v.title, "● Firefox");
        assert_eq!(v.value_text, "muted", "the fixture mic is muted");
        assert!(v.struck);
        assert_eq!(mic_view(&["Firefox", "Zoom"]).title, "● 2 apps");
        assert_eq!(
            mic_view(&["A very long application name"]).title,
            shorten("● A very long application name", TITLE_CHARS)
        );
    }

    #[test]
    fn no_recorder_is_exactly_todays_view() {
        let settings = AudioSettings {
            target: TargetKind::DefaultInput,
            label: "Mic".into(),
            ..AudioSettings::default()
        };
        let resolved = resolve(settings.target, "", &fixture_snapshot());
        assert_eq!(mic_view(&[]), target_view(&resolved, &settings));
        assert_eq!(mic_view(&[]).title, "Mic");
    }

    /// The real wiring: a recorder on the speakers' monitor never reaches
    /// an output control (build_snapshot drops it, recorders_for only
    /// looks at inputs).
    #[test]
    fn an_output_shows_nothing_for_a_monitor_recorder() {
        let snap = crate::audio::model::build_snapshot(
            include_str!("fixtures/info.json"),
            include_str!("fixtures/sinks.json"),
            include_str!("fixtures/sources.json"),
            include_str!("fixtures/sink_inputs.json"),
            include_str!("fixtures/source_outputs.json"),
        )
        .unwrap();
        let settings = AudioSettings::default();
        let speakers = resolve(settings.target, "", &snap);
        let recorders = crate::audio::target::recorders_for(&speakers, &snap);
        assert!(recorders.is_empty());
        assert_eq!(
            audio_view(&speakers, &settings, &recorders).title,
            "Razer Leviathan V2 …"
        );
        let mic = resolve(TargetKind::DefaultInput, "", &snap);
        assert_eq!(
            crate::audio::target::recorders_for(&mic, &snap),
            vec!["Firefox", "arecord", "unknown"]
        );
    }
}
