mod actions;
mod audio;
mod brightness;
mod controller;
mod cycle;
mod dispatch;
mod host;
mod lenient;
mod opendeck_state;
mod pending;
mod profile;
mod render;
mod ui;

#[cfg(test)]
mod pi_pages;

use actions::audio::AudioAction;
use actions::brightness::BrightnessAction;
use actions::profile::ProfileAction;
use audio::backend::AudioBackend;
use host::{Host, OpenDeckHost};
use openaction::{OpenActionResult, register_action, run};
use opendeck_state::OpenDeckState;
use std::sync::Arc;
use ui::{OpenDeckUi, Ui};

// Two workers are plenty for a handful of controls (performance review);
// nothing here blocks a worker for long.
/// CoreAudio on macOS, `pactl` (PipeWire/PulseAudio) elsewhere.
fn audio_backend() -> Arc<dyn AudioBackend> {
    #[cfg(target_os = "macos")]
    return Arc::new(audio::coreaudio::CoreAudioBackend);
    #[cfg(not(target_os = "macos"))]
    return Arc::new(audio::backend::PactlBackend);
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> OpenActionResult<()> {
    simplelog::SimpleLogger::init(log::LevelFilter::Info, simplelog::Config::default())
        .expect("logger init");
    // Lets a smoke-test run confirm which build it exercised.
    log::info!("opendeck-utilities {}", env!("CARGO_PKG_VERSION"));

    let ui: Arc<dyn Ui> = Arc::new(OpenDeckUi::default());
    let host: Arc<dyn Host> = Arc::new(OpenDeckHost);
    let deck_state = OpenDeckState::discover();
    let deck_changes = opendeck_state::spawn_watcher(deck_state.clone());

    register_action(AudioAction::start(audio_backend(), ui.clone())).await;
    register_action(BrightnessAction::start(
        deck_state.clone(),
        deck_changes.clone(),
        host.clone(),
        ui.clone(),
    ))
    .await;
    register_action(ProfileAction::start(deck_state, deck_changes, host, ui)).await;

    run(std::env::args().collect()).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_version_matches_cargo() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        assert_eq!(manifest["Version"], env!("CARGO_PKG_VERSION"));
    }

    /// `build.mjs` names each binary after `CodePaths`; the Linux default must
    /// be one of them.
    #[test]
    fn manifest_code_paths_name_the_built_binaries() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        let paths = manifest["CodePaths"].as_object().unwrap();
        for (triple, bin) in paths {
            assert_eq!(
                bin.as_str().unwrap(),
                format!("{}-{triple}", env!("CARGO_PKG_NAME"))
            );
        }
        for key in ["CodePathLin", "CodePathMac"] {
            assert!(
                paths.values().any(|bin| *bin == manifest[key]),
                "{key} must be one of CodePaths"
            );
        }
    }
}
