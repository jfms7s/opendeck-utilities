use super::is_choices_request;
use crate::audio::backend::{AudioBackend, BackendError};
use crate::audio::exec::execute;
use crate::audio::gesture::{self, Controller, Operation};
use crate::audio::model::Snapshot;
use crate::audio::pi::choices;
use crate::audio::settings::AudioSettings;
use crate::audio::target::resolve;
use crate::audio::view::{audio_view, error_view};
use crate::render::show_level;
use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use openaction::{Action, Instance, OpenActionResult};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, watch};

/// Coalesces a burst of `pactl subscribe` events (one dial turn can emit
/// several) into one refresh.
pub const DEBOUNCE: Duration = Duration::from_millis(50);

struct Shared {
    backend: Arc<dyn AudioBackend>,
    /// Last snapshot, or a short message for why there is none.
    snapshot: RwLock<Result<Snapshot, String>>,
    /// Visible instances and their latest settings.
    instances: DashMap<String, AudioSettings>,
    /// When each currently-held key/dial went down.
    pressed: DashMap<String, Instant>,
    /// Dials turned while held: their release performs nothing, so a
    /// long "hold + turn" never fires the long-press operation.
    consumed: DashSet<String>,
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

fn controller(instance: &Instance) -> Controller {
    Controller::from_openaction(&instance.controller)
}

impl AudioAction {
    pub fn new(backend: Arc<dyn AudioBackend>) -> Self {
        Self {
            shared: Arc::new(Shared {
                backend,
                snapshot: RwLock::new(Err("loading".to_string())),
                instances: DashMap::new(),
                pressed: DashMap::new(),
                consumed: DashSet::new(),
            }),
        }
    }

    async fn refresh(&self) {
        let result = self.shared.backend.snapshot().await.map_err(|e| {
            log::warn!("audio snapshot failed: {e}");
            short_error(&e)
        });
        *self.shared.snapshot.write().await = result;
    }

    async fn render(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        let view = match &*self.shared.snapshot.read().await {
            Ok(snap) => audio_view(
                &resolve(settings.target, &settings.target_name, snap),
                settings,
            ),
            Err(message) => error_view(settings, message),
        };
        show_level(instance, &view).await
    }

    async fn render_all(&self) {
        // Collect first so no DashMap shard lock is held across an await.
        let entries: Vec<(String, AudioSettings)> = self
            .shared
            .instances
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        for (id, settings) in entries {
            let Some(instance) = openaction::get_instance(id).await else {
                continue;
            };
            if let Err(e) = self.render(&instance, &settings).await {
                log::warn!("audio render failed: {e}");
            }
        }
    }

    /// Runs forever: refresh + re-render on every (debounced) change tick.
    pub async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        self.refresh().await;
        self.render_all().await;
        while changes.changed().await.is_ok() {
            tokio::time::sleep(DEBOUNCE).await;
            changes.mark_unchanged();
            self.refresh().await;
            self.render_all().await;
        }
    }

    async fn perform(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
        op: Operation,
    ) -> OpenActionResult<()> {
        if op == Operation::None {
            return Ok(());
        }
        // Fresh read: a cached level could lag a fast dial by one debounce.
        let snap = match self.shared.backend.snapshot().await {
            Ok(snap) => snap,
            Err(e) => {
                log::warn!("audio snapshot failed: {e}");
                return instance.show_alert().await;
            }
        };
        let resolved = resolve(settings.target, &settings.target_name, &snap);
        if let Err(e) = execute(
            self.shared.backend.as_ref(),
            &snap,
            &resolved,
            settings,
            &op,
        )
        .await
        {
            log::warn!("audio {op:?} failed: {e}");
            instance.show_alert().await?;
        }
        self.refresh().await;
        self.render_all().await;
        Ok(())
    }

    async fn send_choices(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        let snap = self.shared.backend.snapshot().await.unwrap_or_default();
        instance
            .send_to_property_inspector(choices(&snap, settings, controller(instance)))
            .await
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

    fn forget(&self, id: &str) {
        self.shared.instances.remove(id);
        self.shared.pressed.remove(id);
        self.shared.consumed.remove(id);
    }

    async fn press_down(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.press_started(&instance.instance_id);
        let e = gesture::effective(settings, controller(instance));
        self.perform(instance, settings, gesture::down_operation(&e))
            .await
    }

    async fn press_up(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        let held = self.press_released(&instance.instance_id);
        let e = gesture::effective(settings, controller(instance));
        self.perform(
            instance,
            settings,
            gesture::release_operation(&e, held, settings.step()),
        )
        .await
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
        self.shared
            .instances
            .insert(instance.instance_id.clone(), settings.clone());
        self.render(instance, settings).await
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        _settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.forget(&instance.instance_id);
        Ok(())
    }

    async fn did_receive_settings(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.shared
            .instances
            .insert(instance.instance_id.clone(), settings.clone());
        self.render(instance, settings).await
    }

    async fn key_down(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.press_down(instance, settings).await
    }

    async fn key_up(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_up(instance, settings).await
    }

    async fn dial_down(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.press_down(instance, settings).await
    }

    async fn dial_up(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_up(instance, settings).await
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
        ticks: i16,
        pressed: bool,
    ) -> OpenActionResult<()> {
        self.press_rotated(&instance.instance_id, pressed);
        let op = gesture::rotate_operation(settings.rotate, ticks, settings.step());
        self.perform(instance, settings, op).await
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
        _position: (u16, u16),
        _hold: bool,
    ) -> OpenActionResult<()> {
        let e = gesture::effective(settings, controller(instance));
        self.perform(
            instance,
            settings,
            gesture::gesture_operation(&e.touch_tap, settings.step()),
        )
        .await
    }

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
    ) -> OpenActionResult<()> {
        self.send_choices(instance, settings).await
    }

    async fn send_to_plugin(
        &self,
        instance: &Instance,
        settings: &AudioSettings,
        payload: &serde_json::Value,
    ) -> OpenActionResult<()> {
        if is_choices_request(payload) {
            self.send_choices(instance, settings).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::FakeBackend;

    fn action() -> AudioAction {
        AudioAction::new(Arc::new(FakeBackend::default()))
    }

    #[test]
    fn uuid_is_in_the_manifest() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/manifest.json")).unwrap();
        let uuids: Vec<&str> = manifest["Actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["UUID"].as_str().unwrap())
            .collect();
        assert!(uuids.contains(&<AudioAction as Action>::UUID));
    }

    #[test]
    fn release_without_press_counts_as_a_tap() {
        assert_eq!(
            action().press_released("never-pressed"),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn press_is_forgotten_after_release() {
        let a = action();
        a.press_started("k");
        assert!(a.press_released("k").is_some());
        assert!(a.shared.pressed.is_empty());
    }

    #[test]
    fn rotating_while_held_consumes_the_press() {
        let a = action();
        a.press_started("d");
        a.press_rotated("d", true);
        assert_eq!(a.press_released("d"), None);
    }

    #[test]
    fn rotating_without_holding_keeps_the_press() {
        let a = action();
        a.press_started("d");
        a.press_rotated("d", false);
        assert!(a.press_released("d").is_some());
    }

    #[test]
    fn consumed_mark_is_cleared_after_release() {
        let a = action();
        a.press_started("d");
        a.press_rotated("d", true);
        a.press_released("d");
        assert!(a.shared.consumed.is_empty());
        a.press_started("d");
        assert!(a.press_released("d").is_some());
    }

    #[test]
    fn forgetting_an_instance_clears_its_press_state() {
        let a = action();
        a.press_started("d");
        a.press_rotated("d", true);
        a.press_started("k");
        a.forget("d");
        a.forget("k");
        assert!(a.shared.pressed.is_empty());
        assert!(a.shared.consumed.is_empty());
    }

    #[tokio::test]
    async fn refresh_caches_the_snapshot() {
        let a = action();
        a.refresh().await;
        assert!(a.shared.snapshot.read().await.is_ok());
    }

    #[test]
    fn missing_pactl_gets_a_readable_message() {
        assert_eq!(short_error(&BackendError::NotInstalled), "pactl not found");
    }
}
