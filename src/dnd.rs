//! Do Not Disturb: what the key shows and what a press does, from the
//! desktop's notification inhibition and whether the plugin holds one.
//! Pure - the bus is behind `notifications::NotificationsPort`.

use crate::render::dnd::DndView;

/// The desktop's notification inhibition, as the notification server
/// reports it (any source: Plasma's own DND, another app, this plugin).
// Only the D-Bus adapter (Linux) reports a known state; macOS is always
// `Unsupported`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inhibition {
    /// This platform has no notification inhibition the plugin can use.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Unsupported,
    /// No notification server answered, or it has no `Inhibited` property.
    Unknown,
    Off,
    On,
}

/// What a press does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Start the plugin's own inhibition.
    Inhibit,
    /// End the plugin's own inhibition.
    UnInhibit,
    /// Nothing the plugin can do: alert.
    Refuse,
}

/// `ours`: the plugin holds an inhibition cookie.
pub fn view(inhibition: Inhibition, ours: bool) -> DndView {
    let (on, state, hint) = match inhibition {
        Inhibition::Unsupported => (false, "Linux only", ""),
        Inhibition::Unknown => (false, "Unavailable", "no notification server"),
        Inhibition::Off => (false, "Off", "press to silence"),
        Inhibition::On if ours => (true, "On", "press to resume"),
        // Someone else's inhibition: a press can't lift it (ruling 2).
        Inhibition::On => (true, "On", "by desktop"),
    };
    DndView {
        on,
        state: state.to_string(),
        hint: hint.to_string(),
    }
}

/// Inhibited and ours: end it. Not inhibited: start ours (a cookie left
/// from a notification server that restarted is stale and replaced).
/// Inhibited by someone else, or no server: refuse.
pub fn press(inhibition: Inhibition, ours: bool) -> Press {
    match inhibition {
        Inhibition::Off => Press::Inhibit,
        Inhibition::On if ours => Press::UnInhibit,
        Inhibition::On | Inhibition::Unknown | Inhibition::Unsupported => Press::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_inhibits() {
        assert_eq!(press(Inhibition::Off, false), Press::Inhibit);
        let v = view(Inhibition::Off, false);
        assert!(!v.on);
        assert_eq!(v.state, "Off");
    }

    #[test]
    fn ours_uninhibits() {
        assert_eq!(press(Inhibition::On, true), Press::UnInhibit);
        let v = view(Inhibition::On, true);
        assert!(v.on);
        assert_eq!(v.hint, "press to resume");
    }

    /// Ruling 2 (a): DND from elsewhere shows "On" and a press refuses.
    #[test]
    fn inhibited_by_someone_else_shows_on_and_refuses() {
        assert_eq!(press(Inhibition::On, false), Press::Refuse);
        let v = view(Inhibition::On, false);
        assert!(v.on);
        assert_eq!((v.state.as_str(), v.hint.as_str()), ("On", "by desktop"));
    }

    /// A cookie from before a notification server restart: the server says
    /// off, so the key says off and a press starts a fresh inhibition.
    #[test]
    fn a_stale_cookie_does_not_keep_the_key_on() {
        assert_eq!(press(Inhibition::Off, true), Press::Inhibit);
        assert!(!view(Inhibition::Off, true).on);
    }

    #[test]
    fn no_server_or_platform_refuses() {
        for i in [Inhibition::Unknown, Inhibition::Unsupported] {
            assert_eq!(press(i, false), Press::Refuse);
            assert!(!view(i, false).on);
        }
        assert_eq!(view(Inhibition::Unsupported, false).state, "Linux only");
        assert_eq!(view(Inhibition::Unknown, false).state, "Unavailable");
    }
}
