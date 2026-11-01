use crate::controller::{Half, strip_half};
use crate::lenient::Fields;
use crate::render::profile::ProfileView;
use serde::{Deserialize, Deserializer, Serialize};

/// What a short tap on the dial's touch strip does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StripTap {
    /// Switch to the highlighted profile (like pressing the dial).
    #[default]
    Switch,
    /// Left half: previous profile, right half: next, switching at once.
    HalfStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapAction {
    CommitHighlight,
    /// Switch to the profile this many places from the current one.
    Step(i64),
    /// Switch to the first profile in the dial's list.
    Home,
}

/// A touch-strip tap at segment-relative `x`, or a long touch (`hold`).
pub fn tap_action(s: &ProfileSettings, x: u16, hold: bool) -> TapAction {
    match (hold, s.strip_tap) {
        (true, _) => TapAction::Home,
        (false, StripTap::Switch) => TapAction::CommitHighlight,
        (false, StripTap::HalfStep) => TapAction::Step(match strip_half(x) {
            Half::Left => -1,
            Half::Right => 1,
        }),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProfileSettings {
    /// Empty = the device this control is on.
    pub device: String,
    /// Key: profile to switch to.
    pub profile: String,
    /// Dial: profiles to cycle through, in order; empty = all.
    pub cycle: Vec<String>,
    /// Dial: what a short strip tap does.
    pub strip_tap: StripTap,
}

impl<'de> Deserialize<'de> for ProfileSettings {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let f = Fields::from_deserializer(d)?;
        let mut s = Self::default();
        f.read("device", &mut s.device);
        f.read("profile", &mut s.profile);
        f.read("cycle", &mut s.cycle);
        f.read("strip_tap", &mut s.strip_tap);
        Ok(s)
    }
}

pub fn target_device(s: &ProfileSettings, own: &str) -> String {
    let d = s.device.trim();
    if d.is_empty() {
        own.to_string()
    } else {
        d.to_string()
    }
}

pub fn key_view(active: Option<&str>, target: &str) -> ProfileView {
    let target = target.trim();
    let is_active = !target.is_empty() && active == Some(target);
    let hint = if target.is_empty() {
        "set a profile"
    } else if is_active {
        "active"
    } else {
        "press to switch"
    };
    ProfileView {
        active: active.map(str::to_string),
        shown: if target.is_empty() {
            "—".to_string()
        } else {
            target.to_string()
        },
        is_active,
        hint: hint.to_string(),
    }
}

pub fn dial_view(active: Option<&str>, highlighted: Option<&str>) -> ProfileView {
    let shown = highlighted.or(active).unwrap_or("—");
    let is_active = active == Some(shown);
    ProfileView {
        active: active.map(str::to_string),
        shown: shown.to_string(),
        is_active,
        hint: if is_active {
            "active"
        } else {
            "press to switch"
        }
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_defaults_to_own() {
        assert_eq!(target_device(&ProfileSettings::default(), "sd-1"), "sd-1");
        let s = ProfileSettings {
            device: " sd-2 ".into(),
            ..Default::default()
        };
        assert_eq!(target_device(&s, "sd-1"), "sd-2");
    }

    #[test]
    fn garbled_fields_keep_their_defaults() {
        let s: ProfileSettings = serde_json::from_value(serde_json::json!({
            "device": 5, "profile": "gaming", "cycle": "x"
        }))
        .unwrap();
        assert_eq!(
            s,
            ProfileSettings {
                profile: "gaming".into(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn key_view_states() {
        assert!(key_view(Some("gaming"), "gaming").is_active);
        assert_eq!(key_view(Some("Default"), "gaming").hint, "press to switch");
        assert_eq!(key_view(Some("Default"), " ").hint, "set a profile");
    }

    #[test]
    fn dial_view_shows_highlight_or_active() {
        let v = dial_view(Some("Default"), Some("gaming"));
        assert_eq!((v.shown.as_str(), v.is_active), ("gaming", false));
        let v = dial_view(Some("Default"), None);
        assert_eq!((v.shown.as_str(), v.is_active), ("Default", true));
        assert_eq!(dial_view(None, None).shown, "—");
    }

    #[test]
    fn strip_taps_commit_by_default_step_by_half_and_hold_goes_home() {
        let s = ProfileSettings::default();
        assert_eq!(tap_action(&s, 150, false), TapAction::CommitHighlight);
        assert_eq!(tap_action(&s, 150, true), TapAction::Home);
        let half = ProfileSettings {
            strip_tap: StripTap::HalfStep,
            ..ProfileSettings::default()
        };
        assert_eq!(tap_action(&half, 99, false), TapAction::Step(-1));
        assert_eq!(tap_action(&half, 100, false), TapAction::Step(1));
        assert_eq!(tap_action(&half, 10, true), TapAction::Home);
        let old: ProfileSettings =
            serde_json::from_value(serde_json::json!({ "cycle": ["a"] })).unwrap();
        assert_eq!(old.strip_tap, StripTap::Switch);
    }
}
