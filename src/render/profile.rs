use super::text::shorten;
use super::tile::{self, MUTED_TEXT_COLOR, TEXT_COLOR};
use crate::profile::ProfileView;
use serde_json::{Value, json};

pub const ACTIVE_COLOR: &str = "#22c55e";

pub fn strip_feedback(view: &ProfileView) -> Value {
    let active = view.active.as_deref().unwrap_or("unknown");
    json!({
        "active": format!("active: {}", shorten(active, 20)),
        "candidate": {
            "value": shorten(&view.shown, 16),
            "color": if view.is_active { ACTIVE_COLOR } else { TEXT_COLOR },
        },
        "hint": view.hint,
    })
}

pub fn tile_image(view: &ProfileView) -> String {
    let border = if view.is_active {
        format!(
            r#"<rect x="3" y="3" width="94" height="94" rx="10" fill="none" stroke="{ACTIVE_COLOR}" stroke-width="5"/>"#
        )
    } else {
        String::new()
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{card}{border}{name}{hint}</svg>"#,
        card = tile::card(),
        name = tile::text_line(56.0, 20.0, true, TEXT_COLOR, &shorten(&view.shown, 12)),
        hint = tile::text_line(80.0, 12.0, false, MUTED_TEXT_COLOR, &view.hint),
    );
    tile::data_uri(&svg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::key_view;

    #[test]
    fn feedback_keys_match_the_shipped_layout() {
        let layout: Value =
            serde_json::from_str(include_str!("../../assets/layouts/profile.json")).unwrap();
        let keys: Vec<&str> = layout["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap())
            .collect();
        for k in strip_feedback(&key_view(Some("a"), "b"))
            .as_object()
            .unwrap()
            .keys()
        {
            assert!(keys.contains(&k.as_str()), "layout has no item keyed {k}");
        }
    }

    #[test]
    fn active_tile_has_a_border_and_name_is_escaped() {
        let svg = tile::decode(&tile_image(&key_view(Some("<a>"), "<a>")));
        assert!(svg.contains(ACTIVE_COLOR));
        assert!(svg.contains("&lt;a&gt;"));
        let svg = tile::decode(&tile_image(&key_view(Some("x"), "y")));
        assert!(!svg.contains(ACTIVE_COLOR));
    }
}
