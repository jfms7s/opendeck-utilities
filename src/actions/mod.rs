pub mod audio;
pub mod brightness;
pub mod profile;

/// A property inspector asks for its choices once it has registered, in
/// case the push from `property_inspector_did_appear` arrived before it
/// was listening.
pub fn is_choices_request(payload: &serde_json::Value) -> bool {
    payload["event"] == "requestChoices"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recognises_a_choices_request() {
        assert!(is_choices_request(&json!({"event": "requestChoices"})));
        assert!(!is_choices_request(&json!({"event": "other"})));
        assert!(!is_choices_request(&json!({})));
        assert!(!is_choices_request(&json!("requestChoices")));
    }
}
