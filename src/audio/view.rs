//! What an Audio control shows for a resolved target.

use super::model::DeviceKind;
use super::settings::{AudioSettings, DEFAULT_ACCENT};
use super::target::{Resolved, TargetKind};
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
        TargetKind::DefaultInput | TargetKind::Input => Icon::Mic,
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

pub fn audio_view(resolved: &Resolved, settings: &AudioSettings) -> LevelView {
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
}
