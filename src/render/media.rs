//! The Media dial's touch strip (`layouts/now-playing.json`).

use super::text::shorten;
use serde_json::{Value, json};

/// What a Media dial shows.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlayingView {
    pub title: String,
    pub artist: String,
    /// 0-100, the track's progress.
    pub progress: f64,
    pub playing: bool,
}

const PLAYING_COLOR: &str = "#22c55e";
const PAUSED_COLOR: &str = "#6b7280";

pub fn strip_feedback(view: &NowPlayingView) -> Value {
    json!({
        "title": shorten(&view.title, 22),
        "artist": shorten(&view.artist, 28),
        "bar": {
            "value": view.progress.clamp(0.0, 100.0),
            "bar_fill_c": if view.playing { PLAYING_COLOR } else { PAUSED_COLOR },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_keys_match_the_shipped_layout() {
        let layout: Value =
            serde_json::from_str(include_str!("../../assets/layouts/now-playing.json")).unwrap();
        let keys: Vec<&str> = layout["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap())
            .collect();
        let view = NowPlayingView {
            title: "t".into(),
            artist: "a".into(),
            progress: 150.0,
            playing: true,
        };
        let feedback = strip_feedback(&view);
        for k in feedback.as_object().unwrap().keys() {
            assert!(keys.contains(&k.as_str()), "layout has no item keyed {k}");
        }
        assert_eq!(feedback["bar"]["value"], 100.0);
    }
}
