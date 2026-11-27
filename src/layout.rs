//! Window Layout: the fixed table of KWin tile layouts, the key's settings,
//! the script a press runs inside KWin and what the key shows. Pure - KWin
//! is behind the port in `kwin/`.

use crate::lenient::Fields;
use crate::render::layout::{CYCLE_TITLE, LayoutView};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The script a press runs; `script` puts its `CONFIG` in front.
const SCRIPT: &str = include_str!("kwin/layout.js");

const THIRD: f64 = 1.0 / 3.0;

/// A set of tile columns, left to right. Stored in settings by `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Halves,
    Thirds,
    Wide,
}

impl Layout {
    /// Cycle order.
    pub const ALL: [Layout; 3] = [Layout::Halves, Layout::Thirds, Layout::Wide];

    pub fn id(self) -> &'static str {
        match self {
            Layout::Halves => "halves",
            Layout::Thirds => "thirds",
            Layout::Wide => "wide",
        }
    }

    /// The name on the key.
    pub fn label(self) -> &'static str {
        match self {
            Layout::Halves => "1/2",
            Layout::Thirds => "1/3",
            Layout::Wide => "25·50·25",
        }
    }

    /// Column widths, left to right, as fractions of the screen summing to 1.
    pub fn widths(self) -> &'static [f64] {
        match self {
            Layout::Halves => &[0.5, 0.5],
            Layout::Thirds => &[THIRD, THIRD, THIRD],
            Layout::Wide => &[0.25, 0.5, 0.25],
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }
}

/// What a press does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Go to the next layout in `Layout::ALL` (or restore a drifted one).
    Cycle,
    /// Always apply the chosen layout.
    Set,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Cycle, Mode::Set];

    pub fn id(self) -> &'static str {
        match self {
            Mode::Cycle => "cycle",
            Mode::Set => "set",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }
}

/// Serialises as its `id` and reads only an `id` from the table, so the
/// ids live in one place.
macro_rules! serde_by_id {
    ($ty:ty, $what:literal) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.id())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let id = String::deserialize(d)?;
                Self::from_id(&id)
                    .ok_or_else(|| serde::de::Error::custom(concat!("unknown ", $what)))
            }
        }
    };
}

serde_by_id!(Layout, "layout");
serde_by_id!(Mode, "mode");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LayoutSettings {
    /// Default: cycle.
    pub mode: Mode,
    /// Set mode: the layout a press applies. Default: wide.
    pub layout: Layout,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Cycle,
            layout: Layout::Wide,
        }
    }
}

impl<'de> Deserialize<'de> for LayoutSettings {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let f = Fields::from_deserializer(d)?;
        let mut s = Self::default();
        f.read("mode", &mut s.mode);
        f.read("layout", &mut s.layout);
        Ok(s)
    }
}

/// Widths this close to a layout's, column by column, are that layout
/// drifted (cycle mode restores it before moving on).
pub const TOLERANCE: f64 = 0.08;

/// The KWin script for a press: `const CONFIG = <json>;` then `kwin/layout.js`.
/// The JSON holds only ids and widths from the table, never settings text.
pub fn script(settings: &LayoutSettings) -> String {
    let layouts: Vec<_> = Layout::ALL
        .into_iter()
        .map(|l| serde_json::json!({ "id": l.id(), "widths": l.widths() }))
        .collect();
    let config = serde_json::json!({
        "mode": settings.mode.id(),
        "target": settings.layout.id(),
        "layouts": layouts,
        "tolerance": TOLERANCE,
    });
    format!("const CONFIG = {config};\n{SCRIPT}")
}

/// `supported`: this platform can run the script (Linux with KWin).
pub fn view(settings: &LayoutSettings, supported: bool) -> LayoutView {
    if !supported {
        return LayoutView {
            title: "Linux only".to_string(),
            columns: Vec::new(),
            hint: String::new(),
        };
    }
    match settings.mode {
        Mode::Set => LayoutView {
            title: settings.layout.label().to_string(),
            columns: settings.layout.widths().to_vec(),
            hint: "press to set".to_string(),
        },
        Mode::Cycle => LayoutView {
            title: CYCLE_TITLE.to_string(),
            columns: Vec::new(),
            hint: Layout::ALL
                .into_iter()
                .map(Layout::label)
                .collect::<Vec<_>>()
                .join(" → "),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::HashSet;

    fn settings(v: Value) -> LayoutSettings {
        serde_json::from_value(v).unwrap()
    }

    /// The `CONFIG` JSON at the top of `script`, and the rest of it.
    fn split_script(script: &str) -> (Value, &str) {
        let (first, rest) = script.split_once('\n').expect("a CONFIG line");
        let json = first
            .strip_prefix("const CONFIG = ")
            .and_then(|s| s.strip_suffix(';'))
            .expect("const CONFIG = <json>;");
        (serde_json::from_str(json).unwrap(), rest)
    }

    #[test]
    fn defaults_cycle_from_wide() {
        let d = LayoutSettings::default();
        assert_eq!((d.mode, d.layout), (Mode::Cycle, Layout::Wide));
        assert_eq!(settings(json!({})), d);
    }

    #[test]
    fn a_bad_field_keeps_its_default_and_good_ones_are_read() {
        let s = settings(json!({ "mode": "sideways", "layout": "thirds" }));
        assert_eq!((s.mode, s.layout), (Mode::Cycle, Layout::Thirds));
        let s = settings(json!({ "mode": "set", "layout": 5 }));
        assert_eq!((s.mode, s.layout), (Mode::Set, Layout::Wide));
        let s = settings(json!({ "mode": ["set"], "layout": "Halves" }));
        assert_eq!(s, LayoutSettings::default());
        assert!(serde_json::from_value::<LayoutSettings>(json!("set")).is_err());
    }

    #[test]
    fn settings_round_trip_through_their_ids() {
        for mode in Mode::ALL {
            for layout in Layout::ALL {
                let s = LayoutSettings { mode, layout };
                let v = serde_json::to_value(s).unwrap();
                assert_eq!(v, json!({ "mode": mode.id(), "layout": layout.id() }));
                assert_eq!(settings(v), s);
            }
        }
    }

    #[test]
    fn ids_parse_back_and_are_unique() {
        for layout in Layout::ALL {
            assert_eq!(Layout::from_id(layout.id()), Some(layout));
        }
        for mode in Mode::ALL {
            assert_eq!(Mode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(Layout::from_id("Wide"), None);
        let ids: HashSet<_> = Layout::ALL.iter().map(|l| l.id()).collect();
        assert_eq!(ids.len(), Layout::ALL.len());
        let labels: HashSet<_> = Layout::ALL.iter().map(|l| l.label()).collect();
        assert_eq!(labels.len(), Layout::ALL.len());
    }

    #[test]
    fn every_layout_has_two_or_three_columns_that_fill_the_screen() {
        for layout in Layout::ALL {
            let w = layout.widths();
            assert!((2..=3).contains(&w.len()), "{layout:?}");
            assert!(w.iter().all(|&c| c > 0.0), "{layout:?}");
            let sum: f64 = w.iter().sum();
            assert!((sum - 1.0).abs() < 1e-9, "{layout:?} sums to {sum}");
        }
    }

    #[test]
    fn the_table_is_the_one_asked_for() {
        assert_eq!(Layout::Halves.widths(), [0.5, 0.5]);
        assert_eq!(Layout::Wide.widths(), [0.25, 0.5, 0.25]);
        assert_eq!(Layout::Thirds.widths().len(), 3);
        let labels: Vec<_> = Layout::ALL.iter().map(|l| l.label()).collect();
        assert_eq!(labels, ["1/2", "1/3", "25·50·25"]);
    }

    #[test]
    fn script_is_a_config_line_then_the_file() {
        let s = LayoutSettings {
            mode: Mode::Set,
            layout: Layout::Thirds,
        };
        let out = script(&s);
        let (config, rest) = split_script(&out);
        assert_eq!(rest, include_str!("kwin/layout.js"));
        assert_eq!(config["mode"], "set");
        assert_eq!(config["target"], "thirds");
        assert_eq!(config["tolerance"], TOLERANCE);
        let layouts = config["layouts"].as_array().unwrap();
        let ids: Vec<_> = layouts.iter().map(|l| l["id"].as_str().unwrap()).collect();
        let table: Vec<_> = Layout::ALL.iter().map(|l| l.id()).collect();
        assert_eq!(ids, table);
        for (json, layout) in layouts.iter().zip(Layout::ALL) {
            assert_eq!(json["widths"], json!(layout.widths()));
        }
        let keys: Vec<_> = config.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 4, "{keys:?}");

        let (config, _) = split_script(&script(&LayoutSettings::default()));
        assert_eq!(
            (&config["mode"], &config["target"]),
            (&json!("cycle"), &json!("wide"))
        );
    }

    #[test]
    fn hostile_settings_never_reach_the_script() {
        let default = script(&LayoutSettings::default());
        for hostile in [
            r#"wide"};process.exit();//"#,
            "</script><script>alert(1)</script>",
            "`${process.exit()}`",
            r#"";process.exit()"#,
        ] {
            let s = settings(json!({ "mode": hostile, "layout": hostile }));
            let out = script(&s);
            assert_eq!(out, default);
            assert!(!out.contains(hostile), "{hostile}");
        }
    }

    #[test]
    fn set_mode_shows_the_chosen_layout() {
        for layout in Layout::ALL {
            let v = view(
                &LayoutSettings {
                    mode: Mode::Set,
                    layout,
                },
                true,
            );
            assert_eq!(v.title, layout.label());
            assert_eq!(v.columns, layout.widths());
            assert_eq!(v.hint, "press to set");
        }
    }

    #[test]
    fn cycle_mode_lists_the_layouts_in_order() {
        let v = view(&LayoutSettings::default(), true);
        assert_eq!(v.title, "Cycle");
        assert!(v.columns.is_empty());
        assert_eq!(v.hint, "1/2 → 1/3 → 25·50·25");
    }

    #[test]
    fn unsupported_says_linux_only() {
        for mode in Mode::ALL {
            let v = view(
                &LayoutSettings {
                    mode,
                    layout: Layout::Halves,
                },
                false,
            );
            assert_eq!(v.title, "Linux only");
            assert!(v.columns.is_empty());
            assert_eq!(v.hint, "");
        }
    }
}
