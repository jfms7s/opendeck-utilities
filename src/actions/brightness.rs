use crate::brightness::{
    BrightnessSettings, Request, brightness_view, key_request, rotate_request, toggle_request,
};
use crate::host;
use crate::opendeck_state::OpenDeckState;
use crate::render::show_level;
use async_trait::async_trait;
use dashmap::DashSet;
use openaction::{Action, Instance, OpenActionResult};
use std::sync::Arc;
use tokio::sync::watch;

struct Shared {
    state: OpenDeckState,
    instances: DashSet<String>,
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
            }),
        }
    }

    async fn render(&self, instance: &Instance) -> OpenActionResult<()> {
        show_level(instance, &brightness_view(self.shared.state.brightness())).await
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

    /// Sends the request, then checks in the background that OpenDeck's
    /// stored brightness actually moved; alerts once if it didn't.
    async fn send(&self, instance: &Instance, request: Request) -> OpenActionResult<()> {
        let before = self.shared.state.brightness();
        if let Err(e) = host::send(host::brightness_event(request.change, request.value)).await {
            log::warn!("deviceBrightness send failed: {e}");
            return instance.show_alert().await;
        }
        let Some(expected) = request.expected.filter(|e| Some(*e) != before) else {
            return Ok(());
        };
        let state = self.shared.state.clone();
        let id = instance.instance_id.clone();
        tokio::spawn(async move {
            if !host::wait_for(&expected, || state.brightness(), host::CONFIRM_TIMEOUT).await {
                host::warn_ignored_once();
                if let Some(instance) = openaction::get_instance(id).await {
                    let _ = instance.show_alert().await;
                }
            }
        });
        Ok(())
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
        self.send(
            instance,
            key_request(self.shared.state.brightness(), settings),
        )
        .await
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
        ticks: i16,
        _pressed: bool,
    ) -> OpenActionResult<()> {
        match rotate_request(self.shared.state.brightness(), ticks, settings) {
            Some(request) => self.send(instance, request).await,
            None => Ok(()),
        }
    }

    async fn dial_up(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
    ) -> OpenActionResult<()> {
        self.send(
            instance,
            toggle_request(self.shared.state.brightness(), settings),
        )
        .await
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        settings: &BrightnessSettings,
        _position: (u16, u16),
        _hold: bool,
    ) -> OpenActionResult<()> {
        self.send(
            instance,
            toggle_request(self.shared.state.brightness(), settings),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
