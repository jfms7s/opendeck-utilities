//! `org.freedesktop.Notifications` on the session bus: the `Inhibited`
//! property and `Inhibit`/`UnInhibit` (Plasma's notification server and
//! others that implement the inhibition extension). Only this file talks
//! to the bus for Do Not Disturb.
//!
//! One connection for the plugin's life: the server ties an inhibition to
//! the connection that asked for it, so the cookie is only good on it, and
//! the inhibition ends when the plugin exits and the connection closes.

use super::NotificationsPort;
use crate::dnd::Inhibition;
use async_trait::async_trait;
use futures_util::{Stream, StreamExt, stream};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{OnceCell, watch};
use zbus::proxy::CacheProperties;
use zbus::{Connection, Variant, proxy};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
/// What the server shows as the inhibiting app.
const DESKTOP_ENTRY: &str = "opendeck";
const REASON: &str = "Do Not Disturb key";
/// Re-read even without a signal (a server that never emits, or one that
/// restarted under a name the subscription missed).
const SAFETY_POLL: Duration = Duration::from_secs(30);
/// Every call gives up after this: a server that is alive but not
/// answering must not hold a press (or the cookie lock) forever.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

// Must match SERVICE, PATH and INTERFACE (the macro needs literals).
#[proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    fn inhibit(
        &self,
        desktop_entry: &str,
        reason: &str,
        hints: HashMap<&str, Variant<'_>>,
    ) -> zbus::Result<u32>;

    #[zbus(name = "UnInhibit")]
    fn un_inhibit(&self, cookie: u32) -> zbus::Result<()>;

    #[zbus(property)]
    fn inhibited(&self) -> zbus::Result<bool>;
}

enum Change {
    Property,
    Owner,
}

pub struct DbusNotifications {
    /// `None`: the session bus (`DBUS_SESSION_BUS_ADDRESS`).
    address: Option<String>,
    connection: OnceCell<Connection>,
    state: watch::Sender<Inhibition>,
    /// Bumped whenever the service name changes hands: cookies from an
    /// earlier server mean nothing to the new one.
    generation: AtomicU64,
    /// A failure was logged since the last good read (warn once per outage).
    warned: AtomicBool,
}

impl DbusNotifications {
    /// Connects lazily and keeps `inhibition()` current from then on.
    pub fn start(address: Option<String>) -> Arc<Self> {
        let me = Arc::new(Self {
            address,
            connection: OnceCell::new(),
            state: watch::Sender::new(Inhibition::Unknown),
            generation: AtomicU64::new(0),
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

    /// Every read is a round trip: zbus's property cache is never cleared
    /// when the server goes away.
    async fn proxy(&self) -> zbus::Result<NotificationsProxy<'static>> {
        NotificationsProxy::builder(self.connection().await?)
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    fn warn_once(&self, what: &str, e: &zbus::Error) {
        if !self.warned.swap(true, Ordering::SeqCst) {
            log::warn!("Do Not Disturb: {what}: {e}");
        } else {
            log::debug!("Do Not Disturb: {what}: {e}");
        }
    }

    async fn read(&self) -> Inhibition {
        let result = match self.proxy().await {
            Ok(proxy) => proxy.inhibited().await,
            Err(e) => Err(e),
        };
        match result {
            Ok(true) => {
                self.warned.store(false, Ordering::SeqCst);
                Inhibition::On
            }
            Ok(false) => {
                self.warned.store(false, Ordering::SeqCst);
                Inhibition::Off
            }
            Err(e) => {
                self.warn_once(&format!("can't read {INTERFACE}.Inhibited"), &e);
                Inhibition::Unknown
            }
        }
    }

    async fn refresh(&self) {
        let now = self.read().await;
        self.state.send_if_modified(|s| {
            let changed = *s != now;
            *s = now;
            changed
        });
    }

    /// Yields when the server announces a property change or the service
    /// name changes hands (server restarted or gone).
    async fn changes(&self) -> zbus::Result<impl Stream<Item = Change> + 'static> {
        let connection = self.connection().await?;
        let properties = zbus::fdo::PropertiesProxy::builder(connection)
            .destination(SERVICE)
            .path(PATH)
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let changed =
            properties
                .receive_properties_changed()
                .await?
                .filter_map(|signal| async move {
                    let args = signal.args().ok()?;
                    (args.interface_name().as_str() == INTERFACE).then_some(Change::Property)
                });
        let owners = self
            .proxy()
            .await?
            .inner()
            .receive_owner_changed()
            .await?
            .map(|_| Change::Owner);
        Ok(stream::select(changed, owners))
    }

    async fn run(self: Arc<Self>) {
        loop {
            match self.changes().await {
                Ok(changes) => {
                    let mut changes = std::pin::pin!(changes);
                    self.refresh().await;
                    loop {
                        tokio::select! {
                            next = changes.next() => match next {
                                Some(Change::Owner) => {
                                    self.generation.fetch_add(1, Ordering::SeqCst);
                                    self.refresh().await;
                                }
                                Some(Change::Property) => self.refresh().await,
                                None => break,
                            },
                            () = tokio::time::sleep(SAFETY_POLL) => self.refresh().await,
                        }
                    }
                }
                Err(e) => {
                    self.warn_once("can't watch the notification server", &e);
                    self.refresh().await;
                }
            }
            tokio::time::sleep(SAFETY_POLL).await;
        }
    }
}

#[async_trait]
impl NotificationsPort for DbusNotifications {
    fn inhibition(&self) -> watch::Receiver<Inhibition> {
        self.state.subscribe()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    async fn inhibit(&self) -> Result<u32, String> {
        let proxy = self.proxy().await.map_err(|e| e.to_string())?;
        let cookie = proxy
            .inhibit(DESKTOP_ENTRY, REASON, HashMap::new())
            .await
            .map_err(|e| e.to_string())?;
        self.refresh().await;
        Ok(cookie)
    }

    async fn uninhibit(&self, cookie: u32) -> Result<(), String> {
        let proxy = self.proxy().await.map_err(|e| e.to_string())?;
        let result = proxy.un_inhibit(cookie).await.map_err(|e| e.to_string());
        self.refresh().await;
        result
    }
}
