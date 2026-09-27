use super::is_choices_request;
use crate::cycle::step_in;
use crate::host;
use crate::opendeck_state::OpenDeckState;
use crate::profile::{ProfileSettings, cycle_list, dial_view, key_view, target_device};
use crate::render::{KEYPAD, show_profile};
use async_trait::async_trait;
use dashmap::DashMap;
use openaction::{Action, Instance, OpenActionResult};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::watch;

struct Shared {
    state: OpenDeckState,
    instances: DashMap<String, ProfileSettings>,
    /// Dial instances: the profile currently highlighted but not yet chosen.
    highlighted: DashMap<String, String>,
}

#[derive(Clone)]
pub struct ProfileAction {
    shared: Arc<Shared>,
}

impl ProfileAction {
    pub fn new(state: OpenDeckState) -> Self {
        Self {
            shared: Arc::new(Shared {
                state,
                instances: DashMap::new(),
                highlighted: DashMap::new(),
            }),
        }
    }

    async fn render(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        let device = target_device(settings, &instance.device_id);
        let active = self.shared.state.active_profile(&device);
        let view = if instance.controller == KEYPAD {
            key_view(active.as_deref(), &settings.profile)
        } else {
            let highlighted = self
                .shared
                .highlighted
                .get(&instance.instance_id)
                .map(|h| h.clone());
            dial_view(active.as_deref(), highlighted.as_deref())
        };
        show_profile(instance, &view).await
    }

    async fn render_all(&self) {
        let entries: Vec<(String, ProfileSettings)> = self
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
                log::warn!("profile render failed: {e}");
            }
        }
    }

    pub async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        while changes.changed().await.is_ok() {
            self.render_all().await;
        }
    }

    async fn switch_to(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
        profile: &str,
    ) -> OpenActionResult<()> {
        let device = target_device(settings, &instance.device_id);
        if profile.trim().is_empty() {
            return instance.show_alert().await;
        }
        let already = self.shared.state.active_profile(&device).as_deref() == Some(profile);
        if let Err(e) = host::send(host::switch_profile_event(&device, profile)).await {
            log::warn!("switchProfile send failed: {e}");
            return instance.show_alert().await;
        }
        if already {
            return Ok(());
        }
        let state = self.shared.state.clone();
        let expected = profile.to_string();
        let id = instance.instance_id.clone();
        tokio::spawn(async move {
            if !host::wait_for(
                &expected,
                || state.active_profile(&device),
                host::CONFIRM_TIMEOUT,
            )
            .await
            {
                host::warn_ignored_once();
                if let Some(instance) = openaction::get_instance(id).await {
                    let _ = instance.show_alert().await;
                }
            }
        });
        Ok(())
    }

    async fn commit_highlight(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        let Some((_, profile)) = self.shared.highlighted.remove(&instance.instance_id) else {
            return Ok(());
        };
        self.switch_to(instance, settings, &profile).await?;
        self.render(instance, settings).await
    }

    async fn send_choices(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        let device = target_device(settings, &instance.device_id);
        let mut devices: Vec<String> = openaction::get_connected_devices()
            .await
            .into_keys()
            .collect();
        devices.sort();
        instance
            .send_to_property_inspector(json!({
                "event": "profileChoices",
                "devices": devices,
                "device": device,
                "profiles": self.shared.state.profiles(&device),
            }))
            .await
    }
}

#[async_trait]
impl Action for ProfileAction {
    const UUID: &'static str = "com.jfms7s.utilities.profile";
    type Settings = ProfileSettings;

    async fn will_appear(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.shared
            .instances
            .insert(instance.instance_id.clone(), settings.clone());
        self.render(instance, settings).await
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        _settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.shared.instances.remove(&instance.instance_id);
        self.shared.highlighted.remove(&instance.instance_id);
        Ok(())
    }

    async fn did_receive_settings(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.shared
            .instances
            .insert(instance.instance_id.clone(), settings.clone());
        self.render(instance, settings).await?;
        // The device may have changed, which changes the profile list.
        self.send_choices(instance, settings).await
    }

    async fn key_up(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.switch_to(instance, settings, settings.profile.trim())
            .await
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
        ticks: i16,
        _pressed: bool,
    ) -> OpenActionResult<()> {
        let device = target_device(settings, &instance.device_id);
        let list = cycle_list(&self.shared.state.profiles(&device), &settings.cycle);
        let current = self
            .shared
            .highlighted
            .get(&instance.instance_id)
            .map(|h| h.clone())
            .or_else(|| self.shared.state.active_profile(&device))
            .unwrap_or_default();
        if let Some(next) = step_in(&list, &current, i64::from(ticks)) {
            self.shared
                .highlighted
                .insert(instance.instance_id.clone(), next);
        }
        self.render(instance, settings).await
    }

    async fn dial_up(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.commit_highlight(instance, settings).await
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
        _position: (u16, u16),
        _hold: bool,
    ) -> OpenActionResult<()> {
        self.commit_highlight(instance, settings).await
    }

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.send_choices(instance, settings).await
    }

    async fn send_to_plugin(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
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
        assert!(uuids.contains(&<ProfileAction as Action>::UUID));
    }
}
