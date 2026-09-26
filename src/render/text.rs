/// Truncates to `max_chars` characters, ending in `…` when cut.
pub fn shorten(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max_chars || max_chars == 0 {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

/// Only `#rgb` / `#rrggbb` pass - anything else (it is user text that ends
/// up in SVG attributes and layout colours) becomes `fallback`.
pub fn sanitize_color(value: &str, fallback: &str) -> String {
    let v = value.trim();
    let hex = v.strip_prefix('#').unwrap_or("");
    if matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        v.to_string()
    } else {
        fallback.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_keeps_short_and_cuts_long() {
        assert_eq!(shorten("Razer", 10), "Razer");
        assert_eq!(shorten("Razer Leviathan V2", 10), "Razer Lev…");
    }

    #[test]
    fn sanitize_color_accepts_hex() {
        assert_eq!(sanitize_color("#4fc3f7", "#000"), "#4fc3f7");
        assert_eq!(sanitize_color("#ABC", "#000"), "#ABC");
    }

    #[test]
    fn sanitize_color_rejects_everything_else() {
        for bad in [
            r#"red" onload="x"#,
            "url(#a)",
            "#12345",
            "#ggg",
            "",
            "4fc3f7",
            "#4fc3f7;x",
        ] {
            assert_eq!(sanitize_color(bad, "#000"), "#000", "{bad}");
        }
    }
}
