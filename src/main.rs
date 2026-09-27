mod actions;
mod audio;
mod brightness;
mod cycle;
mod host;
mod lenient;
mod opendeck_state;
mod pending;
mod profile;
mod render;

use actions::audio::AudioAction;
use actions::brightness::BrightnessAction;
use actions::profile::ProfileAction;
use audio::backend::{AudioBackend, PactlBackend, spawn_subscriber};
use openaction::{OpenActionResult, register_action, run};
use opendeck_state::OpenDeckState;
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    simplelog::SimpleLogger::init(log::LevelFilter::Info, simplelog::Config::default())
        .expect("logger init");

    let (audio_tx, audio_rx) = watch::channel(0u64);
    spawn_subscriber(audio_tx);
    let backend: Arc<dyn AudioBackend> = Arc::new(PactlBackend);
    let audio = AudioAction::new(backend);
    tokio::spawn(audio.clone().run_watcher(audio_rx));
    register_action(audio).await;

    let deck_state = OpenDeckState::discover();
    let (deck_tx, deck_rx) = watch::channel(0u64);
    opendeck_state::spawn_watcher(deck_state.clone(), deck_tx);

    let brightness = BrightnessAction::new(deck_state.clone());
    tokio::spawn(brightness.clone().run_watcher(deck_rx.clone()));
    tokio::spawn(brightness.clone().run_sender());
    register_action(brightness).await;

    let profile = ProfileAction::new(deck_state);
    tokio::spawn(profile.clone().run_watcher(deck_rx));
    register_action(profile).await;

    run(std::env::args().collect()).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_version_matches_cargo() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        assert_eq!(manifest["Version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            manifest["CodePathLin"],
            "opendeck-utilities-x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn every_manifest_action_is_registered() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        let mut uuids: Vec<&str> = manifest["Actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["UUID"].as_str().unwrap())
            .collect();
        uuids.sort();
        assert_eq!(
            uuids,
            vec![
                "com.jfms7s.utilities.audio",
                "com.jfms7s.utilities.brightness",
                "com.jfms7s.utilities.profile",
            ]
        );
    }
}
