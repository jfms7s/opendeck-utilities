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

/// The plugin's inhibition: its cookie and the server generation that
/// issued it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    cookie: u32,
    generation: u64,
}

struct Shared {
    port: Arc<dyn NotificationsPort>,
    ui: Arc<dyn Ui>,
    instances: Instances<()>,
    /// One inhibition for every DND key. Never held across an await, so a
    /// render never waits on the bus.
    held: std::sync::Mutex<Option<Held>>,
    /// Serialises presses, so two quick presses can't both inhibit. Held
    /// across the D-Bus call, which `dbus.rs` bounds with a timeout; only
    /// press tasks take it, never the event loop.
    pressing: Mutex<()>,
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
                held: std::sync::Mutex::new(None),
                pressing: Mutex::new(()),
            }),
        }
    }

    fn inhibition(&self) -> Inhibition {
        *self.shared.port.inhibition().borrow()
    }

    fn held(&self) -> Option<Held> {
        *self.shared.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_held(&self, held: Option<Held>) {
        *self.shared.held.lock().unwrap_or_else(|e| e.into_inner()) = held;
    }

    /// The plugin holds an inhibition the current server knows about. A
    /// cookie from before a server restart is not ours any more: sending it
    /// could lift someone else's inhibition.
    fn ours(&self) -> bool {
        self.held()
            .is_some_and(|h| h.generation == self.shared.port.generation())
    }

    async fn render_all(&self) {
        let ours = self.ours();
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
        let ours = self.ours();
        self.shared
            .ui
            .show_dnd(control, &view(self.inhibition(), ours))
            .await;
    }

    fn disappear(&self, control: &Control) {
        self.shared.instances.remove(&control.id);
    }

    /// A press runs on its own task: the D-Bus call never holds up the
    /// event loop that serves every action.
    fn key(&self, control: &Control) {
        let action = self.clone();
        let control = control.clone();
        tokio::spawn(async move { action.press(&control).await });
    }

    async fn press(&self, control: &Control) {
        let pressing = self.shared.pending_press().await;
        let generation = self.shared.port.generation();
        let refused = match press(self.inhibition(), self.ours()) {
            Press::Inhibit => match self.shared.port.inhibit().await {
                Ok(cookie) => {
                    self.set_held(Some(Held { cookie, generation }));
                    false
                }
                Err(e) => {
                    log::warn!("Do Not Disturb: Inhibit failed: {e}");
                    true
                }
            },
            Press::UnInhibit => {
                let held = self.held().expect("UnInhibit only with a cookie");
                // Forgotten either way: on failure the server no longer
                // knows the cookie (it restarted) or can't be reached.
                self.set_held(None);
                match self.shared.port.uninhibit(held.cookie).await {
                    Ok(()) => false,
                    Err(e) => {
                        log::warn!("Do Not Disturb: UnInhibit failed: {e}");
                        true
                    }
                }
            }
            Press::Refuse => true,
        };
        drop(pressing);
        if refused {
            self.shared.ui.alert(&control.id).await;
        }
        self.render_all().await;
    }
}

impl Shared {
    async fn pending_press(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.pressing.lock().await
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
        self.key(&Control::of(instance));
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
        action.press(&k).await;
        assert_eq!(ui.events().last().unwrap(), "dnd k On press to resume");
        action.press(&k).await;
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
        action.press(&k).await;
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
        action.press(&a).await;
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
        action.press(&k).await;
        assert_eq!(ui.alerts(), 1);
        assert!(action.held().is_none());
    }

    /// After a server restart the old cookie is not ours: someone else's
    /// inhibition shows "by desktop" and a press sends no stale UnInhibit.
    #[tokio::test]
    async fn a_cookie_from_before_a_server_restart_is_not_used() {
        let (action, port, ui) = rig(Inhibition::Off);
        let k = Control::key("k");
        action.appear(&k).await;
        action.press(&k).await;
        port.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        port.set(Inhibition::On);
        action.render_all().await;
        assert_eq!(ui.events().last().unwrap(), "dnd k On by desktop");
        action.press(&k).await;
        assert_eq!(port.calls(), vec!["inhibit"]);
        assert_eq!(ui.alerts(), 1);
    }

    /// A server that doesn't answer holds up neither the key handler nor a
    /// re-render (the call itself is bounded in `dbus.rs`).
    #[tokio::test]
    async fn a_hung_call_blocks_neither_the_handler_nor_renders() {
        let (action, port, ui) = rig(Inhibition::Off);
        let k = Control::key("k");
        action.appear(&k).await;
        let hang = port.hang.lock().await;
        action.key(&k);
        tokio::task::yield_now().await;
        assert_eq!(port.calls(), vec!["inhibit"], "the press is in flight");
        port.set(Inhibition::On);
        tokio::time::timeout(std::time::Duration::from_secs(1), action.render_all())
            .await
            .expect("render waited on the press");
        drop(hang);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert_eq!(ui.events().last().unwrap(), "dnd k On press to resume");
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
        action.press(&k).await;
        assert!(port.calls().is_empty());
        assert_eq!(ui.alerts(), 1);
    }
}
