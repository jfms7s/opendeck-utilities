use super::{Control, Instances, is_choices_request, render_each};
use crate::audio::backend::AudioBackend;
use crate::audio::exec::{apply_locally, execute};
use crate::audio::gesture::{self, Operation};
use crate::audio::model::Snapshot;
use crate::audio::pi::choices;
use crate::audio::settings::{AudioSettings, OpKind};
use crate::audio::target::{recorders_for, resolve};
use crate::audio::view::{audio_view, error_view};
use crate::ui::Ui;
use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use openaction::{Action, Instance, OpenActionResult};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, Notify, RwLock, watch};

/// Coalesces a burst of backend change events (one dial turn can emit
/// several) into one refresh.
pub const DEBOUNCE: Duration = Duration::from_millis(50);

/// How long the exit re-mute waits for the job in flight before going
/// ahead (the caller bounds the whole re-mute at about 1 s).
const IN_FLIGHT_WAIT: Duration = Duration::from_millis(300);

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
/// a dial queues volume steps faster than the backend applies them; consecutive
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
    /// the backend before that update can't roll the shown level back.
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
    /// Held by the worker while it takes and runs a job, so the exit
    /// re-mute never races a job in flight.
    running: AsyncMutex<()>,
}

#[derive(Clone)]
pub struct AudioAction {
    shared: Arc<Shared>,
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
                running: AsyncMutex::new(()),
            }),
        }
    }

    async fn read(&self) -> Result<Snapshot, String> {
        self.shared.backend.snapshot().await.map_err(|e| {
            log::warn!("audio snapshot failed: {e}");
            e.short()
        })
    }

    async fn refresh(&self) {
        let writes = self.shared.local_writes.load(Ordering::SeqCst);
        let result = self.read().await;
        let mut slot = self.shared.snapshot.write().await;
        if result.is_ok() && self.shared.local_writes.load(Ordering::SeqCst) != writes {
            // Read before a local update landed. Ask the watcher for another
            // read rather than trusting the change channel to bump again:
            // a backend may count this read as delivered (CoreAudio's
            // `Seen::record`), and it may hold an outside change too.
            self.shared.kick.notify_one();
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
            Ok(snap) => {
                let resolved = resolve(settings.target, &settings.target_name, snap);
                audio_view(
                    &resolved,
                    settings,
                    snap.caps.volume_cap,
                    &recorders_for(&resolved, snap),
                )
            }
            Err(message) => error_view(settings, message),
        };
        self.shared.ui.show_level(control, &view).await;
    }

    async fn render_all(&self) {
        render_each(&self.shared.instances, |control, settings| async move {
            self.render(&control, &settings).await;
        })
        .await;
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
                    // The subscriber gave up (backend tool missing); only kicks remain.
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
            let running = self.shared.running.lock().await;
            if let Some(job) = self.shared.queue.pop() {
                self.run_job(job).await;
                continue;
            }
            drop(running);
            self.shared.queue.ready.notified().await;
        }
    }

    /// Re-mutes every push-to-talk control still held, for when the plugin
    /// exits (OpenDeck quit or SIGTERM): no key-up or disappear follows.
    /// Waits (up to `IN_FLIGHT_WAIT`) for the job in flight, then drops the
    /// queued jobs except re-mutes (a queued unmute would reopen the mic)
    /// and runs the mutes here, without touching the deck. A job still
    /// running after the wait (a slow backend) doesn't stop the mutes.
    /// Returns how many held controls it tried to re-mute.
    pub async fn remute_held(&self) -> usize {
        let _running = match tokio::time::timeout(IN_FLIGHT_WAIT, self.shared.running.lock()).await
        {
            Ok(guard) => Some(guard),
            Err(_) => {
                log::warn!("an audio job is still running; re-muting anyway");
                None
            }
        };
        let mut mutes: Vec<(AudioSettings, Operation)> =
            std::iter::from_fn(|| self.shared.queue.pop())
                .filter(|job| job.must)
                .map(|job| (job.settings, job.op))
                .collect();
        let mut held = 0;
        for (control, settings) in self.shared.instances.snapshot() {
            let e = gesture::effective(&settings, control.controller);
            if e.press.op == OpKind::PushToTalk && self.is_held(&control.id) {
                mutes.push((settings, Operation::SetMute(true)));
                held += 1;
            }
        }
        for (settings, op) in &mutes {
            // Failures are logged by `execute_job`.
            let _ = self.execute_job(settings, op, true).await;
        }
        held
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
        if let Err(e) = self.execute_job(&settings, &op, must).await {
            log::warn!("audio {op:?} failed: {e}");
            self.shared.ui.alert(&control.id).await;
            // Show what actually happened (e.g. half of an app's streams).
            self.refresh().await;
        }
        self.render_all().await;
    }

    /// Runs `op`; a `must` one (push-to-talk's re-mute) that fails is
    /// retried once on a fresh read, since failing leaves the mic live.
    async fn execute_job(
        &self,
        settings: &AudioSettings,
        op: &Operation,
        must: bool,
    ) -> Result<(), String> {
        let mut result = self.execute(settings, op).await;
        if result.is_err() && must {
            self.refresh().await;
            result = self.execute(settings, op).await;
            if let Err(e) = &result {
                log::error!(
                    "push-to-talk could not mute {:?} again: {e}",
                    settings.target
                );
            }
        }
        result
    }

    /// Carries `op` out against the cached snapshot and updates the cache
    /// to match, so the next tick needs no backend read.
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
        // Linux trusts the prediction (`pactl subscribe` reports the change
        // anyway). macOS also re-reads once after each write, because a
        // device rounds the volume to its own steps: the backend's
        // `written` bumps the change channel for that.
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

    fn tap(&self, control: &Control, settings: &AudioSettings, x: u16, hold: bool) {
        let e = gesture::effective(settings, control.controller);
        let op = gesture::tap_operation(&e, x, hold, settings.step());
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
        position: (u16, u16),
        hold: bool,
    ) -> OpenActionResult<()> {
        self.tap(&Control::of(instance), settings, position.0, hold);
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
    use crate::audio::backend::BackendError;
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

    /// QA mutation M9: ticks from two dials turned together are summed per
    /// dial, never into one call with the other dial's target.
    #[tokio::test]
    async fn two_dials_turned_together_change_their_own_targets() {
        const SPDIF: &str = "alsa_output.pci-0000_2f_00.4.iec958-stereo";
        let r = rig();
        let (d1, d2) = (Control::dial("d1"), Control::dial("d2"));
        let s1 = AudioSettings::default();
        let s2 = AudioSettings {
            target: crate::audio::target::TargetKind::Output,
            target_name: SPDIF.into(),
            ..AudioSettings::default()
        };
        r.action.refresh().await;
        for _ in 0..2 {
            r.action.rotate(&d1, &s1, 1, false);
            r.action.rotate(&d2, &s2, -1, false);
        }
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("volume Sink({RAZER:?}) 52"),
                format!("volume Sink({SPDIF:?}) 95"),
                format!("volume Sink({RAZER:?}) 57"),
                format!("volume Sink({SPDIF:?}) 90"),
            ]
        );
    }

    #[tokio::test]
    async fn mute_and_volume_steps_keep_their_order() {
        let r = rig();
        let c = Control::dial("d");
        let s = AudioSettings::default();
        r.action.rotate(&c, &s, 1, false);
        r.action.tap(&c, &s, 0, false);
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

    /// QA mutation M11: a push-to-talk dial that is held and turned moves
    /// from `pressed` to `consumed`, and must still re-mute when it
    /// disappears while held.
    #[tokio::test]
    async fn a_held_and_turned_push_to_talk_dial_that_disappears_mutes_again() {
        let r = rig();
        let c = Control::dial("d");
        let s = ptt_mic();
        r.action.appear(&c, &s).await;
        r.action.press_down(&c, &s);
        r.action.rotate(&c, &s, 1, true);
        r.action.disappear(&c, &s);
        r.action.drain().await;
        let calls = r.backend.calls();
        assert_eq!(calls.first(), Some(&format!("mute Source({MIC:?}) Off")));
        assert_eq!(calls.last(), Some(&format!("mute Source({MIC:?}) On")));
        assert!(r.action.shared.pressed.is_empty());
        assert!(r.action.shared.consumed.is_empty());
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

    /// Plugin exit: a held push-to-talk key and a held-and-turned dial
    /// re-mute; a released one and a held non-push-to-talk control don't.
    #[tokio::test]
    async fn exit_remutes_only_held_push_to_talk_controls() {
        let r = rig();
        let (key, dial, released, plain) = (
            Control::key("key"),
            Control::dial("dial"),
            Control::key("released"),
            Control::key("plain"),
        );
        let ptt = ptt_mic();
        for c in [&key, &dial, &released] {
            r.action.appear(c, &ptt).await;
        }
        r.action.appear(&plain, &AudioSettings::default()).await;
        r.action.press_down(&key, &ptt);
        r.action.press_down(&dial, &ptt);
        r.action.rotate(&dial, &ptt, 1, true);
        r.action.press_down(&released, &ptt);
        r.action.press_up(&released, &ptt);
        r.action.press_down(&plain, &AudioSettings::default());
        r.action.drain().await;
        let before = r.backend.calls().len();
        assert_eq!(r.action.remute_held().await, 2);
        assert_eq!(
            r.backend.calls()[before..],
            [
                format!("mute Source({MIC:?}) On"),
                format!("mute Source({MIC:?}) On"),
            ]
        );
        assert_eq!(r.ui.alerts(), 0);
    }

    /// Plugin exit with the unmute still queued: it is dropped, not run
    /// after the re-mute, and a queued re-mute (control gone) still runs.
    #[tokio::test]
    async fn exit_drops_a_queued_unmute_and_keeps_queued_remutes() {
        let r = rig();
        let (key, gone) = (Control::key("key"), Control::key("gone"));
        let ptt = ptt_mic();
        r.action.appear(&key, &ptt).await;
        r.action.appear(&gone, &ptt).await;
        r.action.press_down(&gone, &ptt);
        r.action.disappear(&gone, &ptt);
        r.action.press_down(&key, &ptt);
        assert_eq!(r.action.remute_held().await, 1);
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("mute Source({MIC:?}) On"),
                format!("mute Source({MIC:?}) On"),
            ]
        );
        r.action.drain().await;
        assert_eq!(r.backend.calls().len(), 2, "nothing left to run");
    }

    /// Review: a job stuck in flight (slow `pactl`) must not stop the exit
    /// re-mute from being sent.
    #[tokio::test(start_paused = true)]
    async fn exit_remutes_even_while_a_job_is_stuck() {
        let r = rig();
        let key = Control::key("key");
        let ptt = ptt_mic();
        r.action.appear(&key, &ptt).await;
        r.action.press_down(&key, &ptt);
        r.action.drain().await;
        let _stuck = r.action.shared.running.lock().await;
        assert_eq!(r.action.remute_held().await, 1);
        assert_eq!(
            r.backend.calls().last().unwrap(),
            &format!("mute Source({MIC:?}) On")
        );
    }

    /// QA mutation M12: the retry re-reads the audio system first, so it
    /// re-targets the current default input rather than the cached one.
    #[tokio::test]
    async fn a_failed_push_to_talk_mute_is_retried_on_a_fresh_read() {
        const OTHER: &str = "alsa_input.test-other";
        let r = rig();
        let c = Control::key("k");
        r.action.refresh().await;
        r.action.press_down(&c, &ptt_mic());
        r.action.drain().await;
        {
            let mut snap = r.backend.snapshot.lock().unwrap();
            snap.sources.push(crate::audio::model::Device {
                index: 99,
                name: OTHER.into(),
                description: "Other mic".into(),
                volume: 40,
                muted: false,
            });
            snap.default_source = OTHER.into();
        }
        r.backend.fail_times("mute", 1);
        r.action.press_up(&c, &ptt_mic());
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("mute Source({MIC:?}) Off"),
                format!("mute Source({MIC:?}) On"),
                format!("mute Source({OTHER:?}) On"),
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

    /// QA mutation M19: after a failed operation the control re-reads the
    /// audio system, so the cache holds what actually happened rather than
    /// the state before the attempt.
    #[tokio::test]
    async fn a_failed_operation_alerts_and_rereads() {
        let r = rig();
        let c = Control::key("k");
        r.action.refresh().await;
        r.backend.snapshot.lock().unwrap().sinks[0].muted = true;
        r.backend.fail("mute", 0);
        r.action.press_up(&c, &AudioSettings::default());
        r.action.drain().await;
        assert_eq!(r.ui.alerts(), 1);
        assert_ne!(
            *r.action.shared.snapshot.read().await,
            Ok(fixture_snapshot()),
            "the cache still holds the read from before the failure"
        );
        assert_eq!(
            *r.action.shared.snapshot.read().await,
            Ok(r.backend.snapshot.lock().unwrap().clone())
        );
    }

    fn all_inputs_rig() -> Rig {
        let r = rig();
        {
            let mut snap = r.backend.snapshot.lock().unwrap();
            for d in &mut snap.sources {
                d.muted = false;
            }
            let mut other = snap.sources[0].clone();
            other.index = 71;
            other.name = "alsa_input.pci-analog-stereo".into();
            snap.sources.push(other);
        }
        r
    }

    fn all_inputs() -> AudioSettings {
        AudioSettings {
            target: crate::audio::target::TargetKind::AllInputs,
            ..AudioSettings::default()
        }
    }

    /// A mute that lands on the first mic and fails on the second alerts
    /// and re-reads, so the control shows the half-muted truth.
    #[tokio::test]
    async fn an_all_inputs_mute_that_fails_part_way_alerts_and_rereads() {
        let r = all_inputs_rig();
        let c = Control::key("k");
        r.action.refresh().await;
        r.backend.snapshot.lock().unwrap().sources[0].muted = true;
        r.backend.fail("mute", 1);
        r.action.press_up(&c, &all_inputs());
        r.action.drain().await;
        assert_eq!(r.ui.alerts(), 1);
        assert_eq!(
            *r.action.shared.snapshot.read().await,
            Ok(r.backend.snapshot.lock().unwrap().clone())
        );
    }

    #[tokio::test]
    async fn push_to_talk_on_all_inputs_opens_and_closes_every_mic() {
        let r = all_inputs_rig();
        let c = Control::key("k");
        let s = AudioSettings {
            press: ptt_mic().press,
            ..all_inputs()
        };
        r.action.press_down(&c, &s);
        r.action.press_up(&c, &s);
        r.action.drain().await;
        assert_eq!(
            r.backend.calls(),
            vec![
                format!("mute Source({MIC:?}) Off"),
                r#"mute Source("alsa_input.pci-analog-stereo") Off"#.to_string(),
                format!("mute Source({MIC:?}) On"),
                r#"mute Source("alsa_input.pci-analog-stereo") On"#.to_string(),
            ]
        );
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
        // The dropped read asked the watcher for a fresh one (adversarial
        // review: CoreAudio counts the dropped read as delivered, so no
        // change tick may follow).
        tokio::time::timeout(Duration::from_millis(1), r.action.shared.kick.notified())
            .await
            .expect("the dropped refresh kicks the watcher");
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
        let e = BackendError::NotInstalled("pactl");
        assert_eq!(e.short(), "pactl not found");
    }
}
