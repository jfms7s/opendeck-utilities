//! Small glyphs drawn in a 100x100 box: used inside key tiles and, wrapped
//! in their own `<svg>`, as the touch-strip `icon` pixmap.

use super::level::MUTED_COLOR;
use super::tile::{data_uri, escape_xml};

/// The strike through a muted icon uses the "muted" colour.
pub const STRIKE_COLOR: &str = MUTED_COLOR;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    Speaker,
    Mic,
    Sun,
    Letter(char),
}

pub fn icon_svg(icon: &Icon, color: &str, struck: bool) -> String {
    let body = match icon {
        Icon::Speaker => format!(
            r#"<path d="M14 38 H32 L54 18 V82 L32 62 H14 Z" fill="{color}"/><path d="M64 34 Q76 50 64 66" fill="none" stroke="{color}" stroke-width="7" stroke-linecap="round"/><path d="M74 22 Q94 50 74 78" fill="none" stroke="{color}" stroke-width="7" stroke-linecap="round"/>"#
        ),
        Icon::Mic => format!(
            r#"<rect x="37" y="10" width="26" height="48" rx="13" fill="{color}"/><path d="M26 44 Q26 72 50 72 Q74 72 74 44" fill="none" stroke="{color}" stroke-width="7" stroke-linecap="round"/><path d="M50 72 V88 M36 90 H64" stroke="{color}" stroke-width="7" stroke-linecap="round"/>"#
        ),
        Icon::Sun => {
            let rays: String = (0..8)
                .map(|i| {
                    let a = f64::from(i) * std::f64::consts::FRAC_PI_4;
                    let (x1, y1) = (50.0 + 26.0 * a.cos(), 50.0 + 26.0 * a.sin());
                    let (x2, y2) = (50.0 + 40.0 * a.cos(), 50.0 + 40.0 * a.sin());
                    format!(
                        r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{color}" stroke-width="7" stroke-linecap="round"/>"#
                    )
                })
                .collect();
            format!(r#"<circle cx="50" cy="50" r="17" fill="{color}"/>{rays}"#)
        }
        Icon::Letter(c) => format!(
            r#"<circle cx="50" cy="50" r="42" fill="none" stroke="{color}" stroke-width="7"/><text x="50" y="66" text-anchor="middle" font-family="sans-serif" font-size="46" font-weight="700" fill="{color}">{}</text>"#,
            escape_xml(&c.to_string())
        ),
    };
    let strike = if struck {
        format!(
            r#"<line x1="14" y1="86" x2="86" y2="14" stroke="{STRIKE_COLOR}" stroke-width="9" stroke-linecap="round"/>"#
        )
    } else {
        String::new()
    };
    format!("{body}{strike}")
}

pub fn icon_data_uri(icon: &Icon, color: &str, struck: bool) -> String {
    data_uri(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{}</svg>"#,
        icon_svg(icon, color, struck)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struck_icons_carry_the_strike_line() {
        assert!(icon_svg(&Icon::Speaker, "#fff", true).contains(STRIKE_COLOR));
        assert!(!icon_svg(&Icon::Speaker, "#fff", false).contains(STRIKE_COLOR));
    }

    #[test]
    fn letter_is_escaped() {
        assert!(icon_svg(&Icon::Letter('<'), "#fff", false).contains("&lt;"));
    }

    #[test]
    fn data_uri_wraps_a_standalone_svg() {
        let svg = super::super::tile::decode(&icon_data_uri(&Icon::Mic, "#fff", false));
        assert!(svg.starts_with("<svg xmlns="));
    }
}
