use super::{Control, Instances};
use crate::brightness::{
    BrightnessSettings, Request, apply, brightness_view, key_request, rotate_request,
    toggle_request,
};
use crate::dispatch::{Confirm, Dispatcher, Job};
use crate::host::{self, BrightnessChange, Host};
use crate::opendeck_state::OpenDeckState;
use crate::pending::Pending;
use crate::ui::Ui;
use async_trait::async_trait;
use openaction::{Action, Instance, OpenActionResult};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

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
}

#[derive(Clone)]
pub struct BrightnessAction {
    shared: Arc<Shared>,
}

impl BrightnessAction {
    /// Builds the action and starts its re-render task.
    pub fn start(
        state: OpenDeckState,
        deck_changes: watch::Receiver<u64>,
        host: Arc<dyn Host>,
        ui: Arc<dyn Ui>,
    ) -> Self {
        let action = Self::new(state, deck_changes.clone(), host, ui);
        tokio::spawn(action.clone().run_watcher(deck_changes));
        action
    }

    fn new(
        state: OpenDeckState,
        deck_changes: watch::Receiver<u64>,
        host: Arc<dyn Host>,
        ui: Arc<dyn Ui>,
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
        self.shared.dispatch.submit("", &control.id, ask);
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
        _position: (u16, u16),
        _hold: bool,
    ) -> OpenActionResult<()> {
        self.toggle(&Control::of(instance), settings).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::fake::FakeHost;
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
        action: BrightnessAction,
    }

    /// An action over a temp OpenDeck config at `brightness` (`None`: no
    /// settings.json). `applies`: whether the fake OpenDeck acts on events.
    fn rig(brightness: Option<u64>, applies: bool) -> Rig {
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
        let action =
            BrightnessAction::new(OpenDeckState::at(dir.clone()), rx, host.clone(), ui.clone());
        Rig {
            _tmp: tmp,
            host,
            ui,
            action,
        }
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
