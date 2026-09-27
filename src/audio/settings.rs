use super::target::TargetKind;
use crate::lenient::lenient;
use serde::{Deserialize, Serialize};

pub const DEFAULT_ACCENT: &str = "#4fc3f7";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    #[default]
    None,
    ToggleMute,
    VolumeUp,
    VolumeDown,
    SetVolume,
    CycleDevice,
    SetDefaultDevice,
    PushToTalk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotateKind {
    #[default]
    Volume,
    CycleDevice,
    None,
}

/// One gesture's configured operation. `volume` is used by `SetVolume`,
/// `device` by `SetDefaultDevice`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GestureSetting {
    #[serde(deserialize_with = "lenient")]
    pub op: OpKind,
    #[serde(deserialize_with = "lenient")]
    pub volume: u16,
    #[serde(deserialize_with = "lenient")]
    pub device: String,
}

/// `None` gesture fields mean "use the controller's default" (see
/// `gesture::effective`) - the same settings behave sensibly on a key or a dial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    #[serde(deserialize_with = "lenient")]
    pub target: TargetKind,
    #[serde(deserialize_with = "lenient")]
    pub target_name: String,
    #[serde(deserialize_with = "lenient")]
    pub step: u16,
    #[serde(deserialize_with = "lenient")]
    pub max_volume: u16,
    #[serde(deserialize_with = "lenient")]
    pub rotate: RotateKind,
    #[serde(deserialize_with = "lenient")]
    pub press: Option<GestureSetting>,
    #[serde(deserialize_with = "lenient")]
    pub long_press: Option<GestureSetting>,
    #[serde(deserialize_with = "lenient")]
    pub touch_tap: Option<GestureSetting>,
    /// Device node names to cycle through; empty = all.
    #[serde(deserialize_with = "lenient")]
    pub cycle_devices: Vec<String>,
    #[serde(deserialize_with = "lenient")]
    pub label: String,
    #[serde(deserialize_with = "lenient")]
    pub accent: String,
    #[serde(deserialize_with = "lenient")]
    pub show_percent: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            target: TargetKind::DefaultOutput,
            target_name: String::new(),
            step: 5,
            max_volume: 100,
            rotate: RotateKind::Volume,
            press: None,
            long_press: None,
            touch_tap: None,
            cycle_devices: Vec::new(),
            label: String::new(),
            accent: DEFAULT_ACCENT.to_string(),
            show_percent: true,
        }
    }
}

impl AudioSettings {
    pub fn step(&self) -> u16 {
        self.step.clamp(1, 20)
    }

    pub fn max_volume(&self) -> u16 {
        self.max_volume.clamp(100, 150)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_object_is_the_default() {
        let s: AudioSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(s, AudioSettings::default());
    }

    #[test]
    fn round_trips() {
        let s = AudioSettings {
            target: TargetKind::App,
            target_name: "chrome".into(),
            press: Some(GestureSetting {
                op: OpKind::SetVolume,
                volume: 30,
                device: String::new(),
            }),
            ..AudioSettings::default()
        };
        let back: AudioSettings =
            serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn garbled_settings_fall_back_to_defaults() {
        let s: AudioSettings = serde_json::from_value(json!({
            "target": "bogus",
            "step": "x",
            "max_volume": -3,
            "press": { "op": "explode", "volume": 20 },
            "cycle_devices": "not a list",
            "show_percent": "yes"
        }))
        .unwrap();
        assert_eq!(s.target, TargetKind::DefaultOutput);
        assert_eq!(s.step(), 1);
        assert_eq!(s.max_volume(), 100);
        assert_eq!(
            s.press,
            Some(GestureSetting {
                op: OpKind::None,
                volume: 20,
                device: String::new()
            })
        );
        assert!(s.cycle_devices.is_empty());
        assert!(!s.show_percent);
    }

    #[test]
    fn step_and_max_are_clamped() {
        let s = AudioSettings {
            step: 99,
            max_volume: 500,
            ..AudioSettings::default()
        };
        assert_eq!(s.step(), 20);
        assert_eq!(s.max_volume(), 150);
    }
}
