//! The Window Layout key: a press runs `crate::layout::script` inside KWin
//! (the port in `crate::kwin`) and the key shows what a press sets.

use super::{Control, Instances};
use crate::host;
use crate::kwin::KwinPort;
use crate::layout::{self, LayoutSettings};
use crate::ui::Ui;
use async_trait::async_trait;
use openaction::{Action, Instance, OpenActionResult};
use std::sync::Arc;
use tokio::sync::Mutex;

/// `host::warn_once` key: the first failed run of this process is logged.
const WARN_KWIN: &str = "layout::kwin";

struct Shared {
    port: Arc<dyn KwinPort>,
    ui: Arc<dyn Ui>,
    instances: Instances<LayoutSettings>,
    /// Serialises presses: one script runs at a time, in press order. Held
    /// across the run, whose D-Bus calls `kwin/scripting.rs` bounds with
    /// timeouts; only press tasks take it, never the event loop. A run is
    /// never cut short: a cancelled run would skip its own cleanup.
    pressing: Mutex<()>,
}

#[derive(Clone)]
pub struct LayoutAction {
    shared: Arc<Shared>,
}

impl LayoutAction {
    /// Builds the action. It has no background task: the key changes only
    /// when its settings do.
    pub fn start(port: Arc<dyn KwinPort>, ui: Arc<dyn Ui>) -> Self {
        Self {
            shared: Arc::new(Shared {
                port,
                ui,
                instances: Instances::new(),
                pressing: Mutex::new(()),
            }),
        }
    }

    async fn render(&self, control: &Control, settings: &LayoutSettings) {
        let view = layout::view(settings, self.shared.port.supported());
        self.shared.ui.show_layout(control, &view).await;
    }

    async fn appear(&self, control: &Control, settings: &LayoutSettings) {
        self.shared.ui.forget(&control.id);
        self.shared.instances.insert(control, settings);
        self.render(control, settings).await;
    }

    fn disappear(&self, control: &Control) {
        self.shared.instances.remove(&control.id);
    }

    /// Only a visible key is re-rendered; a late change for one that has
    /// gone must not bring it back.
    async fn settings_changed(&self, control: &Control, settings: &LayoutSettings) {
        if !self.shared.instances.contains(&control.id) {
            return;
        }
        self.shared.instances.insert(control, settings);
        self.render(control, settings).await;
    }

    /// A press runs on its own task: the KWin call never holds up the event
    /// loop that serves every action. Without KWin a press does nothing
    /// (the key already says "Linux only").
    fn key(&self, control: &Control, settings: &LayoutSettings) {
        if !self.shared.port.supported() {
            return;
        }
        let action = self.clone();
        let control = control.clone();
        let settings = *settings;
        tokio::spawn(async move { action.press(&control, &settings).await });
    }

    async fn press(&self, control: &Control, settings: &LayoutSettings) {
        let pressing = self.shared.pressing.lock().await;
        let result = self.shared.port.run_script(&layout::script(settings)).await;
        drop(pressing);
        if let Err(e) = result {
            host::warn_once(WARN_KWIN, || {
                format!("Window Layout: KWin script failed: {e}")
            });
            self.shared.ui.alert(&control.id).await;
        }
    }
}

#[async_trait]
impl Action for LayoutAction {
    const UUID: &'static str = "com.jfms7s.utilities.layout";
    type Settings = LayoutSettings;

    async fn will_appear(
        &self,
        instance: &Instance,
        settings: &LayoutSettings,
    ) -> OpenActionResult<()> {
        self.appear(&Control::of(instance), settings).await;
        Ok(())
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        _settings: &LayoutSettings,
    ) -> OpenActionResult<()> {
        self.disappear(&Control::of(instance));
        Ok(())
    }

    async fn did_receive_settings(
        &self,
        instance: &Instance,
        settings: &LayoutSettings,
    ) -> OpenActionResult<()> {
        self.settings_changed(&Control::of(instance), settings)
            .await;
        Ok(())
    }

    async fn key_down(
        &self,
        instance: &Instance,
        settings: &LayoutSettings,
    ) -> OpenActionResult<()> {
        self.key(&Control::of(instance), settings);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kwin::fake::FakeKwin;
    use crate::layout::{Layout, Mode};
    use crate::ui::fake::FakeUi;
    use serde_json::Value;

    fn rig(supported: bool) -> (LayoutAction, Arc<FakeKwin>, Arc<FakeUi>) {
        let port = Arc::new(FakeKwin::with_support(supported));
        let ui = Arc::new(FakeUi::default());
        (LayoutAction::start(port.clone(), ui.clone()), port, ui)
    }

    fn set(layout: Layout) -> LayoutSettings {
        LayoutSettings {
            mode: Mode::Set,
            layout,
        }
    }

    /// The `CONFIG` JSON on a script's first line.
    fn config(script: &str) -> Value {
        let first = script.lines().next().unwrap();
        let json = first
            .strip_prefix("const CONFIG = ")
            .and_then(|s| s.strip_suffix(';'))
            .expect("const CONFIG = <json>;");
        serde_json::from_str(json).unwrap()
    }

    /// `(mode, target)` of every script run so far, in order.
    fn runs(port: &FakeKwin) -> Vec<(String, String)> {
        port.scripts()
            .iter()
            .map(|s| {
                let c = config(s);
                (
                    c["mode"].as_str().unwrap().to_string(),
                    c["target"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// Lets spawned presses run until `done`, or fails.
    async fn until(what: &str, done: impl Fn() -> bool) {
        for _ in 0..100 {
            if done() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("never happened: {what}");
    }

    fn run(mode: &str, target: &str) -> (String, String) {
        (mode.to_string(), target.to_string())
    }

    #[tokio::test]
    async fn appearing_shows_what_a_press_sets() {
        let (action, _port, ui) = rig(true);
        action
            .appear(&Control::key("a"), &set(Layout::Thirds))
            .await;
        action
            .appear(&Control::key("b"), &LayoutSettings::default())
            .await;
        assert_eq!(
            ui.events(),
            [
                "layout a 1/3 press to set",
                "layout b Cycle 1/3 → 1/4 → 1/5"
            ]
        );
    }

    #[tokio::test]
    async fn a_press_runs_one_script_for_the_keys_settings() {
        let (action, port, ui) = rig(true);
        let k = Control::key("k");
        action.appear(&k, &set(Layout::Thirds)).await;
        action.key(&k, &set(Layout::Thirds));
        until("the script ran", || !port.scripts().is_empty()).await;
        // Let anything else the press might do happen.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(runs(&port), [run("set", "thirds")]);
        assert_eq!(ui.alerts(), 0);

        action.press(&k, &LayoutSettings::default()).await;
        assert_eq!(runs(&port)[1], run("cycle", "quarters"));
    }

    #[tokio::test]
    async fn a_failed_run_alerts_that_key_only() {
        let (action, port, ui) = rig(true);
        let (a, b) = (Control::key("a"), Control::key("b"));
        action.appear(&a, &set(Layout::Quarters)).await;
        action.appear(&b, &set(Layout::Quarters)).await;
        *port.fail.lock().unwrap() = Some("KWin is not on the bus".into());
        action.press(&a, &set(Layout::Quarters)).await;
        assert_eq!(ui.alerts(), 1);
        assert!(ui.events().contains(&"alert a".to_string()));
        assert!(!ui.events().contains(&"alert b".to_string()));
    }

    /// A run in progress holds back the next press; the next one starts
    /// once it finishes, and presses run in the order they came.
    #[tokio::test]
    async fn quick_presses_run_one_after_the_other_in_order() {
        let (action, port, _ui) = rig(true);
        let k = Control::key("k");
        action.appear(&k, &set(Layout::Thirds)).await;
        let hang = port.hang.lock().await;
        action.key(&k, &set(Layout::Thirds));
        action.key(&k, &set(Layout::Fifths));
        until("the first run started", || port.scripts().len() == 1).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(runs(&port), [run("set", "thirds")], "the second waited");
        drop(hang);
        until("the second run started", || port.scripts().len() == 2).await;
        assert_eq!(runs(&port), [run("set", "thirds"), run("set", "fifths")]);
    }

    #[tokio::test]
    async fn changed_settings_re_render_and_drive_the_next_press() {
        let (action, port, ui) = rig(true);
        let k = Control::key("k");
        action.appear(&k, &set(Layout::Thirds)).await;
        action.press(&k, &set(Layout::Thirds)).await;
        action.settings_changed(&k, &set(Layout::Quarters)).await;
        assert_eq!(ui.events().last().unwrap(), "layout k 1/4 press to set");
        action.press(&k, &set(Layout::Quarters)).await;
        assert_eq!(runs(&port), [run("set", "thirds"), run("set", "quarters")]);
    }

    #[tokio::test]
    async fn without_kwin_the_key_says_linux_only_and_a_press_does_nothing() {
        let (action, port, ui) = rig(false);
        let k = Control::key("k");
        action.appear(&k, &set(Layout::Quarters)).await;
        assert_eq!(ui.events(), ["layout k Linux only "]);
        action.key(&k, &set(Layout::Quarters));
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert!(port.scripts().is_empty());
        assert_eq!(ui.alerts(), 0);
    }

    #[tokio::test]
    async fn a_key_that_has_gone_is_not_rendered() {
        let (action, _port, ui) = rig(true);
        let k = Control::key("k");
        action.appear(&k, &set(Layout::Quarters)).await;
        action.disappear(&k);
        ui.clear();
        action.settings_changed(&k, &set(Layout::Thirds)).await;
        assert!(ui.events().is_empty(), "{:?}", ui.events());
        assert!(action.shared.instances.is_empty());
    }
}
