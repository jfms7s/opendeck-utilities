use crate::lenient::lenient;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileSettings {
    /// Empty = the device this control is on.
    #[serde(deserialize_with = "lenient")]
    pub device: String,
    /// Key: profile to switch to.
    #[serde(deserialize_with = "lenient")]
    pub profile: String,
    /// Dial: profiles to cycle through, in order; empty = all.
    #[serde(deserialize_with = "lenient")]
    pub cycle: Vec<String>,
}

pub fn target_device(s: &ProfileSettings, own: &str) -> String {
    let d = s.device.trim();
    if d.is_empty() {
        own.to_string()
    } else {
        d.to_string()
    }
}

pub fn cycle_list(all: &[String], subset: &[String]) -> Vec<String> {
    if subset.is_empty() {
        all.to_vec()
    } else {
        subset.iter().filter(|p| all.contains(p)).cloned().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileView {
    pub active: Option<String>,
    pub shown: String,
    pub is_active: bool,
    pub hint: String,
}

pub fn key_view(active: Option<&str>, target: &str) -> ProfileView {
    let target = target.trim();
    let is_active = !target.is_empty() && active == Some(target);
    let hint = if target.is_empty() {
        "set a profile"
    } else if is_active {
        "active"
    } else {
        "press to switch"
    };
    ProfileView {
        active: active.map(str::to_string),
        shown: if target.is_empty() {
            "—".to_string()
        } else {
            target.to_string()
        },
        is_active,
        hint: hint.to_string(),
    }
}

pub fn dial_view(active: Option<&str>, highlighted: Option<&str>) -> ProfileView {
    let shown = highlighted.or(active).unwrap_or("—");
    let is_active = active == Some(shown);
    ProfileView {
        active: active.map(str::to_string),
        shown: shown.to_string(),
        is_active,
        hint: if is_active {
            "active"
        } else {
            "press to switch"
        }
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<String> {
        ["Default", "claude", "gaming"].map(String::from).to_vec()
    }

    #[test]
    fn device_defaults_to_own() {
        assert_eq!(target_device(&ProfileSettings::default(), "sd-1"), "sd-1");
        let s = ProfileSettings {
            device: " sd-2 ".into(),
            ..Default::default()
        };
        assert_eq!(target_device(&s, "sd-1"), "sd-2");
    }

    #[test]
    fn cycle_list_keeps_subset_order_and_drops_missing() {
        let subset = ["gaming", "deleted", "Default"].map(String::from).to_vec();
        assert_eq!(cycle_list(&all(), &subset), vec!["gaming", "Default"]);
        assert_eq!(cycle_list(&all(), &[]), all());
    }

    #[test]
    fn key_view_states() {
        assert!(key_view(Some("gaming"), "gaming").is_active);
        assert_eq!(key_view(Some("Default"), "gaming").hint, "press to switch");
        assert_eq!(key_view(Some("Default"), " ").hint, "set a profile");
    }

    #[test]
    fn dial_view_shows_highlight_or_active() {
        let v = dial_view(Some("Default"), Some("gaming"));
        assert_eq!((v.shown.as_str(), v.is_active), ("gaming", false));
        let v = dial_view(Some("Default"), None);
        assert_eq!((v.shown.as_str(), v.is_active), ("Default", true));
        assert_eq!(dial_view(None, None).shown, "—");
    }
}
