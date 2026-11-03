//! Now playing: which MPRIS player a Media dial follows and what it
//! shows. Pure, apart from the port: the bus is behind `MediaPort`
//! (`mpris.rs` on Linux; nothing elsewhere).
// Only the MPRIS adapter (Linux) builds players; on macOS the port is
// always `Unsupported`, but the pure code still builds and is tested.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(all(test, target_os = "linux"))]
mod dbus_tests;
#[cfg(all(test, target_os = "linux"))]
mod fake_player;
#[cfg(target_os = "linux")]
pub mod mpris;

use crate::render::media::NowPlayingView;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::watch;

/// How far one detent seeks (ruling 3).
pub const SEEK_PER_TICK: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl Status {
    /// MPRIS `PlaybackStatus`; anything unknown counts as stopped.
    pub fn parse(s: &str) -> Self {
        match s {
            "Playing" => Status::Playing,
            "Paused" => Status::Paused,
            _ => Status::Stopped,
        }
    }
}

/// One MPRIS player, as last read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Player {
    /// Its bus name, e.g. `org.mpris.MediaPlayer2.spotify`.
    pub bus: String,
    pub title: String,
    pub artists: Vec<String>,
    /// Track length (`mpris:length`), when the player gives one.
    pub length: Option<Duration>,
    pub position: Duration,
    pub status: Status,
    pub can_seek: bool,
}

/// What the port knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Players {
    /// No MPRIS on this platform: "Linux only".
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Unsupported,
    /// The session bus can't be read.
    Unknown,
    /// Every player on the bus (possibly none).
    Listed(Vec<Player>),
}

/// The player a dial follows (ruling 4): a playing one (the one followed
/// last if it is among them, else the first by bus name); else the one
/// followed last if it still exists; else the first by bus name.
pub fn pick<'a>(players: &'a [Player], last: Option<&str>) -> Option<&'a Player> {
    let is_last = |p: &&Player| Some(p.bus.as_str()) == last;
    let first = |it: Vec<&'a Player>| it.into_iter().min_by(|a, b| a.bus.cmp(&b.bus));
    let playing: Vec<&Player> = players
        .iter()
        .filter(|p| p.status == Status::Playing)
        .collect();
    playing
        .iter()
        .copied()
        .find(is_last)
        .or_else(|| first(playing))
        .or_else(|| players.iter().find(is_last))
        .or_else(|| first(players.iter().collect()))
}

/// What a Media dial shows for `players`, following `followed`.
pub fn view(players: &Players, followed: Option<&Player>) -> NowPlayingView {
    let message = |title: &str, artist: &str| NowPlayingView {
        title: title.to_string(),
        artist: artist.to_string(),
        progress: 0.0,
        playing: false,
    };
    match (players, followed) {
        (Players::Unsupported, _) => message("Media", "Linux only"),
        (Players::Unknown, _) => message("Media", "unavailable"),
        (Players::Listed(_), None) => message("Nothing playing", ""),
        (Players::Listed(_), Some(p)) => {
            let artist = p.artists.join(", ");
            let artist = match p.status {
                Status::Playing => artist,
                Status::Paused if artist.is_empty() => "paused".to_string(),
                Status::Paused => format!("paused · {artist}"),
                Status::Stopped if artist.is_empty() => "stopped".to_string(),
                Status::Stopped => format!("stopped · {artist}"),
            };
            NowPlayingView {
                title: if p.title.is_empty() {
                    short_name(&p.bus).to_string()
                } else {
                    p.title.clone()
                },
                artist,
                progress: progress(p),
                playing: p.status == Status::Playing,
            }
        }
    }
}

/// `org.mpris.MediaPlayer2.spotify` -> `spotify`.
fn short_name(bus: &str) -> &str {
    bus.strip_prefix("org.mpris.MediaPlayer2.").unwrap_or(bus)
}

/// 0-100; no length, no fill.
fn progress(p: &Player) -> f64 {
    match p.length {
        Some(len) if !len.is_zero() => {
            (p.position.as_secs_f64() / len.as_secs_f64() * 100.0).clamp(0.0, 100.0)
        }
        _ => 0.0,
    }
}

/// Microseconds to seek for `ticks` detents (MPRIS `Seek` offset).
pub fn seek_offset(ticks: i16) -> i64 {
    i64::from(ticks) * SEEK_PER_TICK.as_micros() as i64
}

#[async_trait]
pub trait MediaPort: Send + Sync {
    fn players(&self) -> watch::Receiver<Players>;
    /// Whether any Media dial is visible: the port only polls positions
    /// while one is.
    fn set_watching(&self, watching: bool);
    async fn play_pause(&self, bus: &str) -> Result<(), String>;
    async fn next(&self, bus: &str) -> Result<(), String>;
    /// Relative, in microseconds (negative: back).
    async fn seek(&self, bus: &str, offset: i64) -> Result<(), String>;
}

/// No MPRIS on this platform.
#[cfg(not(target_os = "linux"))]
pub struct Unsupported {
    state: watch::Sender<Players>,
}

#[cfg(not(target_os = "linux"))]
impl Default for Unsupported {
    fn default() -> Self {
        Self {
            state: watch::Sender::new(Players::Unsupported),
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[async_trait]
impl MediaPort for Unsupported {
    fn players(&self) -> watch::Receiver<Players> {
        self.state.subscribe()
    }
    fn set_watching(&self, _watching: bool) {}
    async fn play_pause(&self, _bus: &str) -> Result<(), String> {
        Err("MPRIS is Linux only".into())
    }
    async fn next(&self, _bus: &str) -> Result<(), String> {
        Err("MPRIS is Linux only".into())
    }
    async fn seek(&self, _bus: &str, _offset: i64) -> Result<(), String> {
        Err("MPRIS is Linux only".into())
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Records calls; `set` is the bus changing.
    pub struct FakeMedia {
        state: watch::Sender<Players>,
        calls: Mutex<Vec<String>>,
        pub watching: AtomicBool,
    }

    impl FakeMedia {
        pub fn new(players: Players) -> Self {
            Self {
                state: watch::Sender::new(players),
                calls: Mutex::default(),
                watching: AtomicBool::new(false),
            }
        }
        pub fn set(&self, players: Players) {
            self.state.send_replace(players);
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn push(&self, call: String) -> Result<(), String> {
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
    }

    #[async_trait]
    impl MediaPort for FakeMedia {
        fn players(&self) -> watch::Receiver<Players> {
            self.state.subscribe()
        }
        fn set_watching(&self, watching: bool) {
            self.watching.store(watching, Ordering::SeqCst);
        }
        async fn play_pause(&self, bus: &str) -> Result<(), String> {
            self.push(format!("PlayPause {bus}"))
        }
        async fn next(&self, bus: &str) -> Result<(), String> {
            self.push(format!("Next {bus}"))
        }
        async fn seek(&self, bus: &str, offset: i64) -> Result<(), String> {
            self.push(format!("Seek {bus} {offset}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(bus: &str, status: Status) -> Player {
        Player {
            bus: bus.into(),
            status,
            ..Default::default()
        }
    }

    #[test]
    fn a_playing_player_wins_then_the_last_followed_then_the_first() {
        let a = player("org.mpris.MediaPlayer2.a", Status::Paused);
        let b = player("org.mpris.MediaPlayer2.b", Status::Playing);
        let c = player("org.mpris.MediaPlayer2.c", Status::Playing);
        let all = [c.clone(), a.clone(), b.clone()];
        assert_eq!(pick(&all, None), Some(&b), "first playing by name");
        assert_eq!(pick(&all, Some(&c.bus)), Some(&c), "last followed, playing");
        assert_eq!(
            pick(&all, Some(&a.bus)),
            Some(&b),
            "playing beats last followed"
        );
        let paused = [
            player("org.mpris.MediaPlayer2.z", Status::Paused),
            a.clone(),
        ];
        assert_eq!(
            pick(&paused, Some("org.mpris.MediaPlayer2.z")),
            Some(&paused[0])
        );
        assert_eq!(pick(&paused, Some("gone")), Some(&a), "first by name");
        assert_eq!(pick(&[], None), None);
    }

    #[test]
    fn the_strip_shows_title_artists_and_progress() {
        let p = Player {
            bus: "org.mpris.MediaPlayer2.spotify".into(),
            title: "Song".into(),
            artists: vec!["A".into(), "B".into()],
            length: Some(Duration::from_secs(200)),
            position: Duration::from_secs(50),
            status: Status::Playing,
            can_seek: true,
        };
        let v = view(&Players::Listed(vec![p.clone()]), Some(&p));
        assert_eq!((v.title.as_str(), v.artist.as_str()), ("Song", "A, B"));
        assert_eq!(v.progress, 25.0);
        assert!(v.playing);
        let paused = Player {
            status: Status::Paused,
            title: String::new(),
            length: None,
            ..p
        };
        let v = view(&Players::Listed(vec![]), Some(&paused));
        assert_eq!(
            (v.title.as_str(), v.artist.as_str()),
            ("spotify", "paused · A, B")
        );
        assert_eq!(v.progress, 0.0);
    }

    #[test]
    fn nothing_playing_unavailable_and_linux_only() {
        assert_eq!(
            view(&Players::Listed(vec![]), None).title,
            "Nothing playing"
        );
        assert_eq!(view(&Players::Unknown, None).artist, "unavailable");
        assert_eq!(view(&Players::Unsupported, None).artist, "Linux only");
    }

    #[test]
    fn a_detent_seeks_five_seconds() {
        assert_eq!(seek_offset(1), 5_000_000);
        assert_eq!(seek_offset(-3), -15_000_000);
    }

    #[test]
    fn unknown_statuses_are_stopped() {
        assert_eq!(Status::parse("Playing"), Status::Playing);
        assert_eq!(Status::parse("Paused"), Status::Paused);
        assert_eq!(Status::parse("weird"), Status::Stopped);
    }
}
