//! The Do Not Disturb key tile.

use super::tile::{self, MUTED_TEXT_COLOR, TEXT_COLOR};

/// What a Do Not Disturb key shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DndView {
    /// Notifications are inhibited (the key is lit).
    pub on: bool,
    /// "On", "Off", "Unavailable", "Linux only".
    pub state: String,
    /// The line under it: who holds it, or what a press does.
    pub hint: String,
}

pub const ON_COLOR: &str = "#f59e0b";

pub fn tile_image(view: &DndView) -> String {
    let (border, state_color) = if view.on {
        (
            format!(
                r#"<rect x="3" y="3" width="94" height="94" rx="10" fill="none" stroke="{ON_COLOR}" stroke-width="5"/>"#
            ),
            ON_COLOR,
        )
    } else {
        (String::new(), TEXT_COLOR)
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{card}{border}{title}{state}{hint}</svg>"#,
        card = tile::card(),
        title = tile::text_line(28.0, 13.0, false, MUTED_TEXT_COLOR, "Do Not Disturb"),
        state = tile::text_line(58.0, 22.0, true, state_color, &view.state),
        hint = tile::text_line(82.0, 11.0, false, MUTED_TEXT_COLOR, &view.hint),
    );
    tile::data_uri(&svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(on: bool, state: &str) -> DndView {
        DndView {
            on,
            state: state.into(),
            hint: "by <desktop>".into(),
        }
    }

    #[test]
    fn on_is_lit_and_text_is_escaped() {
        let svg = tile::decode(&tile_image(&view(true, "On")));
        assert!(svg.contains(ON_COLOR));
        assert!(svg.contains("by &lt;desktop&gt;"));
        let svg = tile::decode(&tile_image(&view(false, "Off")));
        assert!(!svg.contains(ON_COLOR));
        assert!(svg.contains(">Off<"));
    }
}
