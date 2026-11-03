//! MPRIS players on the session bus (`org.mpris.MediaPlayer2.*`): the only
//! code that talks to them. Every player is re-read whole (`GetAll` on the
//! `Player` interface) when any of them announces a change or a player
//! comes or goes; `Position` emits nothing, so it is re-read once a second
//! while a Media dial is visible and something plays.

use super::{MediaPort, Player, Players, Status};
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, OnceCell, watch};
use zbus::message::Type;
use zbus::names::InterfaceName;
use zbus::proxy::CacheProperties;
use zbus::{Connection, MatchRule, MessageStream, OwnedVariant, proxy};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
/// Every call gives up after this: a player that hangs must not stall the
/// dial or the deck.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// Position re-read while something plays and a dial is visible.
const TICK: Duration = Duration::from_secs(1);
/// Re-read even without a signal.
const SAFETY_POLL: Duration = Duration::from_secs(30);

// Must match PATH and PLAYER_INTERFACE (the macro needs literals).
#[proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait MprisPlayer {
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn seek(&self, offset: i64) -> zbus::Result<()>;
}

fn string(v: &OwnedVariant) -> Option<String> {
    <&str>::try_from(&**v).ok().map(str::to_string)
}

/// `mpris:length` and `Position` are `x` by the spec; some players send
/// `t`. Negative is nonsense: zero.
fn micros(v: &OwnedVariant) -> Option<Duration> {
    let us = i64::try_from(&**v)
        .ok()
        .or_else(|| u64::try_from(&**v).ok().map(|u| u as i64))?;
    Some(Duration::from_micros(us.max(0) as u64))
}

/// One player from its `Player` interface properties; missing or oddly
/// typed fields fall back (no title, stopped, can't seek).
pub fn parse_player(bus: &str, props: &HashMap<String, OwnedVariant>) -> Player {
    let metadata: HashMap<String, OwnedVariant> = props
        .get("Metadata")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| HashMap::try_from(v).ok())
        .unwrap_or_default();
    let artists = metadata
        .get("xesam:artist")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .or_else(|| {
            metadata
                .get("xesam:artist")
                .and_then(string)
                .map(|a| vec![a])
        })
        .unwrap_or_default();
    Player {
        bus: bus.to_string(),
        title: metadata
            .get("xesam:title")
            .and_then(string)
            .unwrap_or_default(),
        artists,
        length: metadata.get("mpris:length").and_then(micros),
        position: props.get("Position").and_then(micros).unwrap_or_default(),
        status: props
            .get("PlaybackStatus")
            .and_then(string)
            .map(|s| Status::parse(&s))
            .unwrap_or_default(),
        can_seek: props
            .get("CanSeek")
            .and_then(|v| bool::try_from(&**v).ok())
            .unwrap_or(false),
    }
}

pub struct MprisMedia {
    /// `None`: the session bus.
    address: Option<String>,
    connection: OnceCell<Connection>,
    state: watch::Sender<Players>,
    watching: AtomicBool,
    wake: Notify,
    warned: AtomicBool,
}

impl MprisMedia {
    pub fn start(address: Option<String>) -> Arc<Self> {
        let me = Arc::new(Self {
            address,
            connection: OnceCell::new(),
            state: watch::Sender::new(Players::Unknown),
            watching: AtomicBool::new(false),
            wake: Notify::new(),
            warned: AtomicBool::new(false),
        });
        tokio::spawn(me.clone().run());
        me
    }

    async fn connection(&self) -> zbus::Result<&Connection> {
        self.connection
            .get_or_try_init(|| async {
                let builder = match &self.address {
                    Some(address) => zbus::connection::Builder::address(address.as_str()),
                    None => zbus::connection::Builder::session(),
                };
                builder.method_timeout(CALL_TIMEOUT).build().await
            })
            .await
    }

    fn warn_once(&self, what: &str, e: &zbus::Error) {
        if !self.warned.swap(true, Ordering::SeqCst) {
            log::warn!("Media: {what}: {e}");
        } else {
            log::debug!("Media: {what}: {e}");
        }
    }

    async fn read_player(conn: &Connection, bus: &str) -> zbus::Result<Player> {
        let props = zbus::fdo::PropertiesProxy::builder(conn)
            .destination(bus.to_string())
            .path(PATH)
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let all = props
            .get_all(InterfaceName::from_static_str_unchecked(PLAYER_INTERFACE))
            .await?;
        Ok(parse_player(bus, &all))
    }

    async fn read(&self) -> zbus::Result<Vec<Player>> {
        let conn = self.connection().await?;
        let names = zbus::fdo::DBusProxy::new(conn).await?.list_names().await?;
        let mut players = Vec::new();
        for name in names.iter().filter(|n| n.as_str().starts_with(PREFIX)) {
            match Self::read_player(conn, name.as_str()).await {
                Ok(p) => players.push(p),
                // A player that just quit, or doesn't implement Player.
                Err(e) => log::debug!("Media: can't read {name}: {e}"),
            }
        }
        players.sort_by(|a, b| a.bus.cmp(&b.bus));
        Ok(players)
    }

    async fn refresh(&self) {
        let now = match self.read().await {
            Ok(players) => {
                self.warned.store(false, Ordering::SeqCst);
                Players::Listed(players)
            }
            Err(e) => {
                self.warn_once("can't list MPRIS players", &e);
                Players::Unknown
            }
        };
        self.state.send_if_modified(|s| {
            let changed = *s != now;
            *s = now;
            changed
        });
    }

    fn anything_playing(&self) -> bool {
        matches!(&*self.state.borrow(), Players::Listed(ps) if ps.iter().any(|p| p.status == Status::Playing))
    }

    /// Yields on any player's property change and on any MPRIS name
    /// coming or going.
    async fn changes(&self) -> zbus::Result<impl futures_util::Stream<Item = ()> + 'static> {
        let conn = self.connection().await?;
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .interface("org.freedesktop.DBus.Properties")
            .member("PropertiesChanged")
            .path(PATH)
            .build()?;
        let props = MessageStream::for_match_rule(rule, conn, None)
            .await?
            .map(|_| ());
        let owners = zbus::fdo::DBusProxy::new(conn)
            .await?
            .receive_name_owner_changed()
            .await?
            .filter_map(|signal| async move {
                let args = signal.args().ok()?;
                args.name().as_str().starts_with(PREFIX).then_some(())
            });
        Ok(stream::select(props, owners))
    }

    async fn run(self: Arc<Self>) {
        loop {
            match self.changes().await {
                Ok(changes) => {
                    let mut changes = std::pin::pin!(changes);
                    self.refresh().await;
                    loop {
                        let wait =
                            if self.watching.load(Ordering::SeqCst) && self.anything_playing() {
                                TICK
                            } else {
                                SAFETY_POLL
                            };
                        tokio::select! {
                            next = changes.next() => match next {
                                Some(()) => self.refresh().await,
                                None => break,
                            },
                            () = self.wake.notified() => self.refresh().await,
                            () = tokio::time::sleep(wait) => self.refresh().await,
                        }
                    }
                }
                Err(e) => {
                    self.warn_once("can't watch MPRIS players", &e);
                    self.refresh().await;
                }
            }
            tokio::time::sleep(SAFETY_POLL).await;
        }
    }

    async fn player(&self, bus: &str) -> Result<MprisPlayerProxy<'static>, String> {
        let conn = self.connection().await.map_err(|e| e.to_string())?;
        MprisPlayerProxy::builder(conn)
            .destination(bus.to_string())
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(|e| e.to_string())
    }

    /// Calls `f` on `bus`'s player, then re-reads at once (the change
    /// signal may lag, and `Seek` changes no property).
    async fn call<F, Fut>(&self, bus: &str, f: F) -> Result<(), String>
    where
        F: FnOnce(MprisPlayerProxy<'static>) -> Fut,
        Fut: Future<Output = zbus::Result<()>>,
    {
        if !bus.starts_with(PREFIX) {
            return Err(format!("not an MPRIS player: {bus:?}"));
        }
        let result = f(self.player(bus).await?).await.map_err(|e| e.to_string());
        self.refresh().await;
        result
    }
}

#[async_trait]
impl MediaPort for MprisMedia {
    fn players(&self) -> watch::Receiver<Players> {
        self.state.subscribe()
    }

    fn set_watching(&self, watching: bool) {
        if !self.watching.swap(watching, Ordering::SeqCst) && watching {
            // A dial just appeared: start ticking now, not at the next poll.
            self.wake.notify_one();
        }
    }

    async fn play_pause(&self, bus: &str) -> Result<(), String> {
        self.call(bus, |p| async move { p.play_pause().await })
            .await
    }

    async fn next(&self, bus: &str) -> Result<(), String> {
        self.call(bus, |p| async move { p.next().await }).await
    }

    async fn seek(&self, bus: &str, offset: i64) -> Result<(), String> {
        self.call(bus, move |p| async move { p.seek(offset).await })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::Variant;

    fn v(value: Variant<'static>) -> OwnedVariant {
        value.try_to_owned().unwrap()
    }

    #[test]
    fn a_full_player_parses() {
        let metadata: HashMap<String, Variant<'static>> = HashMap::from([
            ("xesam:title".to_string(), Variant::from("Song")),
            (
                "xesam:artist".to_string(),
                Variant::from(vec!["A".to_string(), "B".to_string()]),
            ),
            ("mpris:length".to_string(), Variant::from(200_000_000i64)),
        ]);
        let props = HashMap::from([
            ("Metadata".to_string(), v(Variant::from(metadata))),
            ("Position".to_string(), v(Variant::from(50_000_000i64))),
            ("PlaybackStatus".to_string(), v(Variant::from("Playing"))),
            ("CanSeek".to_string(), v(Variant::from(true))),
        ]);
        let p = parse_player("org.mpris.MediaPlayer2.x", &props);
        assert_eq!(p.title, "Song");
        assert_eq!(p.artists, vec!["A", "B"]);
        assert_eq!(p.length, Some(Duration::from_secs(200)));
        assert_eq!(p.position, Duration::from_secs(50));
        assert_eq!(p.status, Status::Playing);
        assert!(p.can_seek);
    }

    #[test]
    fn odd_or_missing_fields_fall_back() {
        let metadata: HashMap<String, Variant<'static>> = HashMap::from([
            ("xesam:artist".to_string(), Variant::from("Solo")),
            ("mpris:length".to_string(), Variant::from(7u64)),
        ]);
        let props = HashMap::from([
            ("Metadata".to_string(), v(Variant::from(metadata))),
            ("Position".to_string(), v(Variant::from(-5i64))),
            ("PlaybackStatus".to_string(), v(Variant::from(3u32))),
        ]);
        let p = parse_player("b", &props);
        assert_eq!(p.title, "");
        assert_eq!(p.artists, vec!["Solo"]);
        assert_eq!(p.length, Some(Duration::from_micros(7)));
        assert_eq!(p.position, Duration::ZERO);
        assert_eq!(p.status, Status::Stopped);
        assert!(!p.can_seek);
        assert_eq!(
            parse_player("b", &HashMap::new()),
            Player {
                bus: "b".into(),
                ..Default::default()
            }
        );
    }
}
