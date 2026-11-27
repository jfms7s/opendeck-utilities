//! The Window Layout key tile.

use super::tile::{self, MUTED_TEXT_COLOR, TEXT_COLOR};

/// What a Window Layout key shows.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutView {
    /// The big line: the chosen layout's label ("1/3"), "Cycle", or
    /// "Linux only".
    pub title: String,
    /// Column widths to draw, left to right, as fractions of the screen
    /// that sum to 1. Empty: no diagram (cycle mode, unsupported).
    pub columns: Vec<f64>,
    /// The line under it: what a press does.
    pub hint: String,
}

/// Cycle mode's title; the tile draws a cycle mark over it.
pub const CYCLE_TITLE: &str = "Cycle";

pub const COLUMN_COLOR: &str = "#60a5fa";

/// The column diagram's box on the 100x100 tile.
const DIAGRAM_X: f64 = 8.0;
const DIAGRAM_WIDTH: f64 = 84.0;
const DIAGRAM_Y: f64 = 36.0;
const DIAGRAM_HEIGHT: f64 = 34.0;
const COLUMN_GAP: f64 = 4.0;

pub fn tile_image(view: &LayoutView) -> String {
    let body = if view.columns.is_empty() {
        let mark = if view.title == CYCLE_TITLE {
            cycle_mark()
        } else {
            String::new()
        };
        format!(
            "{mark}{title}{hint}",
            title = tile::text_line(60.0, 20.0, true, TEXT_COLOR, &view.title),
            hint = tile::text_line(82.0, 10.0, false, MUTED_TEXT_COLOR, &view.hint),
        )
    } else {
        format!(
            "{title}{columns}{hint}",
            title = tile::text_line(27.0, 18.0, true, TEXT_COLOR, &view.title),
            columns = columns(&view.columns),
            hint = tile::text_line(88.0, 11.0, false, MUTED_TEXT_COLOR, &view.hint),
        )
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{card}{body}</svg>"#,
        card = tile::card(),
    );
    tile::data_uri(&svg)
}

/// One rounded rectangle per column, as wide as its share of the diagram
/// once the gaps are taken out.
fn columns(widths: &[f64]) -> String {
    let gaps = COLUMN_GAP * (widths.len() - 1) as f64;
    let usable = DIAGRAM_WIDTH - gaps;
    let mut x = DIAGRAM_X;
    let mut out = String::new();
    for w in widths {
        let width = usable * w;
        out.push_str(&format!(
            r#"<rect x="{x:.1}" y="{DIAGRAM_Y}" width="{width:.1}" height="{DIAGRAM_HEIGHT}" rx="3" fill="{COLUMN_COLOR}"/>"#
        ));
        x += width + COLUMN_GAP;
    }
    out
}

/// A clockwise arrow round a circle, above the "Cycle" title.
fn cycle_mark() -> String {
    format!(
        r#"<path d="M 50 13 A 9 9 0 1 1 42.2 17.5" fill="none" stroke="{COLUMN_COLOR}" stroke-width="3" stroke-linecap="round"/><path d="M 44.2 13.5 L 39.6 16 L 44.8 19 Z" fill="{COLUMN_COLOR}"/>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(title: &str, columns: &[f64], hint: &str) -> String {
        tile::decode(&tile_image(&LayoutView {
            title: title.into(),
            columns: columns.to_vec(),
            hint: hint.into(),
        }))
    }

    /// `(x, width)` of every column rectangle, left to right.
    fn rects(svg: &str) -> Vec<(f64, f64)> {
        let attr = |rect: &str, name: &str| -> f64 {
            let start = rect.find(&format!(r#" {name}=""#)).unwrap() + name.len() + 3;
            let len = rect[start..].find('"').unwrap();
            rect[start..start + len].parse().unwrap()
        };
        svg.split("<rect")
            .skip(1)
            .map(|r| r.split_once("/>").unwrap().0)
            .filter(|r| r.contains(COLUMN_COLOR))
            .map(|r| (attr(r, "x"), attr(r, "width")))
            .collect()
    }

    #[test]
    fn columns_follow_the_layout_proportions() {
        let svg = view("1/4", &[0.25, 0.5, 0.25], "press to set");
        let r = rects(&svg);
        assert_eq!(r.len(), 3, "{svg}");
        assert!((r[1].1 - 2.0 * r[0].1).abs() < 0.2, "{r:?}");
        assert!((r[0].1 - r[2].1).abs() < 0.2, "{r:?}");
        // Left to right with a gap, inside the tile.
        for pair in r.windows(2) {
            assert!(pair[1].0 > pair[0].0 + pair[0].1, "{r:?}");
        }
        let (last_x, last_w) = r[2];
        assert!(r[0].0 >= 0.0 && last_x + last_w <= 100.0, "{r:?}");
        assert!(svg.contains(">1/4<"));
        assert!(svg.contains(">press to set<"));
    }

    #[test]
    fn thirds_draw_equal_columns_and_fifths_a_wide_middle() {
        let r = rects(&view("1/3", &[1.0 / 3.0; 3], ""));
        assert_eq!(r.len(), 3);
        assert!(r.iter().all(|c| (c.1 - r[0].1).abs() < 0.2), "{r:?}");
        let r = rects(&view("1/5", &[0.2, 0.6, 0.2], ""));
        assert_eq!(r.len(), 3);
        assert!((r[1].1 - 3.0 * r[0].1).abs() < 0.3, "{r:?}");
        assert!((r[0].1 - r[2].1).abs() < 0.2, "{r:?}");
    }

    #[test]
    fn cycle_has_a_mark_and_no_columns() {
        let svg = view(CYCLE_TITLE, &[], "1/3 → 1/4 → 1/5");
        assert!(rects(&svg).is_empty());
        assert!(svg.contains("<path"));
        assert!(svg.contains(">Cycle<"));
        assert!(svg.contains("1/3 → 1/4 → 1/5"));
    }

    #[test]
    fn linux_only_is_text_alone() {
        let svg = view("Linux only", &[], "");
        assert!(rects(&svg).is_empty());
        assert!(!svg.contains("<path"));
        assert!(svg.contains(">Linux only<"));
    }

    #[test]
    fn text_is_escaped() {
        let svg = view("<a&b>", &[0.5, 0.5], r#"say "hi""#);
        assert!(svg.contains("&lt;a&amp;b&gt;"), "{svg}");
        assert!(svg.contains("say &quot;hi&quot;"), "{svg}");
    }
}
