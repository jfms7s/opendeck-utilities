use crate::brightness::{
    BrightnessSettings, Request, brightness_view, key_request, rotate_request, toggle_request,
};
use crate::host::{self, BrightnessChange};
use crate::opendeck_state::OpenDeckState;
use crate::pending::Pending;
use crate::render::show_level;
use async_trait::async_trait;
use dashmap::DashSet;
use openaction::{Action, Instance, OpenActionResult};
use std::sync::Arc;
use tokio::sync::watch;

struct Shared {
    state: OpenDeckState,
    instances: DashSet<String>,
    /// The brightness last asked for, shown until OpenDeck saves it.
    pending: Pending<u8>,
    /// Newest absolute target and the instance that asked; the sender task
    /// only ever sends the latest, so a fast dial spin collapses into one
    /// request instead of queueing one `opendeck` call per tick.
    target: watch::Sender<Option<(u8, String)>>,
}

/// Whether to watch for OpenDeck's stored brightness to change (spec §6.5).
/// Only a request that cannot change a known value needs no check.
fn needs_confirmation(before: Option<u8>, expected: Option<u8>) -> bool {
    before.is_none() || expected != before
}

#[derive(Clone)]
pub struct BrightnessAction {
    shared: Arc<Shared>,
}

impl BrightnessAction {
    pub fn new(state: OpenDeckState) -> Self {
        Self {
            shared: Arc::new(Shared {
                state,
                instances: DashSet::new(),
                pending: Pending::new(),
                target: watch::Sender::new(None),
            }),
        }
    }

    /// OpenDeck's brightness, or the one just asked for if it hasn't landed.
    fn current(&self) -> Option<u8> {
        self.shared.pending.resolve(self.shared.state.brightness())
    }

    async fn render(&self, instance: &Instance) -> OpenActionResult<()> {
        show_level(instance, &brightness_view(self.current())).await
    }

    async fn render_all(&self) {
        let ids: Vec<String> = self
            .shared
            .instances
            .iter()
            .map(|e| e.key().clone())
            .collect();
        for id in ids {
            let Some(instance) = openaction::get_instance(id).await else {
                continue;
            };
            if let Err(e) = self.render(&instance).await {
                log::warn!("brightness render failed: {e}");
            }
        }
    }

    pub async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        while changes.changed().await.is_ok() {
            self.render_all().await;
        }
    }

    /// Sends each new target in turn, skipping any overtaken while the
    /// previous one was in flight.
    pub async fn run_sender(self) {
        let mut targets = self.shared.target.subscribe();
        while targets.changed().await.is_ok() {
            let Some((value, id)) = targets.borrow_and_update().clone() else {
                continue;
            };
            let request = Request {
                change: BrightnessChange::Set,
                value,
                expected: Some(value),
            };
            self.deliver(&id, request).await;
        }
    }

    /// Shows a request at once when its outcome is known, then sends it.
    async fn send(&self, instance: &Instance, request: Request) -> OpenActionResult<()> {
        match request.expected {
            Some(target) => {
                self.shared.pending.set(target);
                self.render_all().await;
                self.shared
                    .target
                    .send_replace(Some((target, instance.instance_id.clone())));
            }
            // Brightness unknown: only a relative request makes sense.
            None => self.deliver(&instance.instance_id, request).await,
        }
        Ok(())
    }

    /// Sends the request, then checks in the background that OpenDeck's
    /// stored brightness actually changed; alerts if it didn't. Any change
    /// counts: a newer request may overtake this one.
    async fn deliver(&self, id: &str, request: Request) {
        let before = self.shared.state.brightness();
        if let Err(e) = host::send(host::brightness_event(request.change, request.value)).await {
            log::warn!("deviceBrightness send failed: {e}");
            alert(id.to_string()).await;
            return;
        }
        if !needs_confirmation(before, request.expected) {
            return;
        }
        let state = self.shared.state.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            if !host::wait_until(|| state.brightness() != before, host::CONFIRM_TIMEOUT).await {
                host::warn_ignored_once();
                alert(id).await;
            }
        });
    }
}

async fn alert(id: String) {
    if let Some(instance) = openaction::get_instance(id).await {
        let _ = instance.show_alert().await;
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
        self.shared.instances.insert(instance.instance_id.clone());
        self.render(instance).await
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        _settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.shared.instances.remove(&instance.instance_id);
        Ok(())
    }

    async fn key_up(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.send(instance, key_request(self.current(), settings))
            .await
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
        ticks: i16,
        _pressed: bool,
    ) -> OpenActionResult<()> {
        match rotate_request(self.current(), ticks, settings) {
            Some(request) => self.send(instance, request).await,
            None => Ok(()),
        }
    }

    async fn dial_up(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.send(instance, toggle_request(self.current(), settings))
            .await
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
        _position: (u16, u16),
        _hold: bool,
    ) -> OpenActionResult<()> {
        self.send(instance, toggle_request(self.current(), settings))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirms_unless_the_request_cannot_change_a_known_value() {
        assert!(needs_confirmation(Some(50), Some(55)));
        assert!(!needs_confirmation(Some(100), Some(100)));
        assert!(needs_confirmation(None, None));
        assert!(needs_confirmation(Some(50), None));
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
        assert!(uuids.contains(&<BrightnessAction as Action>::UUID));
    }
}
