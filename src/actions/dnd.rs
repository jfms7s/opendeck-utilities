//! The Do Not Disturb key: shows the desktop's notification inhibition
//! and toggles the plugin's own (`crate::dnd` decides, the port acts).

use super::{Control, Instances, render_each};
use crate::dnd::{Inhibition, Press, press, view};
use crate::notifications::NotificationsPort;
use crate::ui::Ui;
use async_trait::async_trait;
use openaction::{Action, Instance, OpenActionResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{Mutex, watch};

/// The key has no settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DndSettings {}

struct Shared {
    port: Arc<dyn NotificationsPort>,
    ui: Arc<dyn Ui>,
    instances: Instances<()>,
    /// The plugin's inhibition, one for every DND key. The lock also
    /// serialises presses, so two quick presses can't both inhibit.
    cookie: Mutex<Option<u32>>,
}

#[derive(Clone)]
pub struct DndAction {
    shared: Arc<Shared>,
}

impl DndAction {
    /// Builds the action and starts its re-render task.
    pub fn start(port: Arc<dyn NotificationsPort>, ui: Arc<dyn Ui>) -> Self {
        let action = Self::new(port, ui);
        tokio::spawn(action.clone().run_watcher(action.shared.port.inhibition()));
        action
    }

    fn new(port: Arc<dyn NotificationsPort>, ui: Arc<dyn Ui>) -> Self {
        Self {
            shared: Arc::new(Shared {
                port,
                ui,
                instances: Instances::new(),
                cookie: Mutex::new(None),
            }),
        }
    }

    fn inhibition(&self) -> Inhibition {
        *self.shared.port.inhibition().borrow()
    }

    async fn render_all(&self) {
        let ours = self.shared.cookie.lock().await.is_some();
        let shown = view(self.inhibition(), ours);
        render_each(&self.shared.instances, |control, ()| {
            let shown = shown.clone();
            async move { self.shared.ui.show_dnd(&control, &shown).await }
        })
        .await;
    }

    async fn run_watcher(self, mut changes: watch::Receiver<Inhibition>) {
        while changes.changed().await.is_ok() {
            self.render_all().await;
        }
    }

    async fn appear(&self, control: &Control) {
        self.shared.ui.forget(&control.id);
        self.shared.instances.insert(control, &());
        let ours = self.shared.cookie.lock().await.is_some();
        self.shared
            .ui
            .show_dnd(control, &view(self.inhibition(), ours))
            .await;
    }

    fn disappear(&self, control: &Control) {
        self.shared.instances.remove(&control.id);
    }

    async fn key(&self, control: &Control) {
        let mut cookie = self.shared.cookie.lock().await;
        let refused = match press(self.inhibition(), cookie.is_some()) {
            Press::Inhibit => match self.shared.port.inhibit().await {
                Ok(c) => {
                    *cookie = Some(c);
                    false
                }
                Err(e) => {
                    log::warn!("Do Not Disturb: Inhibit failed: {e}");
                    true
                }
            },
            Press::UnInhibit => {
                let c = cookie.take().expect("UnInhibit only with a cookie");
                match self.shared.port.uninhibit(c).await {
                    Ok(()) => false,
                    // The server no longer knows the cookie (it restarted)
                    // or can't be reached; either way it can't be used again.
                    Err(e) => {
                        log::warn!("Do Not Disturb: UnInhibit failed: {e}");
                        true
                    }
                }
            }
            Press::Refuse => true,
        };
        drop(cookie);
        if refused {
            self.shared.ui.alert(&control.id).await;
        }
        self.render_all().await;
    }
}

#[async_trait]
impl Action for DndAction {
    const UUID: &'static str = "com.jfms7s.utilities.dnd";
    type Settings = DndSettings;

    async fn will_appear(&self, instance: &Instance, _: &DndSettings) -> OpenActionResult<()> {
        self.appear(&Control::of(instance)).await;
        Ok(())
    }

    async fn will_disappear(&self, instance: &Instance, _: &DndSettings) -> OpenActionResult<()> {
        self.disappear(&Control::of(instance));
        Ok(())
    }

    async fn key_up(&self, instance: &Instance, _: &DndSettings) -> OpenActionResult<()> {
        self.key(&Control::of(instance)).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::fake::FakeNotifications;
    use crate::ui::fake::FakeUi;

    fn rig(initial: Inhibition) -> (DndAction, Arc<FakeNotifications>, Arc<FakeUi>) {
        let port = Arc::new(FakeNotifications::new(initial));
        let ui = Arc::new(FakeUi::default());
        (DndAction::new(port.clone(), ui.clone()), port, ui)
    }

    #[tokio::test]
    async fn a_press_inhibits_and_the_next_releases_the_same_cookie() {
        let (action, port, ui) = rig(Inhibition::Off);
        let k = Control::key("k");
        action.appear(&k).await;
        action.key(&k).await;
        assert_eq!(ui.events().last().unwrap(), "dnd k On press to resume");
        action.key(&k).await;
        assert_eq!(ui.events().last().unwrap(), "dnd k Off press to silence");
        assert_eq!(port.calls(), vec!["inhibit", "uninhibit 1"]);
        assert_eq!(ui.alerts(), 0);
    }

    /// Ruling 2 (a): someone else's DND shows "On" and a press only alerts.
    #[tokio::test]
    async fn dnd_from_elsewhere_shows_on_and_a_press_alerts() {
        let (action, port, ui) = rig(Inhibition::On);
        let k = Control::key("k");
        action.appear(&k).await;
        assert_eq!(ui.events().last().unwrap(), "dnd k On by desktop");
        action.key(&k).await;
        assert!(port.calls().is_empty());
        assert_eq!(ui.alerts(), 1);
    }

    /// Two keys share the one inhibition.
    #[tokio::test]
    async fn every_key_shows_the_shared_state() {
        let (action, _port, ui) = rig(Inhibition::Off);
        let (a, b) = (Control::key("a"), Control::key("b"));
        action.appear(&a).await;
        action.appear(&b).await;
        action.key(&a).await;
        let mut last: Vec<String> = ui.events().split_off(ui.events().len() - 2);
        last.sort();
        assert_eq!(
            last,
            ["dnd a On press to resume", "dnd b On press to resume"]
        );
    }

    #[tokio::test]
    async fn a_change_from_elsewhere_re_renders() {
        let port = Arc::new(FakeNotifications::new(Inhibition::Off));
        let ui = Arc::new(FakeUi::default());
        let action = DndAction::start(port.clone(), ui.clone());
        action.appear(&Control::key("k")).await;
        port.set(Inhibition::On);
        for _ in 0..50 {
            if ui.events().last().unwrap() == "dnd k On by desktop" {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("not re-rendered: {:?}", ui.events());
    }

    #[tokio::test]
    async fn a_failed_call_alerts_and_keeps_no_cookie() {
        let (action, port, ui) = rig(Inhibition::Off);
        *port.fail.lock().unwrap() = true;
        let k = Control::key("k");
        action.appear(&k).await;
        action.key(&k).await;
        assert_eq!(ui.alerts(), 1);
        assert!(action.shared.cookie.lock().await.is_none());
    }

    #[tokio::test]
    async fn without_a_server_the_key_says_so_and_a_press_alerts() {
        let (action, port, ui) = rig(Inhibition::Unknown);
        let k = Control::key("k");
        action.appear(&k).await;
        assert_eq!(
            ui.events().last().unwrap(),
            "dnd k Unavailable no notification server"
        );
        action.key(&k).await;
        assert!(port.calls().is_empty());
        assert_eq!(ui.alerts(), 1);
    }
}
