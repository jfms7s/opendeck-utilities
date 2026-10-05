//! The three `openaction` actions. Each `Action` impl is a thin adapter: it
//! turns the `Instance` into a `Control` and calls the action's own methods,
//! which reach the deck only through `crate::ui::Ui` and OpenDeck only
//! through `crate::host::Host` - so the gesture handling runs under test.

pub mod audio;
pub mod brightness;
pub mod profile;

use crate::controller::Controller;
use dashmap::DashMap;
use openaction::Instance;

/// What the action logic needs to know about an instance. Unlike
/// `openaction::Instance`, tests can build one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub id: String,
    pub device: String,
    pub controller: Controller,
}

impl Control {
    pub fn of(instance: &Instance) -> Self {
        Self {
            id: instance.instance_id.clone(),
            device: instance.device_id.clone(),
            controller: Controller::from_openaction(&instance.controller),
        }
    }

    #[cfg(test)]
    pub fn key(id: &str) -> Self {
        Self {
            id: id.to_string(),
            device: "sd-1".to_string(),
            controller: Controller::Keypad,
        }
    }

    #[cfg(test)]
    pub fn dial(id: &str) -> Self {
        Self {
            controller: Controller::Encoder,
            ..Self::key(id)
        }
    }
}

/// The visible instances of one action and their latest settings.
pub struct Instances<S> {
    map: DashMap<String, (Control, S)>,
}

impl<S: Clone> Instances<S> {
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub fn insert(&self, control: &Control, settings: &S) {
        self.map
            .insert(control.id.clone(), (control.clone(), settings.clone()));
    }

    pub fn remove(&self, id: &str) {
        self.map.remove(id);
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// A copy, so no `DashMap` shard lock is held across an `await`.
    pub fn snapshot(&self) -> Vec<(Control, S)> {
        self.map.iter().map(|e| e.value().clone()).collect()
    }
}

/// A property inspector asks for its choices once it has registered, in
/// case the push from `property_inspector_did_appear` arrived before it
/// was listening.
pub fn is_choices_request(payload: &serde_json::Value) -> bool {
    payload["event"] == "requestChoices"
}

#[cfg(test)]
mod tests {
    use super::*;
    use openaction::Action;
    use serde_json::json;

    #[test]
    fn recognises_a_choices_request() {
        assert!(is_choices_request(&json!({"event": "requestChoices"})));
        assert!(!is_choices_request(&json!({"event": "other"})));
        assert!(!is_choices_request(&json!({})));
        assert!(!is_choices_request(&json!("requestChoices")));
    }

    #[test]
    fn the_manifest_lists_exactly_the_registered_actions() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/manifest.json")).unwrap();
        let mut in_manifest: Vec<&str> = manifest["Actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["UUID"].as_str().unwrap())
            .collect();
        in_manifest.sort();
        let mut registered = vec![
            <audio::AudioAction as Action>::UUID,
            <brightness::BrightnessAction as Action>::UUID,
            <profile::ProfileAction as Action>::UUID,
        ];
        registered.sort();
        assert_eq!(in_manifest, registered);
    }

    #[test]
    fn the_registry_snapshot_holds_the_latest_settings() {
        let r = Instances::new();
        r.insert(&Control::key("a"), &1);
        r.insert(&Control::key("a"), &2);
        r.insert(&Control::dial("b"), &3);
        let mut snap = r.snapshot();
        snap.sort_by(|x, y| x.0.id.cmp(&y.0.id));
        assert_eq!(snap.iter().map(|(_, s)| *s).collect::<Vec<_>>(), vec![2, 3]);
        r.remove("a");
        r.remove("b");
        assert!(r.is_empty());
    }
}
