//! A fake MPRIS player on a private bus (`crate::test_bus`).

use crate::test_bus::PrivateBus;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use zbus::names::InterfaceName;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, OwnedVariant, Variant};

const PATH: &str = "/org/mpris/MediaPlayer2";
const INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

pub struct PlayerState {
    pub title: Mutex<String>,
    pub artist: Mutex<String>,
    pub playing: Mutex<bool>,
    /// Microseconds.
    pub position: Mutex<i64>,
    pub length: i64,
    pub calls: Mutex<Vec<String>>,
    /// Reads of `Position` (so `GetAll`) never answer: a hung player.
    pub hang: AtomicBool,
    conn: OnceLock<Connection>,
}

impl PlayerState {
    fn status(&self) -> &'static str {
        if *self.playing.lock().unwrap() {
            "Playing"
        } else {
            "Paused"
        }
    }

    fn metadata(&self) -> HashMap<String, OwnedVariant> {
        let v = |x: Variant<'static>| x.try_to_owned().unwrap();
        HashMap::from([
            (
                "xesam:title".to_string(),
                v(Variant::from(self.title.lock().unwrap().clone())),
            ),
            (
                "xesam:artist".to_string(),
                v(Variant::from(vec![self.artist.lock().unwrap().clone()])),
            ),
            ("mpris:length".to_string(), v(Variant::from(self.length))),
        ])
    }

    /// Announces `PlaybackStatus` and `Metadata`, as players do.
    pub async fn announce(&self) {
        let emitter = SignalEmitter::new(self.conn.get().unwrap(), PATH).unwrap();
        zbus::fdo::Properties::properties_changed(
            &emitter,
            InterfaceName::try_from(INTERFACE).unwrap(),
            HashMap::from([
                ("PlaybackStatus", Variant::from(self.status())),
                ("Metadata", Variant::from(self.metadata())),
            ]),
            Cow::Borrowed(&[]),
        )
        .await
        .unwrap();
    }

    /// The player quits: its bus name goes away.
    pub async fn quit(&self, name: &str) {
        self.conn
            .get()
            .unwrap()
            .release_name(format!("org.mpris.MediaPlayer2.{name}"))
            .await
            .unwrap();
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

struct FakePlayer(Arc<PlayerState>);

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl FakePlayer {
    async fn play_pause(&self) {
        self.0.calls.lock().unwrap().push("PlayPause".into());
        {
            let mut playing = self.0.playing.lock().unwrap();
            *playing = !*playing;
        }
        self.0.announce().await;
    }

    async fn next(&self) {
        self.0.calls.lock().unwrap().push("Next".into());
        *self.0.title.lock().unwrap() = "Next Song".into();
        *self.0.position.lock().unwrap() = 0;
        self.0.announce().await;
    }

    fn seek(&self, offset: i64) {
        self.0.calls.lock().unwrap().push(format!("Seek {offset}"));
        let mut position = self.0.position.lock().unwrap();
        *position = (*position + offset).clamp(0, self.0.length);
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn playback_status(&self) -> String {
        self.0.status().to_string()
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn metadata(&self) -> HashMap<String, OwnedVariant> {
        self.0.metadata()
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn position(&self) -> i64 {
        if self.0.hang.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
        *self.0.position.lock().unwrap()
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn can_seek(&self) -> bool {
        true
    }
}

/// Serves a playing fake player as `org.mpris.MediaPlayer2.<name>`:
/// "Song" by "Artist", 200 s long, at 50 s.
pub async fn serve_player(bus: &PrivateBus, name: &str) -> Arc<PlayerState> {
    let state = Arc::new(PlayerState {
        title: Mutex::new("Song".into()),
        artist: Mutex::new("Artist".into()),
        playing: Mutex::new(true),
        position: Mutex::new(50_000_000),
        length: 200_000_000,
        calls: Mutex::default(),
        hang: AtomicBool::new(false),
        conn: OnceLock::new(),
    });
    let conn = zbus::connection::Builder::address(bus.address.as_str())
        .name(format!("org.mpris.MediaPlayer2.{name}"))
        .serve_at(PATH, FakePlayer(state.clone()))
        .build()
        .await
        .unwrap();
    state.conn.set(conn).unwrap();
    state
}
