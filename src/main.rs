mod actions;
mod audio;
mod cycle;
mod lenient;
mod render;

use actions::audio::AudioAction;
use audio::backend::{AudioBackend, PactlBackend, spawn_subscriber};
use openaction::{OpenActionResult, register_action, run};
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
}
