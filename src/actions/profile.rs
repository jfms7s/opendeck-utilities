use super::{Control, Instances, is_choices_request};
use crate::controller::Controller;
use crate::cycle::step_in;
use crate::dispatch::{self, Confirm, Dispatcher, Job};
use crate::host::{self, Host};
use crate::opendeck_state::{OpenDeckState, is_safe_device_id};
use crate::pending::Pending;
use crate::profile::{ProfileSettings, cycle_list, dial_view, key_view, target_device};
use crate::ui::Ui;
use async_trait::async_trait;
use dashmap::DashMap;
use openaction::{Action, Instance, OpenActionResult};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// One switch request: (device, profile).
type Switch = (String, String);

/// How to confirm a switch (spec §6.5).
fn confirm_rule(state: &OpenDeckState, device: &str, profile: &str) -> Confirm {
    match state.active_profile(device) {
        None => Confirm::Unreadable(state.profile_unreadable_reason(device)),
        Some(active) if active == profile => Confirm::Skip,
        Some(_) => {
            let (state, device, profile) = (state.clone(), device.to_string(), profile.to_string());
            Confirm::Until(Box::new(move || {
                state.active_profile(&device).as_deref() == Some(profile.as_str())
            }))
        }
    }
}

/// Why `profile` can't be sent for `device`, if it can't. Settings are
/// user-editable (and travel with shared OpenDeck profiles), so only a
/// device id that is a plain name and a profile OpenDeck actually has for
/// it reach `opendeck --process-message` - the same rule the audio side
/// applies before anything reaches `pactl`.
fn refusal(state: &OpenDeckState, device: &str, profile: &str) -> Option<String> {
    if !is_safe_device_id(device) {
        return Some(format!(
            "refusing to switch profile on device id {device:?}"
        ));
    }
    if !state.profiles(device).iter().any(|p| p == profile) {
        return Some(format!(
            "profile {profile:?} not found on device {device:?}; not switching"
        ));
    }
    None
}

struct Shared {
    state: OpenDeckState,
    ui: Arc<dyn Ui>,
    instances: Instances<ProfileSettings>,
    /// Dial instances: the profile currently highlighted but not yet chosen.
    highlighted: DashMap<String, String>,
    /// Per device: the profile just switched to, shown until OpenDeck saves it.
    pending: DashMap<String, Pending<String>>,
    dispatch: Dispatcher<Switch>,
}

#[derive(Clone)]
pub struct ProfileAction {
    shared: Arc<Shared>,
}

impl ProfileAction {
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
            Duration::ZERO,
            dispatch::latest,
            move |(device, profile): Switch| Job {
                confirm: confirm_rule(&reader, &device, &profile),
                event: host::switch_profile_event(&device, &profile),
            },
        );
        Self {
            shared: Arc::new(Shared {
                state,
                ui,
                instances: Instances::new(),
                highlighted: DashMap::new(),
                pending: DashMap::new(),
                dispatch,
            }),
        }
    }

    /// The device's active profile, or the one just switched to if OpenDeck
    /// hasn't saved it yet.
    fn active_profile(&self, device: &str) -> Option<String> {
        let saved = self.shared.state.active_profile(device);
        match self.shared.pending.get(device) {
            Some(p) => p.resolve(saved),
            None => saved,
        }
    }

    async fn render(&self, control: &Control, settings: &ProfileSettings) {
        let device = target_device(settings, &control.device);
        let active = self.active_profile(&device);
        let view = match control.controller {
            Controller::Keypad => key_view(active.as_deref(), &settings.profile),
            Controller::Encoder => {
                let highlighted = self.shared.highlighted.get(&control.id).map(|h| h.clone());
                dial_view(active.as_deref(), highlighted.as_deref())
            }
        };
        self.shared.ui.show_profile(control, &view).await;
    }

    async fn render_all(&self) {
        for (control, settings) in self.shared.instances.snapshot() {
            self.render(&control, &settings).await;
        }
    }

    async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        while changes.changed().await.is_ok() {
            self.render_all().await;
        }
    }

    /// Shows the switch at once, then queues it; the event loop never waits
    /// on the `opendeck` command.
    async fn switch_to(&self, control: &Control, settings: &ProfileSettings, profile: &str) {
        let device = target_device(settings, &control.device);
        if profile.trim().is_empty() {
            self.shared.ui.alert(&control.id).await;
            return;
        }
        if let Some(why) = refusal(&self.shared.state, &device, profile) {
            log::warn!("{why}");
            self.shared.ui.alert(&control.id).await;
            return;
        }
        if self.active_profile(&device).as_deref() != Some(profile) {
            self.shared
                .pending
                .entry(device.clone())
                .or_insert_with(Pending::new)
                .set(profile.to_string());
            self.render_all().await;
        }
        self.shared
            .dispatch
            .submit(&device, &control.id, (device.clone(), profile.to_string()));
    }

    async fn commit_highlight(&self, control: &Control, settings: &ProfileSettings) {
        let Some((_, profile)) = self.shared.highlighted.remove(&control.id) else {
            return;
        };
        self.switch_to(control, settings, &profile).await;
        self.render(control, settings).await;
    }

    async fn send_choices(&self, control: &Control, settings: &ProfileSettings) {
        let device = target_device(settings, &control.device);
        let mut devices: Vec<String> = openaction::get_connected_devices()
            .await
            .into_keys()
            .collect();
        devices.sort();
        self.shared
            .ui
            .send_to_pi(
                &control.id,
                json!({
                    "event": "profileChoices",
                    "devices": devices,
                    "device": device,
                    "profiles": self.shared.state.profiles(&device),
                }),
            )
            .await;
    }

    async fn appear(&self, control: &Control, settings: &ProfileSettings) {
        self.shared.ui.forget(&control.id);
        self.shared.instances.insert(control, settings);
        self.render(control, settings).await;
    }

    fn disappear(&self, control: &Control) {
        self.shared.instances.remove(&control.id);
        self.shared.highlighted.remove(&control.id);
        self.shared.ui.forget(&control.id);
    }

    async fn settings_changed(&self, control: &Control, settings: &ProfileSettings) {
        self.shared.instances.insert(control, settings);
        // A highlight picked from the old device's list means nothing now.
        self.shared.highlighted.remove(&control.id);
        self.render(control, settings).await;
        // The device may have changed, which changes the profile list.
        self.send_choices(control, settings).await;
    }

    async fn rotate(&self, control: &Control, settings: &ProfileSettings, ticks: i16) {
        let device = target_device(settings, &control.device);
        let list = cycle_list(&self.shared.state.profiles(&device), &settings.cycle);
        let current = self
            .shared
            .highlighted
            .get(&control.id)
            .map(|h| h.clone())
            .or_else(|| self.active_profile(&device))
            .unwrap_or_default();
        if let Some(next) = step_in(&list, &current, i64::from(ticks)) {
            self.shared.highlighted.insert(control.id.clone(), next);
        }
        self.render(control, settings).await;
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
        self.appear(&Control::of(instance), settings).await;
        Ok(())
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        _settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.disappear(&Control::of(instance));
        Ok(())
    }

    async fn did_receive_settings(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.settings_changed(&Control::of(instance), settings)
            .await;
        Ok(())
    }

    async fn key_up(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.switch_to(&Control::of(instance), settings, settings.profile.trim())
            .await;
        Ok(())
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
        ticks: i16,
        _pressed: bool,
    ) -> OpenActionResult<()> {
        self.rotate(&Control::of(instance), settings, ticks).await;
        Ok(())
    }

    async fn dial_up(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.commit_highlight(&Control::of(instance), settings)
            .await;
        Ok(())
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
        _position: (u16, u16),
        _hold: bool,
    ) -> OpenActionResult<()> {
        self.commit_highlight(&Control::of(instance), settings)
            .await;
        Ok(())
    }

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
    ) -> OpenActionResult<()> {
        self.send_choices(&Control::of(instance), settings).await;
        Ok(())
    }

    async fn send_to_plugin(
        &self,
        instance: &Instance,
        settings: &ProfileSettings,
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
    use crate::host::fake::FakeHost;
    use crate::ui::fake::FakeUi;
    use std::fs;
    use std::path::Path;

    fn select(dir: &Path, device: &str, profile: &str) {
        fs::write(
            dir.join(format!("profiles/{device}.json")),
            json!({ "selected_profile": profile }).to_string(),
        )
        .unwrap();
    }

    struct Rig {
        tmp: tempfile::TempDir,
        host: Arc<FakeHost>,
        ui: Arc<FakeUi>,
        action: ProfileAction,
    }

    /// Device `sd-1` with profiles Default/claude/gaming, Default active.
    /// `applies`: whether the fake OpenDeck acts on switch events.
    fn rig(applies: bool) -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::create_dir_all(dir.join("profiles/sd-1")).unwrap();
        for p in ["Default", "claude", "gaming"] {
            fs::write(dir.join(format!("profiles/sd-1/{p}.json")), "{}").unwrap();
        }
        select(&dir, "sd-1", "Default");
        let target = dir.clone();
        let host = FakeHost::new(move |e| {
            if applies {
                select(
                    &target,
                    e["device"].as_str().unwrap(),
                    e["profile"].as_str().unwrap(),
                );
            }
        });
        let ui = Arc::new(FakeUi::default());
        let (_tx, rx) = watch::channel(0u64);
        let action = ProfileAction::new(OpenDeckState::at(dir), rx, host.clone(), ui.clone());
        Rig {
            tmp,
            host,
            ui,
            action,
        }
    }

    fn key_settings(profile: &str) -> ProfileSettings {
        ProfileSettings {
            profile: profile.into(),
            ..Default::default()
        }
    }

    async fn idle() {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    /// QA mutation 3: the pending profile shows before the send finishes.
    #[tokio::test(start_paused = true)]
    async fn a_switch_shows_the_new_profile_at_once_and_is_confirmed() {
        let r = rig(true);
        let c = Control::key("k");
        let s = key_settings("gaming");
        r.action.appear(&c, &s).await;
        r.ui.clear();
        r.action.switch_to(&c, &s, "gaming").await;
        assert_eq!(r.ui.events(), vec!["profile k gaming active"]);
        assert!(r.host.sent().is_empty(), "sent in the background");
        idle().await;
        assert_eq!(
            r.host.sent(),
            vec![json!({"event": "switchProfile", "device": "sd-1", "profile": "gaming"})]
        );
        assert_eq!(r.ui.alerts(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn an_ignored_switch_alerts() {
        let r = rig(false);
        let c = Control::key("k");
        r.action
            .switch_to(&c, &key_settings("gaming"), "gaming")
            .await;
        idle().await;
        assert_eq!(r.host.sent().len(), 1);
        assert_eq!(r.ui.alerts(), 1);
    }

    /// Security review: unchecked ids never reach `opendeck --process-message`.
    #[tokio::test(start_paused = true)]
    async fn unsafe_devices_and_unknown_profiles_are_never_sent() {
        let r = rig(true);
        let c = Control::key("k");
        for (device, profile) in [("..", "Default"), ("sd-1/..", "x"), ("sd-1", "../x")] {
            let s = ProfileSettings {
                device: device.into(),
                profile: profile.into(),
                ..Default::default()
            };
            r.action.switch_to(&c, &s, profile).await;
        }
        r.action
            .switch_to(&c, &key_settings("renamed"), "renamed")
            .await;
        idle().await;
        assert!(r.host.sent().is_empty(), "{:?}", r.host.sent());
        assert_eq!(r.ui.alerts(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn switching_to_the_active_profile_needs_no_confirmation() {
        let r = rig(false);
        let c = Control::key("k");
        r.action
            .switch_to(&c, &key_settings("Default"), "Default")
            .await;
        idle().await;
        assert_eq!(r.host.sent().len(), 1);
        assert_eq!(r.ui.alerts(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn unreadable_active_profile_does_not_alert() {
        let r = rig(false);
        fs::remove_file(r.tmp.path().join("profiles/sd-1.json")).unwrap();
        let c = Control::key("k");
        r.action
            .switch_to(&c, &key_settings("gaming"), "gaming")
            .await;
        idle().await;
        assert_eq!(r.host.sent().len(), 1);
        assert_eq!(r.ui.alerts(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn the_dial_highlights_then_commits() {
        let r = rig(true);
        let c = Control::dial("d");
        let s = ProfileSettings::default();
        r.action.appear(&c, &s).await;
        r.action.rotate(&c, &s, 1).await;
        assert_eq!(
            r.ui.events().last().unwrap(),
            "profile d claude press to switch"
        );
        assert!(r.host.sent().is_empty());
        r.action.commit_highlight(&c, &s).await;
        idle().await;
        assert_eq!(r.host.sent()[0]["profile"], "claude");
        assert_eq!(r.ui.alerts(), 0);
    }

    /// Code-quality review: a highlight from the old device must not be
    /// committed to a new one.
    #[tokio::test(start_paused = true)]
    async fn changing_settings_drops_the_highlight() {
        let r = rig(true);
        let c = Control::dial("d");
        let s = ProfileSettings::default();
        r.action.appear(&c, &s).await;
        r.action.rotate(&c, &s, 1).await;
        r.action.settings_changed(&c, &s).await;
        r.action.commit_highlight(&c, &s).await;
        idle().await;
        assert!(r.host.sent().is_empty());
    }
}
