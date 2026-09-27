//! One renderer for any "level" control (audio, brightness): a touch-strip
//! feedback payload for dials and a generated image for keys.

use super::icons::{Icon, icon_data_uri, icon_svg};
use super::text::sanitize_color;
use super::tile::{self, MUTED_TEXT_COLOR, TEXT_COLOR};
use serde_json::{Value, json};

pub const MUTED_COLOR: &str = "#ef4444";
pub const INACTIVE_COLOR: &str = "#6b7280";
const TRACK_COLOR: &str = "#374151";

#[derive(Debug, Clone, PartialEq)]
pub struct LevelView {
    pub title: String,
    pub value_text: String,
    /// 0-100, the fill of the bar.
    pub bar: f64,
    pub color: String,
    pub icon: Icon,
    pub struck: bool,
}

impl LevelView {
    fn safe_color(&self) -> String {
        sanitize_color(&self.color, INACTIVE_COLOR)
    }
}

pub fn strip_feedback(view: &LevelView) -> Value {
    let color = view.safe_color();
    json!({
        "icon": icon_data_uri(&view.icon, &color, view.struck),
        "title": view.title,
        "value": view.value_text,
        "bar": { "value": view.bar.clamp(0.0, 100.0), "bar_fill_c": color },
    })
}

pub fn tile_image(view: &LevelView) -> String {
    let color = view.safe_color();
    let bar_width = (view.bar.clamp(0.0, 100.0) * 0.8).round();
    // resvg rejects width="0", so an empty bar draws no fill at all.
    let fill = if bar_width > 0.0 {
        format!(r#"<rect x="10" y="90" width="{bar_width}" height="5" rx="2.5" fill="{color}"/>"#)
    } else {
        String::new()
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{card}<g transform="translate(32 4) scale(0.36)">{icon}</g>{value}{title}<rect x="10" y="90" width="80" height="5" rx="2.5" fill="{TRACK_COLOR}"/>{fill}</svg>"#,
        card = tile::card(),
        icon = icon_svg(&view.icon, &color, view.struck),
        value = tile::text_line(64.0, 24.0, true, TEXT_COLOR, &view.value_text),
        title = tile::text_line(82.0, 13.0, false, MUTED_TEXT_COLOR, &view.title),
    );
    tile::data_uri(&svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> LevelView {
        LevelView {
            title: "Razer".into(),
            value_text: "45%".into(),
            bar: 45.0,
            color: "#4fc3f7".into(),
            icon: Icon::Speaker,
            struck: false,
        }
    }

    #[test]
    fn feedback_keys_match_the_shipped_layout() {
        let layout: Value =
            serde_json::from_str(include_str!("../../assets/layouts/level.json")).unwrap();
        let keys: Vec<&str> = layout["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap())
            .collect();
        for k in strip_feedback(&view()).as_object().unwrap().keys() {
            assert!(keys.contains(&k.as_str()), "layout has no item keyed {k}");
        }
    }

    #[test]
    fn bar_is_clamped() {
        let v = LevelView {
            bar: 180.0,
            ..view()
        };
        assert_eq!(strip_feedback(&v)["bar"]["value"], 100.0);
    }

    #[test]
    fn an_unsafe_colour_never_reaches_the_output() {
        let v = LevelView {
            color: r#"x" onload="evil"#.into(),
            ..view()
        };
        assert!(!strip_feedback(&v).to_string().contains("evil"));
        assert!(!tile::decode(&tile_image(&v)).contains("evil"));
    }

    #[test]
    fn empty_bar_draws_no_zero_width_rect() {
        let mut v = view();
        v.bar = 0.0;
        assert!(!tile::decode(&tile_image(&v)).contains(r#"width="0""#));
    }

    #[test]
    fn tile_shows_value_and_title() {
        let svg = tile::decode(&tile_image(&view()));
        assert!(
            svg.contains(">45%</text>") && svg.contains(">Razer</text>"),
            "{svg}"
        );
    }
}
