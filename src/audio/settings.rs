use super::target::TargetKind;
use crate::lenient::Fields;
use serde::{Deserialize, Deserializer, Serialize};
use std::ops::RangeInclusive;

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

/// Volume step per dial tick or key press, in percent.
pub const STEP: RangeInclusive<u16> = 1..=20;
/// The highest volume the controls may set ("Maximum volume").
pub const MAX_VOLUME: RangeInclusive<u16> = 100..=150;
/// "Set volume to…" target.
pub const GESTURE_VOLUME: RangeInclusive<u16> = 0..=150;

/// One gesture's configured operation. `volume` is used by `SetVolume`,
/// `device` by `SetDefaultDevice`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GestureSetting {
    pub op: OpKind,
    pub volume: u16,
    pub device: String,
}

impl Default for GestureSetting {
    fn default() -> Self {
        Self {
            op: OpKind::None,
            volume: 50,
            device: String::new(),
        }
    }
}

impl<'de> Deserialize<'de> for GestureSetting {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let f = Fields::from_deserializer(d)?;
        let mut g = Self::default();
        f.read("op", &mut g.op);
        f.number("volume", GESTURE_VOLUME, &mut g.volume);
        f.read("device", &mut g.device);
        Ok(g)
    }
}

/// `None` gesture fields mean "use the controller's default" (see
/// `gesture::effective`) - the same settings behave sensibly on a key or a dial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioSettings {
    pub target: TargetKind,
    pub target_name: String,
    pub step: u16,
    pub max_volume: u16,
    pub rotate: RotateKind,
    pub press: Option<GestureSetting>,
    pub long_press: Option<GestureSetting>,
    pub touch_tap: Option<GestureSetting>,
    /// Device node names to cycle through; empty = all.
    pub cycle_devices: Vec<String>,
    pub label: String,
    pub accent: String,
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

impl<'de> Deserialize<'de> for AudioSettings {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let f = Fields::from_deserializer(d)?;
        let mut s = Self::default();
        f.read("target", &mut s.target);
        f.read("target_name", &mut s.target_name);
        f.number("step", STEP, &mut s.step);
        f.number("max_volume", MAX_VOLUME, &mut s.max_volume);
        f.read("rotate", &mut s.rotate);
        f.read("press", &mut s.press);
        f.read("long_press", &mut s.long_press);
        f.read("touch_tap", &mut s.touch_tap);
        f.read("cycle_devices", &mut s.cycle_devices);
        f.read("label", &mut s.label);
        f.read("accent", &mut s.accent);
        f.read("show_percent", &mut s.show_percent);
        Ok(s)
    }
}

impl AudioSettings {
    pub fn step(&self) -> u16 {
        self.step.clamp(*STEP.start(), *STEP.end())
    }

    pub fn max_volume(&self) -> u16 {
        self.max_volume
            .clamp(*MAX_VOLUME.start(), *MAX_VOLUME.end())
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
        assert_eq!(s.step, 5, "a garbled step keeps the documented default");
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
        assert!(s.show_percent, "a garbled show_percent keeps the default");
    }

    #[test]
    fn out_of_range_numbers_are_clamped_when_read() {
        let s: AudioSettings = serde_json::from_value(json!({
            "step": 99, "max_volume": 500,
            "press": { "op": "set_volume", "volume": 900 }
        }))
        .unwrap();
        assert_eq!((s.step, s.max_volume), (20, 150));
        assert_eq!(s.press.map(|g| g.volume), Some(150));
    }

    #[test]
    fn a_garbled_gesture_means_the_controller_default() {
        let s: AudioSettings = serde_json::from_value(json!({ "press": "x" })).unwrap();
        assert_eq!(s.press, None);
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
