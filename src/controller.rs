//! Which kind of control an action instance sits on. Every action and the
//! renderer care about it, so it lives at the crate level rather than in
//! one feature.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Controller {
    Keypad,
    Encoder,
}

impl Controller {
    /// OpenDeck reports `"Keypad"` or `"Encoder"` in `Instance::controller`.
    pub fn from_openaction(controller: &str) -> Self {
        if controller == Self::Keypad.as_str() {
            Self::Keypad
        } else {
            Self::Encoder
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keypad => "Keypad",
            Self::Encoder => "Encoder",
        }
    }
}

/// Each dial owns a 200 px slice of the touch strip; OpenDeck reports a
/// tap's x relative to that slice (0-199, `elgato.rs` sends `x % 200`).
pub const STRIP_SEGMENT: u16 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    Left,
    Right,
}

/// Which half of its dial's strip segment a tap at `x` hit. The midpoint
/// and anything out of range count as the right half.
pub fn strip_half(x: u16) -> Half {
    if x < STRIP_SEGMENT / 2 {
        Half::Left
    } else {
        Half::Right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_openaction_names() {
        for c in [Controller::Keypad, Controller::Encoder] {
            assert_eq!(Controller::from_openaction(c.as_str()), c);
            assert_eq!(serde_json::to_value(c).unwrap(), c.as_str());
        }
    }

    #[test]
    fn strip_halves_split_at_the_midpoint() {
        assert_eq!(strip_half(0), Half::Left);
        assert_eq!(strip_half(99), Half::Left);
        assert_eq!(strip_half(100), Half::Right, "the midpoint is right");
        assert_eq!(strip_half(199), Half::Right);
        assert_eq!(strip_half(650), Half::Right, "out of range");
    }
}
