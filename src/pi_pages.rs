//! The property inspectors are hand-written HTML/JS that write the settings
//! the plugin reads. These tests pin the two sides together: the defaults
//! and bounds each page uses, its `<option>` values and the gesture
//! operations it offers must match the Rust types. Because settings are read
//! leniently, a mismatch would otherwise just quietly become a default.

use crate::audio::gesture::effective;
use crate::audio::settings::{self, AudioSettings, OpKind, RotateKind};
use crate::audio::target::TargetKind;
use crate::brightness::{self, BrightnessSettings, KeyOp};
use crate::controller::Controller;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::ops::RangeInclusive;

const AUDIO: &str = include_str!("../assets/propertyInspector/audio.html");
const BRIGHTNESS: &str = include_str!("../assets/propertyInspector/brightness.html");
const PROFILE: &str = include_str!("../assets/propertyInspector/profile.html");

/// The JSON inside `<script type="application/json" id="{id}">`.
fn page_json(html: &str, id: &str) -> Value {
    let open = format!(r#"<script type="application/json" id="{id}">"#);
    let start = html.find(&open).expect("json block") + open.len();
    let end = start + html[start..].find("</script>").unwrap();
    serde_json::from_str(&html[start..end]).unwrap()
}

/// The `value`s of `<select id="{id}">`'s options.
fn select_options(html: &str, id: &str) -> Vec<String> {
    let open = format!(r#"<select id="{id}">"#);
    let start = html.find(&open).expect("select") + open.len();
    let body = &html[start..start + html[start..].find("</select>").unwrap()];
    body.split(r#"value=""#)
        .skip(1)
        .map(|rest| rest[..rest.find('"').unwrap()].to_string())
        .collect()
}

fn bounds<T: Into<i64> + Copy>(range: &RangeInclusive<T>) -> Value {
    json!([(*range.start()).into(), (*range.end()).into()])
}

/// Every value parses strictly (not leniently) as `T`, and together they
/// name `expected` variants.
fn all_parse_as<T: DeserializeOwned + PartialEq + std::fmt::Debug>(
    values: &[String],
    expected: &[T],
) {
    let parsed: Vec<T> = values
        .iter()
        .map(|v| serde_json::from_value(json!(v)).unwrap_or_else(|e| panic!("{v}: {e}")))
        .collect();
    for e in expected {
        assert!(parsed.contains(e), "{e:?} is not offered: {values:?}");
    }
    assert_eq!(parsed.len(), expected.len(), "{values:?}");
}

// Adding a variant without offering it in the page fails to compile here.
fn every_op() -> Vec<OpKind> {
    let all = [
        OpKind::None,
        OpKind::ToggleMute,
        OpKind::VolumeUp,
        OpKind::VolumeDown,
        OpKind::SetVolume,
        OpKind::CycleDevice,
        OpKind::SetDefaultDevice,
        OpKind::PushToTalk,
    ];
    for op in all {
        match op {
            OpKind::None
            | OpKind::ToggleMute
            | OpKind::VolumeUp
            | OpKind::VolumeDown
            | OpKind::SetVolume
            | OpKind::CycleDevice
            | OpKind::SetDefaultDevice
            | OpKind::PushToTalk => {}
        }
    }
    all.to_vec()
}

#[test]
fn audio_defaults_and_bounds_match_the_plugin() {
    let d = page_json(AUDIO, "defaults");
    let rust = serde_json::to_value(AudioSettings::default()).unwrap();
    for (key, value) in d["settings"].as_object().unwrap() {
        assert_eq!(&rust[key], value, "default for {key}");
    }
    for key in rust.as_object().unwrap().keys() {
        let gesture = ["press", "long_press", "touch_tap"].contains(&key.as_str());
        assert!(
            gesture || d["settings"].get(key).is_some(),
            "the page has no default for {key}"
        );
    }
    assert_eq!(d["bounds"]["step"], bounds(&settings::STEP));
    assert_eq!(d["bounds"]["max_volume"], bounds(&settings::MAX_VOLUME));
    assert_eq!(
        d["bounds"]["gesture_volume"],
        bounds(&settings::GESTURE_VOLUME)
    );
}

#[test]
fn audio_gesture_defaults_mirror_effective() {
    let d = page_json(AUDIO, "defaults");
    for c in [Controller::Keypad, Controller::Encoder] {
        let e = effective(&AudioSettings::default(), c);
        let rust =
            json!({ "press": e.press, "long_press": e.long_press, "touch_tap": e.touch_tap });
        assert_eq!(d["gestures"][c.as_str()], rust, "{c:?}");
    }
}

#[test]
fn audio_options_name_real_variants() {
    let d = page_json(AUDIO, "defaults");
    let ops: Vec<String> = d["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o[0].as_str().unwrap().to_string())
        .collect();
    all_parse_as(&ops, &every_op());
    all_parse_as(
        &select_options(AUDIO, "target"),
        &[
            TargetKind::DefaultOutput,
            TargetKind::DefaultInput,
            TargetKind::Output,
            TargetKind::Input,
            TargetKind::App,
        ],
    );
    all_parse_as(
        &select_options(AUDIO, "rotate"),
        &[
            RotateKind::Volume,
            RotateKind::CycleDevice,
            RotateKind::None,
        ],
    );
    for v in select_options(AUDIO, "max_volume") {
        let v: u16 = v.parse().unwrap();
        assert!(settings::MAX_VOLUME.contains(&v), "{v}");
    }
}

#[test]
fn audio_page_reads_every_choice_the_plugin_sends() {
    let snap = crate::audio::model::Snapshot::default();
    let c = crate::audio::pi::choices(&snap, &AudioSettings::default(), Controller::Keypad, None);
    for key in c
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| *k != "event" && *k != "error")
    {
        assert!(
            AUDIO.contains(&format!("choices.{key}")) || AUDIO.contains(&format!("choices?.{key}")),
            "{key}"
        );
    }
    assert!(AUDIO.contains(r#""audioChoices""#));
}

#[test]
fn brightness_defaults_and_bounds_match_the_plugin() {
    let d = page_json(BRIGHTNESS, "defaults");
    assert_eq!(
        d["settings"],
        serde_json::to_value(BrightnessSettings::default()).unwrap()
    );
    assert_eq!(d["bounds"]["step"], bounds(&brightness::STEP));
    for key in ["value", "preset_a", "preset_b"] {
        assert_eq!(d["bounds"][key], bounds(&brightness::PERCENT), "{key}");
    }
    all_parse_as(
        &select_options(BRIGHTNESS, "key_op"),
        &[
            KeyOp::Set,
            KeyOp::Increase,
            KeyOp::Decrease,
            KeyOp::TogglePresets,
        ],
    );
}

#[test]
fn profile_page_writes_the_settings_keys_and_reads_the_choices() {
    let rust = serde_json::to_value(crate::profile::ProfileSettings::default()).unwrap();
    for key in rust.as_object().unwrap().keys() {
        assert!(PROFILE.contains(&format!("{key}: ")), "{key}");
    }
    for key in ["profiles", "devices"] {
        assert!(PROFILE.contains(&format!("payload.{key}")), "{key}");
    }
    assert!(PROFILE.contains(r#""profileChoices""#));
}

#[test]
fn every_page_loads_the_shared_script() {
    for page in [AUDIO, BRIGHTNESS, PROFILE] {
        assert!(page.contains(r#"<script src="pi.js"></script>"#));
    }
}
