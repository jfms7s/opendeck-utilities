//! The one send-and-confirm mechanism for host requests (spec §6.5), shared
//! by Device Brightness and Switch Profile.
//!
//! Each coalescing key (one for brightness, one per device for profiles)
//! gets a background worker, so the event loop never waits on the
//! `opendeck` command. Requests submitted while one is being sent are merged
//! into the next one ("latest wins", or summed for relative steps). For each
//! request the worker builds the event and its confirmation rule *at send
//! time* (from OpenDeck's state just before sending), sends it, and then:
//!
//! - `Confirm::Skip` - nothing can change, done;
//! - `Confirm::Unreadable` - OpenDeck's state can't be read, so the outcome
//!   can't be checked: logged once with the path, no alert on the key;
//! - `Confirm::Until` - waits for the predicate; if it never holds and no
//!   newer request overtook this one, the instance alerts and
//!   `host::warn_ignored_once` points at the sending route.
//!
//! A send that fails outright alerts the instance and logs the error.

use crate::host::{self, Host};
use crate::ui::Ui;
use dashmap::DashMap;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

pub enum Confirm {
    Skip,
    /// Why the outcome can't be checked (shown once in the log).
    Unreadable(String),
    Until(Box<dyn Fn() -> bool + Send + Sync>),
}

pub struct Job {
    pub event: Value,
    pub confirm: Confirm,
}

type Build<R> = dyn Fn(R) -> Job + Send + Sync;
type Merge<R> = fn(R, R) -> R;

struct Slot<R> {
    /// The request waiting to be sent and the instance that asked.
    pending: Mutex<Option<(R, String)>>,
    /// Bumped on every submit, so the worker can tell it was overtaken.
    generation: AtomicU64,
    wake: Notify,
}

pub struct Dispatcher<R> {
    inner: Arc<Inner<R>>,
}

impl<R> Clone for Dispatcher<R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct Inner<R> {
    host: Arc<dyn Host>,
    ui: Arc<dyn Ui>,
    changes: watch::Receiver<u64>,
    build: Box<Build<R>>,
    merge: Merge<R>,
    /// Wait this long after the last submit before sending, so a dial spin
    /// usually becomes one `opendeck` launch (capped at `MAX_SETTLE`).
    settle: Duration,
    slots: DashMap<String, Arc<Slot<R>>>,
}

const MAX_SETTLE: Duration = Duration::from_millis(400);
const WARN_UNREADABLE: &str = "dispatch::unreadable";

/// Merge rule for requests where the newest simply wins.
pub fn latest<R>(_older: R, newer: R) -> R {
    newer
}

impl<R: Send + 'static> Dispatcher<R> {
    pub fn new(
        host: Arc<dyn Host>,
        ui: Arc<dyn Ui>,
        changes: watch::Receiver<u64>,
        settle: Duration,
        merge: Merge<R>,
        build: impl Fn(R) -> Job + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                host,
                ui,
                changes,
                build: Box::new(build),
                merge,
                settle,
                slots: DashMap::new(),
            }),
        }
    }

    /// Queues `request` for `key`, merging it with one not yet sent.
    pub fn submit(&self, key: &str, instance: &str, request: R) {
        let slot = self.slot(key);
        {
            let mut pending = slot.pending.lock().unwrap_or_else(|e| e.into_inner());
            let merged = match pending.take() {
                Some((older, _)) => (self.inner.merge)(older, request),
                None => request,
            };
            *pending = Some((merged, instance.to_string()));
        }
        slot.generation.fetch_add(1, Ordering::SeqCst);
        slot.wake.notify_one();
    }

    /// The key's slot, starting its worker the first time the key is used.
    fn slot(&self, key: &str) -> Arc<Slot<R>> {
        self.inner
            .slots
            .entry(key.to_string())
            .or_insert_with(|| {
                let slot = Arc::new(Slot {
                    pending: Mutex::new(None),
                    generation: AtomicU64::new(0),
                    wake: Notify::new(),
                });
                tokio::spawn(self.inner.clone().run(slot.clone()));
                slot
            })
            .clone()
    }
}

impl<R: Send + 'static> Inner<R> {
    async fn run(self: Arc<Self>, slot: Arc<Slot<R>>) {
        loop {
            let (request, instance) = self.next(&slot).await;
            self.handle(&slot, request, &instance).await;
        }
    }

    async fn next(&self, slot: &Slot<R>) -> (R, String) {
        loop {
            while slot
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none()
            {
                slot.wake.notified().await;
            }
            if !self.settle.is_zero() {
                let start = Instant::now();
                loop {
                    let seen = slot.generation.load(Ordering::SeqCst);
                    tokio::time::sleep(self.settle).await;
                    if slot.generation.load(Ordering::SeqCst) == seen
                        || start.elapsed() >= MAX_SETTLE
                    {
                        break;
                    }
                }
            }
            if let Some(next) = slot
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                return next;
            }
        }
    }

    async fn handle(&self, slot: &Slot<R>, request: R, instance: &str) {
        let mut changes = self.changes.clone();
        changes.borrow_and_update();
        let job = (self.build)(request);
        if let Err(e) = self.host.send(job.event).await {
            log::warn!("host request failed: {e}");
            self.ui.alert(instance).await;
            return;
        }
        match job.confirm {
            Confirm::Skip => {}
            Confirm::Unreadable(why) => host::warn_once(WARN_UNREADABLE, || {
                format!("{why}; sent requests can't be confirmed, so failures won't show an alert")
            }),
            Confirm::Until(done) => {
                let sent = slot.generation.load(Ordering::SeqCst);
                let overtaken = || slot.generation.load(Ordering::SeqCst) != sent;
                let deadline = Instant::now() + host::CONFIRM_TIMEOUT;
                // Stop waiting as soon as a newer request arrives: it is the
                // one whose outcome matters now.
                let confirmed = loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    tokio::select! {
                        ok = host::wait_until(&*done, &mut changes, left) => break ok,
                        _ = slot.wake.notified() => if overtaken() { break false },
                    }
                };
                if !confirmed && !overtaken() {
                    host::warn_ignored_once();
                    self.ui.alert(instance).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::fake::FakeHost;
    use crate::ui::fake::FakeUi;
    use serde_json::json;
    use std::sync::atomic::AtomicBool;

    fn dispatcher(
        host: Arc<FakeHost>,
        ui: Arc<FakeUi>,
        done: Arc<AtomicBool>,
        settle: Duration,
    ) -> Dispatcher<u8> {
        let (_tx, rx) = watch::channel(0u64);
        Dispatcher::new(host, ui, rx, settle, latest, move |v: u8| {
            let done = done.clone();
            Job {
                event: json!({ "value": v }),
                confirm: Confirm::Until(Box::new(move || done.load(Ordering::SeqCst))),
            }
        })
    }

    async fn idle() {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn an_ignored_request_alerts_once_the_timeout_passes() {
        let (host, ui) = (FakeHost::ignoring(), Arc::new(FakeUi::default()));
        let d = dispatcher(
            host.clone(),
            ui.clone(),
            Arc::new(AtomicBool::new(false)),
            Duration::ZERO,
        );
        d.submit("k", "i1", 7);
        idle().await;
        assert_eq!(host.sent(), vec![json!({ "value": 7 })]);
        assert_eq!(ui.events(), vec!["alert i1"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_confirmed_request_does_not_alert() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let host = FakeHost::new(move |_| flag.store(true, Ordering::SeqCst));
        let ui = Arc::new(FakeUi::default());
        let d = dispatcher(host.clone(), ui.clone(), done, Duration::ZERO);
        d.submit("k", "i1", 7);
        idle().await;
        assert_eq!(host.sent().len(), 1);
        assert_eq!(ui.alerts(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_send_alerts_at_once() {
        let host = FakeHost::ignoring();
        host.fail.store(true, Ordering::SeqCst);
        let ui = Arc::new(FakeUi::default());
        let d = dispatcher(
            host,
            ui.clone(),
            Arc::new(AtomicBool::new(true)),
            Duration::ZERO,
        );
        d.submit("k", "i1", 7);
        idle().await;
        assert_eq!(ui.events(), vec!["alert i1"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_settles_into_one_send_of_the_latest() {
        let host = FakeHost::ignoring();
        let ui = Arc::new(FakeUi::default());
        let d = dispatcher(
            host.clone(),
            ui,
            Arc::new(AtomicBool::new(true)),
            Duration::from_millis(100),
        );
        for v in [1, 2, 3] {
            d.submit("k", "i1", v);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        idle().await;
        assert_eq!(host.sent(), vec![json!({ "value": 3 })]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_overtaken_request_does_not_alert() {
        let host = FakeHost::ignoring();
        let ui = Arc::new(FakeUi::default());
        let d = dispatcher(
            host.clone(),
            ui.clone(),
            Arc::new(AtomicBool::new(false)),
            Duration::ZERO,
        );
        d.submit("k", "i1", 1);
        tokio::time::sleep(Duration::from_millis(200)).await;
        d.submit("k", "i2", 2);
        // The first wait ends as soon as it is overtaken, well before its
        // 1.5 s timeout.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(host.sent().len(), 2);
        idle().await;
        // Only the newest, which nothing overtook, may alert.
        assert_eq!(ui.events(), vec!["alert i2"]);
    }

    #[tokio::test(start_paused = true)]
    async fn unreadable_state_never_alerts() {
        let host = FakeHost::ignoring();
        let ui = Arc::new(FakeUi::default());
        let (_tx, rx) = watch::channel(0u64);
        let d: Dispatcher<u8> =
            Dispatcher::new(host.clone(), ui.clone(), rx, Duration::ZERO, latest, |v| {
                Job {
                    event: json!({ "value": v }),
                    confirm: Confirm::Unreadable("no settings.json".into()),
                }
            });
        d.submit("k", "i1", 1);
        idle().await;
        assert_eq!(host.sent().len(), 1);
        assert_eq!(ui.alerts(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn keys_are_independent() {
        let host = FakeHost::ignoring();
        let ui = Arc::new(FakeUi::default());
        let d = dispatcher(
            host.clone(),
            ui,
            Arc::new(AtomicBool::new(true)),
            Duration::from_millis(100),
        );
        d.submit("a", "i1", 1);
        d.submit("b", "i2", 2);
        idle().await;
        let mut sent: Vec<u64> = host
            .sent()
            .iter()
            .map(|e| e["value"].as_u64().unwrap())
            .collect();
        sent.sort();
        assert_eq!(sent, vec![1, 2]);
    }
}
