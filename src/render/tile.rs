//! Shared pieces for generated key images: the dark card, text colours and
//! text drawn *inside* the SVG. Text is baked into the image rather than
//! sent as the key's native title so size, weight and contrast stay under
//! our control whatever title settings the key has.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

pub const CARD_COLOR: &str = "#111827";
pub const TEXT_COLOR: &str = "#f9fafb";
pub const MUTED_TEXT_COLOR: &str = "#d1d5db";

const MAX_TEXT_WIDTH: f64 = 94.0;
const REGULAR_CHAR_WIDTH: f64 = 0.58;
const BOLD_CHAR_WIDTH: f64 = 0.64;

pub fn card() -> String {
    format!(r#"<rect x="0" y="0" width="100" height="100" fill="{CARD_COLOR}" />"#)
}

/// One horizontally-centred line with its baseline at `y`; lines estimated
/// wider than the key are squeezed with `textLength` rather than clipped.
pub fn text_line(y: f64, size: f64, bold: bool, color: &str, content: &str) -> String {
    let char_width = if bold {
        BOLD_CHAR_WIDTH
    } else {
        REGULAR_CHAR_WIDTH
    };
    let estimated_width = content.chars().count() as f64 * size * char_width;
    let fit = if estimated_width > MAX_TEXT_WIDTH {
        format!(r#" textLength="{MAX_TEXT_WIDTH}" lengthAdjust="spacingAndGlyphs""#)
    } else {
        String::new()
    };
    let weight = if bold { "700" } else { "500" };
    let escaped = escape_xml(content);
    format!(
        r#"<text x="50" y="{y}" text-anchor="middle" font-family="sans-serif" font-size="{size}" font-weight="{weight}" fill="{color}"{fit}>{escaped}</text>"#
    )
}

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `setImage` only treats `image` as inline data when it starts with `data:`.
pub fn data_uri(svg: &str) -> String {
    format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg.as_bytes())
    )
}

#[cfg(test)]
pub(crate) fn decode(uri: &str) -> String {
    let b64 = uri.strip_prefix("data:image/svg+xml;base64,").unwrap();
    String::from_utf8(STANDARD.decode(b64).unwrap()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_lines_are_not_squeezed() {
        assert!(!text_line(80.0, 14.0, false, TEXT_COLOR, "45%").contains("textLength"));
    }

    #[test]
    fn long_lines_are_squeezed() {
        assert!(
            text_line(80.0, 14.0, false, TEXT_COLOR, "a very long device label")
                .contains(r#"textLength="94""#)
        );
    }

    #[test]
    fn text_is_escaped() {
        let line = text_line(80.0, 14.0, false, TEXT_COLOR, r#"<b> & "q""#);
        assert!(line.contains("&lt;b&gt; &amp; &quot;q&quot;"), "{line}");
    }

    #[test]
    fn data_uri_round_trips() {
        assert_eq!(decode(&data_uri("<svg/>")), "<svg/>");
    }
}
