//! Pure brightness logic: which host request a gesture makes, what the
//! resulting value should be, and what the control shows.

use crate::host::BrightnessChange;
use crate::lenient::lenient;
use crate::render::icons::Icon;
use crate::render::level::{INACTIVE_COLOR, LevelView};
use serde::{Deserialize, Serialize};

pub const BRIGHTNESS_COLOR: &str = "#fbbf24";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyOp {
    Set,
    #[default]
    Increase,
    Decrease,
    TogglePresets,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrightnessSettings {
    #[serde(deserialize_with = "lenient")]
    pub step: u8,
    #[serde(deserialize_with = "lenient")]
    pub key_op: KeyOp,
    /// Target for `KeyOp::Set`.
    #[serde(deserialize_with = "lenient")]
    pub value: u8,
    #[serde(deserialize_with = "lenient")]
    pub preset_a: u8,
    #[serde(deserialize_with = "lenient")]
    pub preset_b: u8,
}

impl Default for BrightnessSettings {
    fn default() -> Self {
        Self {
            step: 5,
            key_op: KeyOp::Increase,
            value: 50,
            preset_a: 0,
            preset_b: 50,
        }
    }
}

impl BrightnessSettings {
    fn step(&self) -> u8 {
        self.step.clamp(1, 50)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub change: BrightnessChange,
    pub value: u8,
    /// What OpenDeck's stored brightness should become; `None` if unknown.
    pub expected: Option<u8>,
}

/// Mirrors OpenDeck's own handling (SettingsView.svelte).
pub fn apply(current: u8, change: BrightnessChange, value: u8) -> u8 {
    match change {
        BrightnessChange::Set => value.min(100),
        BrightnessChange::Increase => current.saturating_add(value).min(100),
        BrightnessChange::Decrease => current.saturating_sub(value),
    }
}

fn request(current: Option<u8>, change: BrightnessChange, value: u8) -> Request {
    Request {
        change,
        value,
        expected: current.map(|c| apply(c, change, value)),
    }
}

pub fn rotate_request(current: Option<u8>, ticks: i16, s: &BrightnessSettings) -> Option<Request> {
    if ticks == 0 {
        return None;
    }
    let amount = (u32::from(ticks.unsigned_abs()) * u32::from(s.step())).min(100) as u8;
    let change = if ticks > 0 {
        BrightnessChange::Increase
    } else {
        BrightnessChange::Decrease
    };
    Some(request(current, change, amount))
}

/// Goes to preset B when currently at preset A, otherwise to preset A.
pub fn toggle_request(current: Option<u8>, s: &BrightnessSettings) -> Request {
    let (a, b) = (s.preset_a.min(100), s.preset_b.min(100));
    let target = if current == Some(a) { b } else { a };
    Request {
        change: BrightnessChange::Set,
        value: target,
        expected: Some(target),
    }
}

pub fn key_request(current: Option<u8>, s: &BrightnessSettings) -> Request {
    match s.key_op {
        KeyOp::Set => Request {
            change: BrightnessChange::Set,
            value: s.value.min(100),
            expected: Some(s.value.min(100)),
        },
        KeyOp::Increase => request(current, BrightnessChange::Increase, s.step()),
        KeyOp::Decrease => request(current, BrightnessChange::Decrease, s.step()),
        KeyOp::TogglePresets => toggle_request(current, s),
    }
}

pub fn brightness_view(current: Option<u8>) -> LevelView {
    match current {
        Some(v) => LevelView {
            title: "Brightness".to_string(),
            value_text: format!("{v}%"),
            bar: f64::from(v),
            color: BRIGHTNESS_COLOR.to_string(),
            icon: Icon::Sun,
            struck: false,
        },
        None => LevelView {
            title: "Brightness".to_string(),
            value_text: "unknown".to_string(),
            bar: 0.0,
            color: INACTIVE_COLOR.to_string(),
            icon: Icon::Sun,
            struck: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> BrightnessSettings {
        BrightnessSettings::default()
    }

    #[test]
    fn rotation_scales_by_ticks_and_predicts_the_result() {
        let r = rotate_request(Some(50), 3, &s()).unwrap();
        assert_eq!(
            (r.change, r.value, r.expected),
            (BrightnessChange::Increase, 15, Some(65))
        );
        let r = rotate_request(Some(3), -1, &s()).unwrap();
        assert_eq!(
            (r.change, r.expected),
            (BrightnessChange::Decrease, Some(0))
        );
        assert_eq!(rotate_request(Some(3), 0, &s()), None);
    }

    #[test]
    fn unknown_current_has_no_expectation() {
        assert_eq!(rotate_request(None, 1, &s()).unwrap().expected, None);
    }

    #[test]
    fn increase_saturates_at_100() {
        assert_eq!(apply(98, BrightnessChange::Increase, 5), 100);
    }

    #[test]
    fn toggle_goes_between_presets() {
        assert_eq!(toggle_request(Some(0), &s()).value, 50);
        assert_eq!(toggle_request(Some(50), &s()).value, 0);
        assert_eq!(toggle_request(Some(73), &s()).value, 0);
        assert_eq!(toggle_request(None, &s()).value, 0);
    }

    #[test]
    fn key_ops() {
        let set = BrightnessSettings {
            key_op: KeyOp::Set,
            value: 120,
            ..s()
        };
        assert_eq!(key_request(Some(10), &set).expected, Some(100));
        let dec = BrightnessSettings {
            key_op: KeyOp::Decrease,
            ..s()
        };
        assert_eq!(key_request(Some(10), &dec).expected, Some(5));
    }

    #[test]
    fn garbled_settings_fall_back() {
        let parsed: BrightnessSettings =
            serde_json::from_value(serde_json::json!({"key_op": "melt", "step": "big"})).unwrap();
        assert_eq!(parsed.key_op, KeyOp::Increase);
        assert_eq!(parsed.step(), 1);
    }

    #[test]
    fn view() {
        assert_eq!(brightness_view(Some(40)).value_text, "40%");
        assert_eq!(brightness_view(None).color, INACTIVE_COLOR);
    }
}
