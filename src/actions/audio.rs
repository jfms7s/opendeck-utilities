use super::{Control, Instances, is_choices_request};
use crate::audio::backend::{AudioBackend, BackendError};
use crate::audio::exec::{apply_locally, execute};
use crate::audio::gesture::{self, Operation};
use crate::audio::model::Snapshot;
use crate::audio::pi::choices;
use crate::audio::settings::{AudioSettings, OpKind};
use crate::audio::target::resolve;
use crate::audio::view::{audio_view, error_view};
use crate::ui::Ui;
use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use openaction::{Action, Instance, OpenActionResult};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock, watch};

/// Coalesces a burst of `pactl subscribe` events (one dial turn can emit
/// several) into one refresh.
pub const DEBOUNCE: Duration = Duration::from_millis(50);

/// One operation waiting for the worker.
struct Job {
    control: Control,
    settings: AudioSettings,
    op: Operation,
    /// Push-to-talk's re-mute: retried once after a fresh read if it fails,
    /// because failing leaves the microphone live.
    must: bool,
}

/// Operations run in order on one worker task, off the event loop. Turning
/// a dial queues volume steps faster than `pactl` applies them; consecutive
/// steps on the same control are summed into one call.
#[derive(Default)]
struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    ready: Notify,
}

impl Queue {
    fn push(&self, job: Job) {
        if job.op == Operation::None {
            return;
        }
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = jobs.back_mut()
            && last.control.id == job.control.id
            && let (Operation::AdjustVolume(a), Operation::AdjustVolume(b)) = (&last.op, &job.op)
        {
            last.op = Operation::AdjustVolume(a.saturating_add(*b));
            last.settings = job.settings;
            return;
        }
        jobs.push_back(job);
        drop(jobs);
        self.ready.notify_one();
    }

    fn pop(&self) -> Option<Job> {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }
}

struct Shared {
    backend: Arc<dyn AudioBackend>,
    ui: Arc<dyn Ui>,
    /// Last snapshot, or a short message for why there is none.
    snapshot: RwLock<Result<Snapshot, String>>,
    /// Bumped by every local update of `snapshot`, so a refresh that read
    /// `pactl` before that update can't roll the shown level back.
    local_writes: AtomicU64,
    /// Changes were skipped while no Audio control was visible.
    stale: AtomicBool,
    /// Wakes the watcher (a control appeared while the snapshot was stale).
    kick: Notify,
    instances: Instances<AudioSettings>,
    /// When each currently-held key/dial went down.
    pressed: DashMap<String, Instant>,
    /// Dials turned while held: their release performs nothing, so a
    /// long "hold + turn" never fires the long-press operation.
    consumed: DashSet<String>,
    queue: Queue,
}

#[derive(Clone)]
pub struct AudioAction {
    shared: Arc<Shared>,
}

fn short_error(e: &BackendError) -> String {
    match e {
        BackendError::NotInstalled => "pactl not found".to_string(),
        _ => "audio error".to_string(),
    }
}

impl AudioAction {
    /// Builds the action, subscribes to audio changes and starts the
    /// re-render and operation tasks.
    pub fn start(backend: Arc<dyn AudioBackend>, ui: Arc<dyn Ui>) -> Self {
        let (tx, rx) = watch::channel(0u64);
        backend.subscribe(tx);
        let action = Self::new(backend, ui);
        tokio::spawn(action.clone().run_watcher(rx));
        tokio::spawn(action.clone().run_jobs());
        action
    }

    fn new(backend: Arc<dyn AudioBackend>, ui: Arc<dyn Ui>) -> Self {
        Self {
            shared: Arc::new(Shared {
                backend,
                ui,
                snapshot: RwLock::new(Err("loading".to_string())),
                local_writes: AtomicU64::new(0),
                stale: AtomicBool::new(false),
                kick: Notify::new(),
                instances: Instances::new(),
                pressed: DashMap::new(),
                consumed: DashSet::new(),
                queue: Queue::default(),
            }),
        }
    }

    async fn read(&self) -> Result<Snapshot, String> {
        self.shared.backend.snapshot().await.map_err(|e| {
            log::warn!("audio snapshot failed: {e}");
            short_error(&e)
        })
    }

    async fn refresh(&self) {
        let writes = self.shared.local_writes.load(Ordering::SeqCst);
        let result = self.read().await;
        let mut slot = self.shared.snapshot.write().await;
        if result.is_ok() && self.shared.local_writes.load(Ordering::SeqCst) != writes {
            // Read before a local update landed; the change it made
            // triggers another refresh.
            return;
        }
        *slot = result;
        self.shared.stale.store(false, Ordering::SeqCst);
    }

    /// The cached snapshot; a fresh read only when there is none yet.
    async fn current(&self) -> Result<Snapshot, String> {
        if let Ok(snap) = &*self.shared.snapshot.read().await {
            return Ok(snap.clone());
        }
        self.refresh().await;
        self.shared.snapshot.read().await.clone()
    }

    async fn render(&self, control: &Control, settings: &AudioSettings) {
        let view = match &*self.shared.snapshot.read().await {
            Ok(snap) => audio_view(
                &resolve(settings.target, &settings.target_name, snap),
                settings,
            ),
            Err(message) => error_view(settings, message),
        };
        self.shared.ui.show_level(control, &view).await;
    }

    async fn render_all(&self) {
        for (control, settings) in self.shared.instances.snapshot() {
            self.render(&control, &settings).await;
        }
    }

    /// Runs forever: refresh + re-render on every (debounced) change tick.
    /// With no Audio control visible it only marks the snapshot stale.
    async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        self.refresh().await;
        self.render_all().await;
        let mut subscribed = true;
        loop {
            tokio::select! {
                changed = changes.changed(), if subscribed => {
                    // The subscriber gave up (pactl missing); only kicks remain.
                    subscribed = changed.is_ok();
                    if !subscribed {
                        continue;
                    }
                }
                _ = self.shared.kick.notified() => {}
            }
            tokio::time::sleep(DEBOUNCE).await;
            changes.mark_unchanged();
            if self.shared.instances.is_empty() {
                self.shared.stale.store(true, Ordering::SeqCst);
                continue;
            }
            self.refresh().await;
            self.render_all().await;
        }
    }

    async fn run_jobs(self) {
        loop {
            match self.shared.queue.pop() {
                Some(job) => self.run_job(job).await,
                None => self.shared.queue.ready.notified().await,
            }
        }
    }

    /// Runs every queued job now (tests drive the worker this way).
    #[cfg(test)]
    async fn drain(&self) {
        while let Some(job) = self.shared.queue.pop() {
            self.run_job(job).await;
        }
    }

    async fn run_job(&self, job: Job) {
        let Job {
            control,
            settings,
            op,
            must,
        } = job;
        let mut result = self.execute(&settings, &op).await;
        if result.is_err() && must {
            // Push-to-talk must not leave the mic live: re-read and retry.
            self.refresh().await;
            result = self.execute(&settings, &op).await;
            if let Err(e) = &result {
                log::error!(
                    "push-to-talk could not mute {:?} again: {e}",
                    settings.target
                );
            }
        }
        if let Err(e) = result {
            log::warn!("audio {op:?} failed: {e}");
            self.shared.ui.alert(&control.id).await;
            // Show what actually happened (e.g. half of an app's streams).
            self.refresh().await;
        }
        self.render_all().await;
    }

    /// Carries `op` out against the cached snapshot and updates the cache
    /// to match, so the next tick needs no `pactl` read.
    async fn execute(&self, settings: &AudioSettings, op: &Operation) -> Result<(), String> {
        let snap = self.current().await?;
        let resolved = resolve(settings.target, &settings.target_name, &snap);
        execute(self.shared.backend.as_ref(), &snap, &resolved, settings, op)
            .await
            .map_err(|e| e.to_string())?;
        let predicted = {
            let mut slot = self.shared.snapshot.write().await;
            let predicted = match &mut *slot {
                Ok(cached) => apply_locally(cached, &resolved, settings, op),
                Err(_) => false,
            };
            self.shared.local_writes.fetch_add(1, Ordering::SeqCst);
            predicted
        };
        if !predicted {
            self.refresh().await;
        }
        Ok(())
    }

    fn enqueue(&self, control: &Control, settings: &AudioSettings, op: Operation, must: bool) {
        self.shared.queue.push(Job {
            control: control.clone(),
            settings: settings.clone(),
            op,
            must,
        });
    }

    async fn send_choices(&self, control: &Control, settings: &AudioSettings) {
        let (snap, error) = match self.current().await {
            Ok(snap) => (snap, None),
            Err(message) => (Snapshot::default(), Some(message)),
        };
        self.shared
            .ui
            .send_to_pi(
                &control.id,
                choices(&snap, settings, control.controller, error.as_deref()),
            )
            .await;
    }

    fn press_started(&self, id: &str) {
        self.shared.consumed.remove(id);
        self.shared.pressed.insert(id.to_string(), Instant::now());
    }

    /// A turn while the dial is held turns the press into a "hold + turn".
    fn press_rotated(&self, id: &str, pressed: bool) {
        if pressed && self.shared.pressed.remove(id).is_some() {
            self.shared.consumed.insert(id.to_string());
        }
    }

    /// How long the press was held, or `None` when a turn consumed it.
    fn press_released(&self, id: &str) -> Option<Duration> {
        if self.shared.consumed.remove(id).is_some() {
            return None;
        }
        Some(
            self.shared
                .pressed
                .remove(id)
                .map(|(_, t)| t.elapsed())
                .unwrap_or_default(),
        )
    }

    fn is_held(&self, id: &str) -> bool {
        self.shared.pressed.contains_key(id) || self.shared.consumed.contains(id)
    }

    async fn appear(&self, control: &Control, settings: &AudioSettings) {
        self.shared.ui.forget(&control.id);
        self.shared.instances.insert(control, settings);
        if self.shared.stale.load(Ordering::SeqCst) {
            self.shared.kick.notify_one();
        }
        self.render(control, settings).await;
    }

    /// Forgets the control. A push-to-talk control that disappears while
    /// held (profile switch, device unplugged) gets no release event, so it
    /// re-mutes now rather than leave the microphone live.
    fn disappear(&self, control: &Control, settings: &AudioSettings) {
        let e = gesture::effective(settings, control.controller);
        if e.press.op == OpKind::PushToTalk && self.is_held(&control.id) {
            self.enqueue(control, settings, Operation::SetMute(true), true);
        }
        self.shared.instances.remove(&control.id);
        self.shared.pressed.remove(&control.id);
        self.shared.consumed.remove(&control.id);
        self.shared.ui.forget(&control.id);
    }

    async fn settings_changed(&self, control: &Control, settings: &AudioSettings) {
        self.shared.instances.insert(control, settings);
        self.render(control, settings).await;
    }

    fn press_down(&self, control: &Control, settings: &AudioSettings) {
        self.press_started(&control.id);
        let e = gesture::effective(settings, control.controller);
        self.enqueue(control, settings, gesture::down_operation(&e), false);
    }

    fn press_up(&self, control: &Control, settings: &AudioSettings) {
        let held = self.press_released(&control.id);
        let e = gesture::effective(settings, control.controller);
        let must = e.press.op == OpKind::PushToTalk;
        let op = gesture::release_operation(&e, held, settings.step());
        self.enqueue(control, settings, op, must);
    }

    fn rotate(&self, control: &Control, settings: &AudioSettings, ticks: i16, pressed: bool) {
        self.press_rotated(&control.id, pressed);
        let op = gesture::rotate_operation(settings.rotate, ticks, settings.step());
        self.enqueue(control, settings, op, false);
    }

    fn tap(&self, control: &Control, settings: &AudioSettings) {
        let e = gesture::effective(settings, control.controller);
        let op = gesture::gesture_operation(&e.touch_tap, settings.step());
        self.enqueue(control, settings, op, false);
    }
}

#[async_trait]
impl Action for AudioAction {
    const UUID: &'static str = "com.jfms7s.utilities.audio";
    type Settings = AudioSettings;

    async fn will_appear(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.appear(&Control::of(instance), settings).await;
        Ok(())
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.disappear(&Control::of(instance), settings);
        Ok(())
    }

    async fn did_receive_settings(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.settings_changed(&Control::of(instance), settings)
            .await;
        Ok(())
    }

    async fn key_down(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.press_down(&Control::of(instance), settings);
        Ok(())
    }

    async fn key_up(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_up(&Control::of(instance), settings);
        Ok(())
    }

    async fn dial_down(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.press_down(&Control::of(instance), settings);
        Ok(())
    }

    async fn dial_up(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_up(&Control::of(instance), settings);
        Ok(())
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
        ticks: i16,
        pressed: bool,
    ) -> OpenActionResult<()> {
        self.rotate(&Control::of(instance), settings, ticks, pressed);
        Ok(())
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
        _position: (u16, u16),
        _hold: bool,
    ) -> OpenActionResult<()> {
        self.tap(&Control::of(instance), settings);
        Ok(())
    }

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.send_choices(&Control::of(instance), settings).await;
        Ok(())
    }

    async fn send_to_plugin(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
        payload: &serde_json::Value,
    ) -> OpenActionResult<()> {
        if is_choices_request(payload) {
            self.send_choices(&Control::of(instance), settings).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::FakeBackend;
    use crate::audio::model::test_support::fixture_snapshot;
    use crate::audio::settings::GestureSetting;
    use crate::ui::fake::FakeUi;

    const RAZER: &str = "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo";
    const MIC: &str = "alsa_input.usb-webcam-02.mono-fallback";

    struct Rig {
        backend: Arc<FakeBackend>,
        ui: Arc<FakeUi>,
        action: AudioAction,
    }

    fn rig() -> Rig {
        let backend = Arc::new(FakeBackend::new(fixture_snapshot()));
        let ui = Arc::new(FakeUi::default());
        let action = AudioAction::new(backend.clone(), ui.clone());
        Rig {
            backend,
            ui,
            action,
        }
    }

    fn ptt_mic() -> AudioSettings {
        AudioSettings {
            target: crate::audio::target::TargetKind::DefaultInput,
            press: Some(GestureSetting {
                op: OpKind::PushToTalk,
                ..GestureSetting::default()
            }),
            ..AudioSettings::default()
        }
    }

    #[test]
    fn release_without_press_counts_as_a_tap() {
        assert_eq!(
            rig().action.press_released("never-pressed"),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn rotating_without_holding_keeps_the_press() {
        let a = rig().action;
        a.press_started("d");
        a.press_rotated("d", false);
        assert!(a.press_released("d").is_some());
    }

    #[test]
    fn consumed_mark_is_cleared_after_release() {
        let a = rig().action;
        a.press_started("d");
        a.press_rotated("d", true);
        a.press_released("d");
        assert!(a.shared.consumed.is_empty());
        a.press_started("d");
        assert!(a.press_released("d").is_some());
    }

    /// QA mutation 1 (the d0aad42 fix): hold + turn + release fires only
    /// the volume change, never the press or long-press operation.
    #[tokio::test]
    async fn hold_turn_release_only_changes_the_volume() {
        let r = rig();
        let c = Control::dial("d");
        let s = AudioSettings::default();
        r.action.press_down(&c, &s);
        r.action.rotate(&c, &s, 1, true);
        r.action.press_up(&c, &s);
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![format!("volume Sink({RAZER:?}) 52")]
        );
    }

    #[tokio::test]
    async fn a_short_press_toggles_mute_and_a_long_one_switches_device() {
        let r = rig();
        let c = Control::dial("d");
        let s = AudioSettings::default();
        r.action.press_down(&c, &s);
        r.action.press_up(&c, &s);
        r.action.drain().await;
        r.action
            .shared
            .pressed
            .insert("d".into(), Instant::now() - Duration::from_secs(1));
        r.action.press_up(&c, &s);
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("mute Sink({RAZER:?}) Toggle"),
                "default Output alsa_output.pci-0000_2f_00.4.iec958-stereo".to_string(),
            ]
        );
    }

    /// Performance review: a fast spin is summed into one `pactl` call and
    /// read from the cache, not one snapshot per tick.
    #[tokio::test]
    async fn queued_ticks_become_one_call_and_build_on_each_other() {
        let r = rig();
        let c = Control::dial("d");
        let s = AudioSettings::default();
        r.action.refresh().await;
        for _ in 0..3 {
            r.action.rotate(&c, &s, 1, false);
        }
        r.action.drain().await;
        r.action.rotate(&c, &s, 1, false);
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("volume Sink({RAZER:?}) 62"),
                format!("volume Sink({RAZER:?}) 67"),
            ]
        );
    }

    #[tokio::test]
    async fn mute_and_volume_steps_keep_their_order() {
        let r = rig();
        let c = Control::dial("d");
        let s = AudioSettings::default();
        r.action.rotate(&c, &s, 1, false);
        r.action.tap(&c, &s);
        r.action.rotate(&c, &s, 1, false);
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("volume Sink({RAZER:?}) 52"),
                format!("mute Sink({RAZER:?}) Toggle"),
                format!("volume Sink({RAZER:?}) 57"),
            ]
        );
    }

    /// QA mutation 4 / PTT lifecycle: a held push-to-talk control that
    /// disappears re-mutes, and its press state is forgotten.
    #[tokio::test]
    async fn a_held_push_to_talk_key_that_disappears_mutes_again() {
        let r = rig();
        let c = Control::key("k");
        let s = ptt_mic();
        r.action.appear(&c, &s).await;
        r.action.press_down(&c, &s);
        r.action.disappear(&c, &s);
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("mute Source({MIC:?}) Off"),
                format!("mute Source({MIC:?}) On"),
            ]
        );
        assert!(r.action.shared.pressed.is_empty());
        assert!(r.action.shared.consumed.is_empty());
        assert!(r.action.shared.instances.is_empty());
    }

    #[tokio::test]
    async fn a_released_or_ordinary_control_that_disappears_does_nothing() {
        let r = rig();
        let c = Control::key("k");
        r.action.press_down(&c, &ptt_mic());
        r.action.press_up(&c, &ptt_mic());
        r.action.disappear(&c, &ptt_mic());
        r.action.press_down(&c, &AudioSettings::default());
        r.action.disappear(&c, &AudioSettings::default());
        r.action.drain().await;
        assert_eq!(r.backend.calls().len(), 2, "{:?}", r.backend.calls());
    }

    #[tokio::test]
    async fn a_failed_push_to_talk_mute_is_retried() {
        let r = rig();
        let c = Control::key("k");
        r.action.press_down(&c, &ptt_mic());
        r.action.drain().await;
        r.backend.fail_times("mute", 1);
        r.action.press_up(&c, &ptt_mic());
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("mute Source({MIC:?}) Off"),
                format!("mute Source({MIC:?}) On"),
                format!("mute Source({MIC:?}) On"),
            ]
        );
        assert_eq!(r.ui.alerts(), 0);
    }

    #[tokio::test]
    async fn a_push_to_talk_mute_that_keeps_failing_alerts() {
        let r = rig();
        let c = Control::key("k");
        r.backend.fail("mute", 0);
        r.action.press_up(&c, &ptt_mic());
        r.action.drain().await;
        assert_eq!(r.backend.calls().len(), 2, "tried twice");
        assert_eq!(r.ui.events().iter().filter(|e| *e == "alert k").count(), 1);
    }

    #[tokio::test]
    async fn a_failed_operation_alerts_and_rereads() {
        let r = rig();
        let c = Control::key("k");
        r.action.refresh().await;
        r.backend.fail("mute", 0);
        r.action.press_up(&c, &AudioSettings::default());
        r.action.drain().await;
        assert_eq!(r.ui.alerts(), 1);
    }

    #[tokio::test]
    async fn refresh_caches_the_snapshot() {
        let r = rig();
        r.action.refresh().await;
        assert_eq!(
            *r.action.shared.snapshot.read().await,
            Ok(fixture_snapshot())
        );
    }

    #[tokio::test]
    async fn a_snapshot_failure_is_cached_as_a_short_message() {
        let r = rig();
        *r.backend.snapshot_error.lock().unwrap() = Some("boom".into());
        r.action.refresh().await;
        assert_eq!(
            *r.action.shared.snapshot.read().await,
            Err("audio error".to_string())
        );
        let c = Control::key("k");
        r.action.press_up(&c, &AudioSettings::default());
        r.action.drain().await;
        assert!(r.backend.calls().is_empty());
        assert_eq!(r.ui.alerts(), 1);
    }

    /// A refresh that read pactl before a local update landed must not
    /// roll the cached level back (the next tick would build on it).
    #[tokio::test(start_paused = true)]
    async fn a_stale_refresh_does_not_roll_back_a_local_update() {
        let r = rig();
        r.action.refresh().await;
        *r.backend.snapshot_delay.lock().unwrap() = Some(Duration::from_millis(100));
        let slow = tokio::spawn({
            let a = r.action.clone();
            async move { a.refresh().await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        *r.backend.snapshot_delay.lock().unwrap() = None;
        let c = Control::dial("d");
        r.action.rotate(&c, &AudioSettings::default(), 1, false);
        r.action.drain().await;
        slow.await.unwrap();
        r.action.rotate(&c, &AudioSettings::default(), 1, false);
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("volume Sink({RAZER:?}) 52"),
                format!("volume Sink({RAZER:?}) 57"),
            ]
        );
    }

    #[tokio::test]
    async fn the_property_inspector_gets_choices_even_without_pactl() {
        let r = rig();
        *r.backend.snapshot_error.lock().unwrap() = Some("boom".into());
        r.action
            .send_choices(&Control::key("k"), &AudioSettings::default())
            .await;
        assert_eq!(r.ui.events(), vec!["pi k audioChoices"]);
    }

    #[tokio::test(start_paused = true)]
    async fn changes_while_nothing_is_visible_skip_the_refresh() {
        let r = rig();
        let (tx, rx) = watch::channel(0u64);
        tokio::spawn(r.action.clone().run_watcher(rx));
        tokio::time::sleep(Duration::from_millis(10)).await;
        // A device appears in pactl while no Audio control is shown.
        r.backend.snapshot.lock().unwrap().sinks.clear();
        tx.send_modify(|n| *n += 1);
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert!(r.action.shared.stale.load(Ordering::SeqCst));
        assert_eq!(
            r.action
                .shared
                .snapshot
                .read()
                .await
                .clone()
                .unwrap()
                .sinks
                .len(),
            2,
            "not re-read"
        );
        // Showing a control catches up.
        r.action
            .appear(&Control::key("k"), &AudioSettings::default())
            .await;
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert!(!r.action.shared.stale.load(Ordering::SeqCst));
        assert!(r.ui.events().last().unwrap().contains("unavailable"));
    }

    #[test]
    fn missing_pactl_gets_a_readable_message() {
        assert_eq!(short_error(&BackendError::NotInstalled), "pactl not found");
    }
}
