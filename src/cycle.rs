//! Stepping through an ordered list with wrap-around - shared by audio
//! device cycling and the profile dial.

/// Moves `steps` places from `current` (negative = backwards), wrapping.
/// If `current` isn't in the list, stepping forward starts at the first
/// entry and stepping backward at the last. `None` only for an empty list.
pub fn step_in(list: &[String], current: &str, steps: i64) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    let len = list.len() as i64;
    let base = match list.iter().position(|x| x == current) {
        Some(i) => i as i64,
        None if steps > 0 => -1,
        None => 0,
    };
    let next = (base + steps).rem_euclid(len);
    list.get(next as usize).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<String> {
        ["a", "b", "c"].map(String::from).to_vec()
    }

    #[test]
    fn steps_forward_and_wraps() {
        assert_eq!(step_in(&list(), "b", 1).as_deref(), Some("c"));
        assert_eq!(step_in(&list(), "c", 1).as_deref(), Some("a"));
        assert_eq!(step_in(&list(), "a", 4).as_deref(), Some("b"));
    }

    #[test]
    fn steps_backward_and_wraps() {
        assert_eq!(step_in(&list(), "a", -1).as_deref(), Some("c"));
        assert_eq!(step_in(&list(), "b", -5).as_deref(), Some("c"));
    }

    #[test]
    fn unknown_current_starts_at_an_end() {
        assert_eq!(step_in(&list(), "zzz", 1).as_deref(), Some("a"));
        assert_eq!(step_in(&list(), "zzz", -1).as_deref(), Some("c"));
    }

    #[test]
    fn empty_list_is_none() {
        assert_eq!(step_in(&[], "a", 1), None);
    }
}
