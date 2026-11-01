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
use std::time::Duration;
use ui::{OpenDeckUi, Ui};

/// CoreAudio on macOS, `pactl` (PipeWire/PulseAudio) elsewhere.
fn audio_backend() -> Arc<dyn AudioBackend> {
    #[cfg(target_os = "macos")]
    return Arc::new(audio::coreaudio::CoreAudioBackend::default());
    #[cfg(not(target_os = "macos"))]
    return Arc::new(audio::backend::PactlBackend);
}

// Two workers are plenty for a handful of controls (performance review);
// nothing here blocks a worker for long.
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

    let audio = AudioAction::start(audio_backend(), ui.clone());
    register_action(audio.clone()).await;
    register_action(BrightnessAction::start(
        deck_state.clone(),
        deck_changes.clone(),
        host.clone(),
        ui.clone(),
    ))
    .await;
    register_action(ProfileAction::start(deck_state, deck_changes, host, ui)).await;

    let result = tokio::select! {
        result = run(std::env::args().collect()) => result,
        signal = shutdown_signal() => {
            log::info!("{signal}: exiting");
            Ok(())
        }
    };
    // OpenDeck quit or stopped the plugin: a held push-to-talk control
    // gets no key-up, so close the mic before the process goes. SIGKILL
    // can't be caught; nothing covers that.
    match tokio::time::timeout(REMUTE_TIMEOUT, audio.remute_held()).await {
        Ok(0) => {}
        Ok(n) => log::info!("re-muted {n} held push-to-talk control(s) on exit"),
        Err(_) => log::warn!("push-to-talk re-mute on exit timed out"),
    }
    result
}

/// How long exit waits for the push-to-talk re-mute.
const REMUTE_TIMEOUT: Duration = Duration::from_secs(1);

/// Resolves on SIGTERM or SIGINT, naming which; never if neither handler
/// can be installed.
async fn shutdown_signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    let term = async {
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                term.recv().await;
            }
            Err(e) => {
                log::warn!("can't watch for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    let int = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            log::warn!("can't watch for SIGINT: {e}");
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        () = term => "SIGTERM",
        () = int => "SIGINT",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_version_matches_cargo() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        assert_eq!(manifest["Version"], env!("CARGO_PKG_VERSION"));
    }

    /// `build.mjs` names each binary after `CodePaths`; `CodePathLin` and
    /// `CodePathMac` must each be one of them.
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
