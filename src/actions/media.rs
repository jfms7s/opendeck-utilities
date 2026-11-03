//! The Media dial: now playing on the strip; rotate seeks, press plays or
//! pauses, a strip tap skips (`crate::media` decides, the port acts).

use super::{Control, Instances, render_each};
use crate::media::{MediaPort, Player, Players, pick, seek_offset, view};
use crate::ui::Ui;
use async_trait::async_trait;
use openaction::{Action, Instance, OpenActionResult};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// The dial has no settings (ruling 4: no pinned player).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaSettings {}

struct Shared {
    port: Arc<dyn MediaPort>,
    ui: Arc<dyn Ui>,
    instances: Instances<()>,
    /// The bus name followed last, so a second player starting doesn't
    /// steal the dial from the one already playing.
    followed: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct MediaAction {
    shared: Arc<Shared>,
}

/// A command for the followed player.
#[derive(Debug, Clone, Copy)]
enum Command {
    PlayPause,
    Next,
    Seek(i64),
}

impl MediaAction {
    /// Builds the action and starts its re-render task.
    pub fn start(port: Arc<dyn MediaPort>, ui: Arc<dyn Ui>) -> Self {
        let action = Self::new(port, ui);
        tokio::spawn(action.clone().run_watcher(action.shared.port.players()));
        action
    }

    fn new(port: Arc<dyn MediaPort>, ui: Arc<dyn Ui>) -> Self {
        Self {
            shared: Arc::new(Shared {
                port,
                ui,
                instances: Instances::new(),
                followed: Mutex::new(None),
            }),
        }
    }

    /// The players now and the one to follow; remembers the choice.
    fn current(&self) -> (Players, Option<Player>) {
        let players = self.shared.port.players().borrow().clone();
        let mut followed = self
            .shared
            .followed
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let chosen = match &players {
            Players::Listed(ps) => pick(ps, followed.as_deref()).cloned(),
            _ => None,
        };
        if let Some(p) = &chosen {
            *followed = Some(p.bus.clone());
        }
        (players, chosen)
    }

    async fn render_all(&self) {
        let (players, chosen) = self.current();
        let shown = view(&players, chosen.as_ref());
        render_each(&self.shared.instances, |control, ()| {
            let shown = shown.clone();
            async move { self.shared.ui.show_media(&control, &shown).await }
        })
        .await;
    }

    async fn run_watcher(self, mut changes: watch::Receiver<Players>) {
        while changes.changed().await.is_ok() {
            self.render_all().await;
        }
    }

    async fn appear(&self, control: &Control) {
        self.shared.ui.forget(&control.id);
        self.shared.instances.insert(control, &());
        self.shared.port.set_watching(true);
        let (players, chosen) = self.current();
        self.shared
            .ui
            .show_media(control, &view(&players, chosen.as_ref()))
            .await;
    }

    fn disappear(&self, control: &Control) {
        self.shared.instances.remove(&control.id);
        self.shared
            .port
            .set_watching(!self.shared.instances.is_empty());
    }

    /// Sends `command` to the followed player on its own task, so a player
    /// that is slow to answer never holds up the deck. Nothing to follow,
    /// or rotation on a player that can't seek: nothing happens.
    fn command(&self, control: &Control, command: Command) {
        let (_, Some(player)) = self.current() else {
            return;
        };
        if matches!(command, Command::Seek(_)) && !player.can_seek {
            return;
        }
        let action = self.clone();
        let id = control.id.clone();
        tokio::spawn(async move {
            let port = &action.shared.port;
            let result = match command {
                Command::PlayPause => port.play_pause(&player.bus).await,
                Command::Next => port.next(&player.bus).await,
                Command::Seek(offset) => port.seek(&player.bus, offset).await,
            };
            if let Err(e) = result {
                log::warn!("Media: {command:?} on {} failed: {e}", player.bus);
                action.shared.ui.alert(&id).await;
            }
        });
    }
}

#[async_trait]
impl Action for MediaAction {
    const UUID: &'static str = "com.jfms7s.utilities.media";
    type Settings = MediaSettings;

    async fn will_appear(&self, instance: &Instance, _: &MediaSettings) -> OpenActionResult<()> {
        self.appear(&Control::of(instance)).await;
        Ok(())
    }

    async fn will_disappear(&self, instance: &Instance, _: &MediaSettings) -> OpenActionResult<()> {
        self.disappear(&Control::of(instance));
        Ok(())
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        _: &MediaSettings,
        ticks: i16,
        _pressed: bool,
    ) -> OpenActionResult<()> {
        self.command(&Control::of(instance), Command::Seek(seek_offset(ticks)));
        Ok(())
    }

    async fn dial_up(&self, instance: &Instance, _: &MediaSettings) -> OpenActionResult<()> {
        self.command(&Control::of(instance), Command::PlayPause);
        Ok(())
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        _: &MediaSettings,
        _position: (u16, u16),
        _hold: bool,
    ) -> OpenActionResult<()> {
        self.command(&Control::of(instance), Command::Next);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::Status;
    use crate::media::fake::FakeMedia;
    use crate::ui::fake::FakeUi;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    fn player(bus: &str, title: &str, status: Status) -> Player {
        Player {
            bus: format!("org.mpris.MediaPlayer2.{bus}"),
            title: title.into(),
            artists: vec!["Artist".into()],
            length: Some(Duration::from_secs(100)),
            position: Duration::from_secs(10),
            status,
            can_seek: true,
        }
    }

    fn rig(players: Players) -> (MediaAction, Arc<FakeMedia>, Arc<FakeUi>) {
        let port = Arc::new(FakeMedia::new(players));
        let ui = Arc::new(FakeUi::default());
        (MediaAction::new(port.clone(), ui.clone()), port, ui)
    }

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn the_dial_shows_the_playing_track_and_sends_commands_to_it() {
        let (action, port, ui) = rig(Players::Listed(vec![
            player("a", "Paused one", Status::Paused),
            player("b", "Song", Status::Playing),
        ]));
        let d = Control::dial("d");
        action.appear(&d).await;
        assert_eq!(ui.events().last().unwrap(), "media d Song Artist 10%");
        assert!(port.watching.load(Ordering::SeqCst));
        action.command(&d, Command::PlayPause);
        action.command(&d, Command::Next);
        action.command(&d, Command::Seek(seek_offset(-2)));
        settle().await;
        assert_eq!(
            port.calls(),
            vec![
                "PlayPause org.mpris.MediaPlayer2.b",
                "Next org.mpris.MediaPlayer2.b",
                "Seek org.mpris.MediaPlayer2.b -10000000",
            ]
        );
        action.disappear(&d);
        assert!(!port.watching.load(Ordering::SeqCst));
    }

    /// Paused, the dial keeps following the player it followed, even when
    /// another player sorts first.
    #[tokio::test]
    async fn a_paused_player_keeps_the_dial() {
        let (action, port, ui) = rig(Players::Listed(vec![player("m", "Mine", Status::Playing)]));
        let d = Control::dial("d");
        action.appear(&d).await;
        port.set(Players::Listed(vec![
            player("a", "Other", Status::Paused),
            player("m", "Mine", Status::Paused),
        ]));
        action.render_all().await;
        assert_eq!(
            ui.events().last().unwrap(),
            "media d Mine paused · Artist 10%"
        );
    }

    #[tokio::test]
    async fn nothing_to_follow_does_nothing() {
        let (action, port, ui) = rig(Players::Listed(vec![]));
        let d = Control::dial("d");
        action.appear(&d).await;
        assert_eq!(ui.events().last().unwrap(), "media d Nothing playing  0%");
        action.command(&d, Command::PlayPause);
        settle().await;
        assert!(port.calls().is_empty());
        assert_eq!(ui.alerts(), 0);
    }

    #[tokio::test]
    async fn a_player_that_cannot_seek_ignores_rotation() {
        let mut p = player("b", "Song", Status::Playing);
        p.can_seek = false;
        let (action, port, _ui) = rig(Players::Listed(vec![p]));
        let d = Control::dial("d");
        action.appear(&d).await;
        action.command(&d, Command::Seek(seek_offset(1)));
        settle().await;
        assert!(port.calls().is_empty());
    }

    #[tokio::test]
    async fn a_change_on_the_bus_re_renders() {
        let port = Arc::new(FakeMedia::new(Players::Listed(vec![])));
        let ui = Arc::new(FakeUi::default());
        let action = MediaAction::start(port.clone(), ui.clone());
        action.appear(&Control::dial("d")).await;
        port.set(Players::Listed(vec![player("b", "Song", Status::Playing)]));
        settle().await;
        assert_eq!(ui.events().last().unwrap(), "media d Song Artist 10%");
    }
}
