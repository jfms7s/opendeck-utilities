#[allow(dead_code)]
mod audio;

use openaction::{OpenActionResult, run};

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    simplelog::SimpleLogger::init(log::LevelFilter::Info, simplelog::Config::default())
        .expect("logger init");
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
