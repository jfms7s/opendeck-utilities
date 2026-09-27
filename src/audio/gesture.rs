//! Maps a physical gesture to an audio operation using the instance's
//! settings. Pure - timing is passed in, not measured here.

use super::settings::{AudioSettings, GestureSetting, OpKind, RotateKind};
use std::time::Duration;

pub const LONG_PRESS: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controller {
    Keypad,
    Encoder,
}

impl Controller {
    /// OpenDeck reports `"Keypad"` or `"Encoder"`.
    pub fn from_openaction(controller: &str) -> Self {
        if controller == "Keypad" {
            Controller::Keypad
        } else {
            Controller::Encoder
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    None,
    ToggleMute,
    SetMute(bool),
    AdjustVolume(i32),
    SetVolume(u16),
    CycleDevice(i32),
    SetDefaultDevice(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub press: GestureSetting,
    pub long_press: GestureSetting,
    pub touch_tap: GestureSetting,
}

fn op(op: OpKind) -> GestureSetting {
    GestureSetting {
        op,
        ..GestureSetting::default()
    }
}

/// Fills unset gestures with the controller's defaults (spec §6.2).
pub fn effective(settings: &AudioSettings, controller: Controller) -> Effective {
    let (press, long_press, touch_tap) = match controller {
        Controller::Keypad => (OpKind::ToggleMute, OpKind::None, OpKind::None),
        Controller::Encoder => (OpKind::ToggleMute, OpKind::CycleDevice, OpKind::ToggleMute),
    };
    Effective {
        press: settings.press.clone().unwrap_or_else(|| op(press)),
        long_press: settings
            .long_press
            .clone()
            .unwrap_or_else(|| op(long_press)),
        touch_tap: settings.touch_tap.clone().unwrap_or_else(|| op(touch_tap)),
    }
}

/// Push-to-talk is not a one-shot operation; it is handled by
/// `down_operation`/`release_operation` and maps to `None` here.
pub fn gesture_operation(gesture: &GestureSetting, step: u16) -> Operation {
    let step = i32::from(step);
    match gesture.op {
        OpKind::None | OpKind::PushToTalk => Operation::None,
        OpKind::ToggleMute => Operation::ToggleMute,
        OpKind::VolumeUp => Operation::AdjustVolume(step),
        OpKind::VolumeDown => Operation::AdjustVolume(-step),
        OpKind::SetVolume => Operation::SetVolume(gesture.volume),
        OpKind::CycleDevice => Operation::CycleDevice(1),
        OpKind::SetDefaultDevice => Operation::SetDefaultDevice(gesture.device.trim().to_string()),
    }
}

pub fn rotate_operation(kind: RotateKind, ticks: i16, step: u16) -> Operation {
    match kind {
        _ if ticks == 0 => Operation::None,
        RotateKind::Volume => Operation::AdjustVolume(i32::from(ticks) * i32::from(step)),
        RotateKind::CycleDevice => Operation::CycleDevice(i32::from(ticks.signum())),
        RotateKind::None => Operation::None,
    }
}

/// On key/dial down: only push-to-talk acts immediately (unmute).
pub fn down_operation(e: &Effective) -> Operation {
    if e.press.op == OpKind::PushToTalk {
        Operation::SetMute(false)
    } else {
        Operation::None
    }
}

/// On key/dial up. `held` is `None` when a turn consumed the press
/// (hold + turn), which fires nothing — except push-to-talk, which must
/// always mute again. A hold counts as a long-press only when a long-press
/// operation is configured; otherwise any hold is a plain press.
pub fn release_operation(e: &Effective, held: Option<Duration>, step: u16) -> Operation {
    if e.press.op == OpKind::PushToTalk {
        return Operation::SetMute(true);
    }
    let Some(held) = held else {
        return Operation::None;
    };
    if e.long_press.op != OpKind::None && held >= LONG_PRESS {
        gesture_operation(&e.long_press, step)
    } else {
        gesture_operation(&e.press, step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eff(controller: Controller) -> Effective {
        effective(&AudioSettings::default(), controller)
    }

    #[test]
    fn controller_defaults() {
        let k = eff(Controller::Keypad);
        assert_eq!(
            (k.press.op, k.long_press.op, k.touch_tap.op),
            (OpKind::ToggleMute, OpKind::None, OpKind::None)
        );
        let d = eff(Controller::Encoder);
        assert_eq!(
            (d.press.op, d.long_press.op, d.touch_tap.op),
            (OpKind::ToggleMute, OpKind::CycleDevice, OpKind::ToggleMute)
        );
    }

    #[test]
    fn configured_gestures_override_defaults() {
        let s = AudioSettings {
            long_press: Some(op(OpKind::None)),
            ..AudioSettings::default()
        };
        assert_eq!(
            effective(&s, Controller::Encoder).long_press.op,
            OpKind::None
        );
    }

    #[test]
    fn short_press_runs_press_op() {
        let e = eff(Controller::Encoder);
        assert_eq!(
            release_operation(&e, Some(Duration::from_millis(200)), 5),
            Operation::ToggleMute
        );
    }

    #[test]
    fn hold_at_threshold_runs_long_press_op() {
        let e = eff(Controller::Encoder);
        assert_eq!(
            release_operation(&e, Some(LONG_PRESS), 5),
            Operation::CycleDevice(1)
        );
    }

    #[test]
    fn hold_without_long_press_op_is_a_press() {
        let e = eff(Controller::Keypad);
        assert_eq!(
            release_operation(&e, Some(Duration::from_secs(3)), 5),
            Operation::ToggleMute
        );
    }

    #[test]
    fn push_to_talk_unmutes_on_down_and_mutes_on_up_even_when_held() {
        let s = AudioSettings {
            press: Some(op(OpKind::PushToTalk)),
            long_press: Some(op(OpKind::CycleDevice)),
            ..AudioSettings::default()
        };
        let e = effective(&s, Controller::Keypad);
        assert_eq!(down_operation(&e), Operation::SetMute(false));
        assert_eq!(
            release_operation(&e, Some(Duration::from_secs(2)), 5),
            Operation::SetMute(true)
        );
    }

    #[test]
    fn push_to_talk_mutes_on_up_even_after_hold_and_turn() {
        let s = AudioSettings {
            press: Some(op(OpKind::PushToTalk)),
            ..AudioSettings::default()
        };
        let e = effective(&s, Controller::Encoder);
        assert_eq!(release_operation(&e, None, 5), Operation::SetMute(true));
    }

    #[test]
    fn hold_and_turn_release_does_nothing() {
        assert_eq!(
            release_operation(&eff(Controller::Encoder), None, 5),
            Operation::None
        );
    }

    #[test]
    fn non_ptt_down_does_nothing() {
        assert_eq!(down_operation(&eff(Controller::Keypad)), Operation::None);
    }

    #[test]
    fn gesture_ops_carry_their_parameters() {
        let g = GestureSetting {
            op: OpKind::SetVolume,
            volume: 35,
            device: String::new(),
        };
        assert_eq!(gesture_operation(&g, 5), Operation::SetVolume(35));
        let g = GestureSetting {
            op: OpKind::SetDefaultDevice,
            volume: 0,
            device: " hdmi ".into(),
        };
        assert_eq!(
            gesture_operation(&g, 5),
            Operation::SetDefaultDevice("hdmi".into())
        );
        assert_eq!(
            gesture_operation(&op(OpKind::VolumeDown), 7),
            Operation::AdjustVolume(-7)
        );
    }

    #[test]
    fn rotation() {
        assert_eq!(
            rotate_operation(RotateKind::Volume, -3, 5),
            Operation::AdjustVolume(-15)
        );
        assert_eq!(
            rotate_operation(RotateKind::CycleDevice, 4, 5),
            Operation::CycleDevice(1)
        );
        assert_eq!(rotate_operation(RotateKind::Volume, 0, 5), Operation::None);
        assert_eq!(rotate_operation(RotateKind::None, 2, 5), Operation::None);
    }

    #[test]
    fn controller_from_openaction() {
        assert_eq!(Controller::from_openaction("Keypad"), Controller::Keypad);
        assert_eq!(Controller::from_openaction("Encoder"), Controller::Encoder);
    }
}
