use super::{Control, Instances, is_choices_request};
use crate::brightness::{
    BrightnessSettings, Request, apply, brightness_view, key_request, rotate_request, tap_request,
    toggle_request,
};
use crate::dispatch::{Confirm, Dispatcher, Job};
use crate::host::{self, BrightnessChange, Host};
use crate::opendeck_state::OpenDeckState;
use crate::pending::Pending;
use crate::schedule::{GLOBAL_KEY, LocalClock, Schedule};
use crate::ui::Ui;
use async_trait::async_trait;
use openaction::{Action, Instance, OpenActionResult};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The dispatcher coalescing key: OpenDeck has one brightness for every
/// device, so every Brightness control shares one key.
const KEY: &str = "brightness";
/// The dispatcher "instance" of scheduled changes (no control to alert).
const SCHEDULE: &str = "schedule";
/// The schedule timer wakes at least this often and re-reads the clock, so
/// a suspend (which stops the monotonic timer) delays a change by at most
/// this much, and a wall-clock jump is noticed.
const SCHEDULE_RECHECK: Duration = Duration::from_secs(60);
use tokio::sync::watch;

/// Where the plugin-wide schedule is kept: OpenDeck's global settings in
/// the plugin, a recorder in tests.
#[async_trait]
pub trait ScheduleStore: Send + Sync {
    async fn save(&self, schedule: &Schedule);
}

pub struct GlobalSettingsStore;

#[async_trait]
impl ScheduleStore for GlobalSettingsStore {
    async fn save(&self, schedule: &Schedule) {
        // The utilities plugin keeps nothing else in its global settings.
        let global = json!({ GLOBAL_KEY: schedule.to_value() });
        if let Err(e) = openaction::set_global_settings(global).await {
            log::warn!("saving the brightness schedule failed: {e}");
        }
    }
}

/// A dial spin settles into one `opendeck` launch (perf review).
const SETTLE: Duration = Duration::from_millis(120);

/// What is sent to OpenDeck. With the brightness known every gesture is an
/// absolute target; without it only relative steps make sense, and those
/// add up while waiting to be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    Set(u8),
    Step(i16),
}

fn merge(older: Ask, newer: Ask) -> Ask {
    match (older, newer) {
        (Ask::Step(a), Ask::Step(b)) => Ask::Step((a + b).clamp(-100, 100)),
        (_, newer) => newer,
    }
}

impl Ask {
    fn from_request(r: &Request) -> Self {
        let amount = i16::from(r.value);
        match r.change {
            BrightnessChange::Set => Ask::Set(r.value),
            BrightnessChange::Increase => Ask::Step(amount),
            BrightnessChange::Decrease => Ask::Step(-amount),
        }
    }

    /// The event, and what OpenDeck's brightness should then be given
    /// `before` (`None` when it can't be read).
    fn event(self, before: Option<u8>) -> (serde_json::Value, Option<u8>) {
        let (change, value) = match self {
            Ask::Set(v) => (BrightnessChange::Set, v),
            Ask::Step(n) if n >= 0 => (BrightnessChange::Increase, n.unsigned_abs() as u8),
            Ask::Step(n) => (BrightnessChange::Decrease, n.unsigned_abs() as u8),
        };
        (
            host::brightness_event(change, value),
            before.map(|b| apply(b, change, value)),
        )
    }
}

/// How to confirm a request (spec §6.5): nothing to check when the value
/// can't change; can't check when the current value is unknown.
fn confirm_rule(state: &OpenDeckState, before: Option<u8>, expected: Option<u8>) -> Confirm {
    match (before, expected) {
        (None, _) | (_, None) => Confirm::Unreadable(state.brightness_unreadable_reason()),
        (Some(b), Some(e)) if b == e => Confirm::Skip,
        (Some(_), Some(e)) => {
            let state = state.clone();
            Confirm::Until(Box::new(move || state.brightness() == Some(e)))
        }
    }
}

struct Shared {
    state: OpenDeckState,
    ui: Arc<dyn Ui>,
    instances: Instances<()>,
    /// The brightness last asked for, shown until OpenDeck saves it.
    pending: Pending<u8>,
    dispatch: Dispatcher<Ask>,
    clock: Arc<dyn LocalClock>,
    store: Arc<dyn ScheduleStore>,
    schedule: watch::Sender<Schedule>,
    /// The stored schedule has been read once (start-up catch-up done).
    loaded: AtomicBool,
}

#[derive(Clone)]
pub struct BrightnessAction {
    shared: Arc<Shared>,
}

impl BrightnessAction {
    /// Builds the action and starts its re-render task.
    /// Builds the action and starts its re-render and schedule tasks.
    pub fn start(
        state: OpenDeckState,
        deck_changes: watch::Receiver<u64>,
        host: Arc<dyn Host>,
        ui: Arc<dyn Ui>,
        clock: Arc<dyn LocalClock>,
        store: Arc<dyn ScheduleStore>,
    ) -> Self {
        let action = Self::new(state, deck_changes.clone(), host, ui, clock, store);
        tokio::spawn(action.clone().run_watcher(deck_changes));
        tokio::spawn(action.clone().run_schedule());
        action
    }

    fn new(
        state: OpenDeckState,
        deck_changes: watch::Receiver<u64>,
        host: Arc<dyn Host>,
        ui: Arc<dyn Ui>,
        clock: Arc<dyn LocalClock>,
        store: Arc<dyn ScheduleStore>,
    ) -> Self {
        let reader = state.clone();
        let dispatch = Dispatcher::new(
            host,
            ui.clone(),
            deck_changes,
            SETTLE,
            merge,
            move |s: Ask| {
                let before = reader.brightness();
                let (event, expected) = s.event(before);
                Job {
                    event,
                    confirm: confirm_rule(&reader, before, expected),
                }
            },
        );
        Self {
            shared: Arc::new(Shared {
                state,
                ui,
                instances: Instances::new(),
                pending: Pending::new(),
                dispatch,
                clock,
                store,
                schedule: watch::Sender::new(Schedule::default()),
                loaded: AtomicBool::new(false),
            }),
        }
    }

    /// OpenDeck's brightness, or the one just asked for if it hasn't landed.
    fn current(&self) -> Option<u8> {
        self.shared.pending.resolve(self.shared.state.brightness())
    }

    async fn render(&self, control: &Control) {
        self.shared
            .ui
            .show_level(control, &brightness_view(self.current()))
            .await;
    }

    async fn render_all(&self) {
        for (control, ()) in self.shared.instances.snapshot() {
            self.render(&control).await;
        }
    }

    async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        while changes.changed().await.is_ok() {
            self.render_all().await;
        }
    }

    /// Shows a request at once when its outcome is known, then queues it;
    /// the event loop never waits on the `opendeck` command.
    async fn send(&self, control: &Control, request: Request) {
        let ask = match request.expected {
            Some(target) => {
                self.shared.pending.set(target);
                self.render_all().await;
                Ask::Set(target)
            }
            None => Ask::from_request(&request),
        };
        self.shared.dispatch.submit(KEY, &control.id, ask);
    }

    /// A scheduled change: shown at once, then sent like a dial's target.
    async fn scheduled(&self, value: u8) {
        log::info!("brightness schedule: setting {value}%");
        self.shared.pending.set(value);
        self.render_all().await;
        self.shared.dispatch.submit(KEY, SCHEDULE, Ask::Set(value));
    }

    /// The schedule read from OpenDeck's global settings. The first time
    /// (start-up) the value of the current period is applied at once, so a
    /// plugin started at 01:00 is at the night value; later reads only
    /// replace the schedule.
    pub async fn load_schedule(&self, schedule: Schedule) {
        self.shared.schedule.send_if_modified(|s| {
            let changed = *s != schedule;
            *s = schedule;
            changed
        });
        if self.shared.loaded.swap(true, Ordering::SeqCst) {
            return;
        }
        let clock = &self.shared.clock;
        if let Some(value) = schedule.value_at(clock.now(), |t, d, m| clock.at(t, d, m)) {
            self.scheduled(value).await;
        }
    }

    /// A schedule edited on the page: stored, and the next change re-timed.
    /// Nothing is applied now; the next boundary applies it.
    async fn edit_schedule(&self, schedule: Schedule) {
        self.shared.loaded.store(true, Ordering::SeqCst);
        self.shared.schedule.send_replace(schedule);
        self.shared.store.save(&schedule).await;
    }

    async fn send_schedule(&self, control: &Control) {
        let schedule = *self.shared.schedule.borrow();
        self.shared
            .ui
            .send_to_pi(
                &control.id,
                json!({ "event": "brightnessSchedule", "schedule": schedule.to_value() }),
            )
            .await;
    }

    async fn page_message(&self, control: &Control, payload: &Value) {
        if is_choices_request(payload) {
            self.send_schedule(control).await;
        } else if payload["event"] == "setSchedule" {
            self.edit_schedule(Schedule::from_value(&payload["schedule"]))
                .await;
        }
    }

    /// Applies each scheduled change at its local time; a manual change in
    /// between is left alone until the next one. Re-times on every edit.
    /// Boundaries missed while asleep apply once, as the current period's
    /// value.
    async fn run_schedule(self) {
        let mut rx = self.shared.schedule.subscribe();
        let clock = self.shared.clock.clone();
        let at = |t, d, m| clock.at(t, d, m);
        let mut floor = clock.now();
        loop {
            let schedule = *rx.borrow_and_update();
            let Some((due, _)) = schedule.next_after(floor, at) else {
                if rx.changed().await.is_err() {
                    return;
                }
                floor = clock.now();
                continue;
            };
            let now = clock.now();
            if due <= now {
                if let Some(value) = schedule.value_at(now, at) {
                    self.scheduled(value).await;
                }
                floor = now;
                continue;
            }
            let wait = Duration::from_secs((due - now).unsigned_abs()).min(SCHEDULE_RECHECK);
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                changed = rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    floor = clock.now();
                }
            }
        }
    }

    async fn appear(&self, control: &Control) {
        self.shared.ui.forget(&control.id);
        self.shared.instances.insert(control, &());
        self.render(control).await;
    }

    fn disappear(&self, control: &Control) {
        self.shared.instances.remove(&control.id);
        self.shared.ui.forget(&control.id);
    }

    async fn key(&self, control: &Control, settings: &BrightnessSettings) {
        self.send(control, key_request(self.current(), settings))
            .await;
    }

    async fn rotate(&self, control: &Control, settings: &BrightnessSettings, ticks: i16) {
        if let Some(request) = rotate_request(self.current(), ticks, settings) {
            self.send(control, request).await;
        }
    }

    async fn toggle(&self, control: &Control, settings: &BrightnessSettings) {
        self.send(control, toggle_request(self.current(), settings))
            .await;
    }

    async fn tap(&self, control: &Control, settings: &BrightnessSettings, x: u16, hold: bool) {
        self.send(control, tap_request(self.current(), settings, x, hold))
            .await;
    }
}

#[async_trait]
impl Action for BrightnessAction {
    const UUID: &'static str = "com.jfms7s.utilities.brightness";
    type Settings = BrightnessSettings;

    async fn will_appear(
        &self,
        instance: &Instance,
        _settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.appear(&Control::of(instance)).await;
        Ok(())
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        _settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.disappear(&Control::of(instance));
        Ok(())
    }

    async fn key_up(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.key(&Control::of(instance), settings).await;
        Ok(())
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
        ticks: i16,
        _pressed: bool,
    ) -> OpenActionResult<()> {
        self.rotate(&Control::of(instance), settings, ticks).await;
        Ok(())
    }

    async fn dial_up(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.toggle(&Control::of(instance), settings).await;
        Ok(())
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
        position: (u16, u16),
        hold: bool,
    ) -> OpenActionResult<()> {
        self.tap(&Control::of(instance), settings, position.0, hold)
            .await;
        Ok(())
    }

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        _settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.send_schedule(&Control::of(instance)).await;
        Ok(())
    }

    async fn send_to_plugin(
        &self,
        instance: &Instance,
        _settings: &BrightnessSettings,
        payload: &Value,
    ) -> OpenActionResult<()> {
        self.page_message(&Control::of(instance), payload).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::fake::FakeHost;
    use crate::schedule::test_support::FakeZone;
    use crate::ui::fake::FakeUi;
    use serde_json::json;
    use std::path::Path;

    fn write_brightness(dir: &Path, v: u64) {
        std::fs::write(
            dir.join("settings.json"),
            json!({ "brightness": v }).to_string(),
        )
        .unwrap();
    }

    struct Rig {
        _tmp: tempfile::TempDir,
        host: Arc<FakeHost>,
        ui: Arc<FakeUi>,
        store: Arc<FakeStore>,
        action: BrightnessAction,
    }

    #[derive(Default)]
    struct FakeStore {
        saved: std::sync::Mutex<Vec<Schedule>>,
    }

    #[async_trait]
    impl ScheduleStore for FakeStore {
        async fn save(&self, schedule: &Schedule) {
            self.saved.lock().unwrap().push(*schedule);
        }
    }

    fn rig(brightness: Option<u64>, applies: bool) -> Rig {
        rig_at(brightness, applies, 0)
    }

    const H: i64 = 3600;

    /// An action over a temp OpenDeck config at `brightness` (`None`: no
    /// settings.json). `applies`: whether the fake OpenDeck acts on events.
    /// Its clock is a UTC day starting at `start` (seconds), advancing with
    /// tokio's (paused) clock.
    fn rig_at(brightness: Option<u64>, applies: bool, start: i64) -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        if let Some(v) = brightness {
            write_brightness(&dir, v);
        }
        let target = dir.clone();
        let host = FakeHost::new(move |e| {
            if applies && e["action"] == "set" {
                write_brightness(&target, e["value"].as_u64().unwrap());
            }
        });
        let ui = Arc::new(FakeUi::default());
        let (_tx, rx) = watch::channel(0u64);
        let began = tokio::time::Instant::now();
        let clock = FakeZone::fixed(move || start + began.elapsed().as_secs() as i64);
        let store = Arc::new(FakeStore::default());
        let action = BrightnessAction::new(
            OpenDeckState::at(dir.clone()),
            rx,
            host.clone(),
            ui.clone(),
            Arc::new(clock),
            store.clone(),
        );
        Rig {
            _tmp: tmp,
            host,
            ui,
            store,
            action,
        }
    }

    fn night_and_morning() -> Schedule {
        Schedule::from_value(&json!([
            { "time": "23:00", "value": 10 },
            { "time": "08:00", "value": 70 }
        ]))
    }

    fn set(value: u64) -> Value {
        json!({"event": "deviceBrightness", "action": "set", "value": value})
    }

    /// At 23:00 one Set with the scheduled value goes out (one schedule for
    /// the whole plugin, so however many Brightness controls are placed).
    #[tokio::test(start_paused = true)]
    async fn a_boundary_sends_one_set_with_the_scheduled_value() {
        let r = rig_at(Some(50), true, 22 * H + 59 * 60);
        r.action.appear(&Control::dial("a")).await;
        r.action.appear(&Control::key("b")).await;
        r.action.edit_schedule(night_and_morning()).await;
        tokio::spawn(r.action.clone().run_schedule());
        tokio::time::sleep(Duration::from_secs(50)).await;
        assert!(r.host.sent().is_empty(), "not before 23:00");
        tokio::time::sleep(Duration::from_secs(20)).await;
        idle().await;
        assert_eq!(r.host.sent(), vec![set(10)]);
        assert_eq!(r.ui.alerts(), 0);
        // Nothing more until 08:00 (now 23:00:15).
        tokio::time::sleep(Duration::from_secs(8 * 3600 + 59 * 60)).await;
        assert_eq!(r.host.sent(), vec![set(10)]);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(r.host.sent(), vec![set(10), set(70)]);
    }

    /// Decision 4: a manual change after a boundary stays until the next.
    #[tokio::test(start_paused = true)]
    async fn a_manual_change_is_left_alone_until_the_next_boundary() {
        let r = rig_at(Some(50), true, 22 * H + 59 * 60);
        let c = Control::dial("d");
        r.action.appear(&c).await;
        r.action.edit_schedule(night_and_morning()).await;
        tokio::spawn(r.action.clone().run_schedule());
        tokio::time::sleep(Duration::from_secs(30 * 60)).await;
        r.action.rotate(&c, &BrightnessSettings::default(), 1).await;
        idle().await;
        assert_eq!(r.host.sent(), vec![set(10), set(15)]);
        // 07:59:05 the next morning (now 23:29:05): still 15.
        tokio::time::sleep(Duration::from_secs(8 * 3600 + 30 * 60)).await;
        assert_eq!(r.host.sent().len(), 2);
        tokio::time::sleep(Duration::from_secs(120)).await;
        assert_eq!(r.host.sent().last(), Some(&set(70)));
    }

    /// Decision 3: started at 01:00, the night value applies at once; a
    /// later read of the same settings applies nothing.
    #[tokio::test(start_paused = true)]
    async fn the_first_load_applies_the_current_period() {
        let r = rig_at(Some(50), true, H);
        r.action.load_schedule(night_and_morning()).await;
        idle().await;
        assert_eq!(r.host.sent(), vec![set(10)]);
        r.action.load_schedule(night_and_morning()).await;
        idle().await;
        assert_eq!(r.host.sent().len(), 1);
        // Nothing stored or scheduled: no catch-up.
        let r = rig_at(Some(50), true, H);
        r.action.load_schedule(Schedule::default()).await;
        idle().await;
        assert!(r.host.sent().is_empty());
    }

    /// The page reads the schedule and edits it; an edit is stored and
    /// changes nothing until its next boundary.
    #[tokio::test(start_paused = true)]
    async fn the_page_reads_and_stores_the_schedule() {
        let r = rig_at(Some(50), true, H);
        let c = Control::dial("d");
        r.action
            .page_message(&c, &json!({ "event": "requestChoices" }))
            .await;
        assert_eq!(r.ui.events(), vec!["pi d brightnessSchedule"]);
        let page = night_and_morning().to_value();
        r.action
            .page_message(&c, &json!({ "event": "setSchedule", "schedule": page }))
            .await;
        idle().await;
        assert_eq!(*r.store.saved.lock().unwrap(), vec![night_and_morning()]);
        assert!(r.host.sent().is_empty(), "an edit applies nothing at once");
        // A start-up read arriving after an edit doesn't catch up either.
        r.action.load_schedule(night_and_morning()).await;
        idle().await;
        assert!(r.host.sent().is_empty());
    }

    async fn idle() {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    #[test]
    fn steps_add_up_and_a_target_wins() {
        assert_eq!(merge(Ask::Step(5), Ask::Step(-15)), Ask::Step(-10));
        assert_eq!(merge(Ask::Step(90), Ask::Step(90)), Ask::Step(100));
        assert_eq!(merge(Ask::Step(5), Ask::Set(30)), Ask::Set(30));
        assert_eq!(merge(Ask::Set(30), Ask::Set(40)), Ask::Set(40));
    }

    #[test]
    fn confirmation_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let state = OpenDeckState::at(tmp.path().to_path_buf());
        assert!(matches!(
            confirm_rule(&state, Some(50), Some(50)),
            Confirm::Skip
        ));
        assert!(matches!(
            confirm_rule(&state, Some(50), Some(55)),
            Confirm::Until(_)
        ));
        assert!(matches!(
            confirm_rule(&state, None, None),
            Confirm::Unreadable(_)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_dial_spin_shows_at_once_and_sends_one_target() {
        let r = rig(Some(50), true);
        let c = Control::dial("d");
        r.action.appear(&c).await;
        for _ in 0..3 {
            r.action.rotate(&c, &BrightnessSettings::default(), 1).await;
        }
        assert_eq!(r.ui.events().last().unwrap(), "level d 65%");
        idle().await;
        assert_eq!(
            r.host.sent(),
            vec![json!({"event": "deviceBrightness", "action": "set", "value": 65})]
        );
        assert_eq!(r.ui.alerts(), 0);
    }

    /// QA mutation 2: a request OpenDeck ignores must alert.
    #[tokio::test(start_paused = true)]
    async fn an_ignored_request_alerts_the_key() {
        let r = rig(Some(50), false);
        let c = Control::key("k");
        r.action.appear(&c).await;
        r.action.key(&c, &BrightnessSettings::default()).await;
        idle().await;
        assert_eq!(r.host.sent().len(), 1);
        assert_eq!(r.ui.alerts(), 1);
    }

    /// Tech-decisions review: unreadable state is not "OpenDeck ignored it".
    #[tokio::test(start_paused = true)]
    async fn unknown_brightness_sends_steps_without_alerting() {
        let r = rig(None, false);
        let c = Control::dial("d");
        r.action.appear(&c).await;
        r.action.rotate(&c, &BrightnessSettings::default(), 2).await;
        r.action
            .rotate(&c, &BrightnessSettings::default(), -1)
            .await;
        idle().await;
        assert_eq!(
            r.host.sent(),
            vec![json!({"event": "deviceBrightness", "action": "increase", "value": 5})]
        );
        assert_eq!(r.ui.alerts(), 0);
        assert!(r.ui.events().contains(&"level d unknown".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_that_cannot_change_anything_is_not_confirmed() {
        let r = rig(Some(100), false);
        let c = Control::key("k");
        r.action.appear(&c).await;
        r.action.key(&c, &BrightnessSettings::default()).await;
        idle().await;
        assert_eq!(r.host.sent().len(), 1);
        assert_eq!(r.ui.alerts(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_send_alerts() {
        let r = rig(Some(50), true);
        r.host.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        let c = Control::key("k");
        r.action.appear(&c).await;
        r.action.key(&c, &BrightnessSettings::default()).await;
        idle().await;
        assert_eq!(r.ui.alerts(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_hidden_control_is_no_longer_rendered() {
        let r = rig(Some(50), true);
        let (a, b) = (Control::key("a"), Control::key("b"));
        r.action.appear(&a).await;
        r.action.appear(&b).await;
        r.action.disappear(&a);
        r.ui.clear();
        r.action.render_all().await;
        assert_eq!(r.ui.events(), vec!["level b 50%"]);
    }
}
