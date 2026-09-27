//! A value the plugin just asked OpenDeck for. Controls show it straight
//! away instead of waiting for OpenDeck's saved state to catch up; it gives
//! way once the saved state matches, or after `HOLD` if it never does.

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const HOLD: Duration = Duration::from_secs(2);

pub struct Pending<T> {
    slot: Mutex<Option<(T, Instant)>>,
}

impl<T: Clone + PartialEq> Pending<T> {
    pub fn new() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }

    pub fn set(&self, value: T) {
        self.set_at(value, Instant::now());
    }

    /// The pending value while it is fresh and not yet saved; else `actual`.
    pub fn resolve(&self, actual: Option<T>) -> Option<T> {
        self.resolve_at(actual, Instant::now())
    }

    fn set_at(&self, value: T, now: Instant) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((value, now));
    }

    fn resolve_at(&self, actual: Option<T>, now: Instant) -> Option<T> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match &*slot {
            Some((v, at)) if now.duration_since(*at) < HOLD && actual.as_ref() != Some(v) => {
                Some(v.clone())
            }
            _ => {
                *slot = None;
                actual
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_pending_shows_the_saved_value() {
        assert_eq!(Pending::<u8>::new().resolve(Some(50)), Some(50));
    }

    #[test]
    fn a_fresh_request_wins_over_a_stale_saved_value() {
        let p = Pending::new();
        let t = Instant::now();
        p.set_at(60, t);
        assert_eq!(
            p.resolve_at(Some(50), t + Duration::from_millis(300)),
            Some(60)
        );
        assert_eq!(p.resolve_at(None, t), Some(60));
    }

    #[test]
    fn it_gives_way_once_the_saved_value_matches() {
        let p = Pending::new();
        let t = Instant::now();
        p.set_at(60, t);
        assert_eq!(p.resolve_at(Some(60), t), Some(60));
        // Cleared: a later outside change shows even within the hold.
        assert_eq!(p.resolve_at(Some(30), t), Some(30));
    }

    #[test]
    fn it_expires_after_the_hold() {
        let p = Pending::new();
        let t = Instant::now();
        p.set_at(60, t);
        assert_eq!(p.resolve_at(Some(50), t + HOLD), Some(50));
    }
}
