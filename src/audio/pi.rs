//! The data the Audio property inspector needs to offer choices.

use super::gesture::{Controller, effective};
use super::model::{Device, Snapshot};
use super::settings::AudioSettings;
use serde_json::{Value, json};
use std::collections::HashSet;

fn devices(list: &[Device]) -> Vec<Value> {
    list.iter()
        .map(|d| json!({ "name": d.name, "description": d.description }))
        .collect()
}

/// `error`: why the device lists may be empty (the property inspector keeps
/// stored choices it can't see either way).
pub fn choices(
    snap: &Snapshot,
    settings: &AudioSettings,
    controller: Controller,
    error: Option<&str>,
) -> Value {
    let mut seen = HashSet::new();
    let apps: Vec<Value> = snap
        .streams
        .iter()
        .filter_map(|s| {
            let key = if s.binary.is_empty() {
                &s.app_name
            } else {
                &s.binary
            };
            (!key.is_empty() && seen.insert(key.to_lowercase()))
                .then(|| json!({ "match": key, "app_name": s.app_name }))
        })
        .collect();
    let e = effective(settings, controller);
    json!({
        "event": "audioChoices",
        "controller": controller,
        "outputs": devices(&snap.sinks),
        "inputs": devices(&snap.sources),
        "apps": apps,
        "appsSupported": super::APPS_SUPPORTED,
        "effective": { "press": e.press, "long_press": e.long_press, "touch_tap": e.touch_tap },
        "error": error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::test_support::fixture_snapshot;

    #[test]
    fn says_whether_per_app_audio_exists_here() {
        let c = choices(
            &fixture_snapshot(),
            &AudioSettings::default(),
            Controller::Keypad,
            None,
        );
        assert_eq!(c["appsSupported"], !cfg!(target_os = "macos"));
    }

    #[test]
    fn apps_are_deduplicated_and_nameless_streams_skipped() {
        let c = choices(
            &fixture_snapshot(),
            &AudioSettings::default(),
            Controller::Encoder,
            None,
        );
        let apps: Vec<&str> = c["apps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["match"].as_str().unwrap())
            .collect();
        assert_eq!(apps, vec!["chrome", "spotify"]);
    }

    #[test]
    fn lists_devices_without_monitors_and_reports_effective_gestures() {
        let c = choices(
            &fixture_snapshot(),
            &AudioSettings::default(),
            Controller::Encoder,
            None,
        );
        assert_eq!(c["outputs"].as_array().unwrap().len(), 2);
        assert_eq!(c["inputs"].as_array().unwrap().len(), 1);
        assert_eq!(c["effective"]["long_press"]["op"], "cycle_device");
        assert_eq!(c["controller"], "Encoder");
    }
}
