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
}
