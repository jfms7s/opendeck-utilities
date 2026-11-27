//! `KwinScripting` against a fake KWin on a private bus (`test_bus`), with
//! a temporary directory standing in for `$XDG_RUNTIME_DIR`.

use super::KwinPort;
use super::fake_kwin::serve_kwin;
use super::scripting::KwinScripting;
use crate::test_bus::PrivateBus;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const SOURCE: &str = "const CONFIG = {\"mode\":\"cycle\"};\nprint('layout');\n";

fn port(bus: &PrivateBus, runtime: &TempDir) -> KwinScripting {
    KwinScripting::new(
        Some(bus.address.clone()),
        Some(runtime.path().as_os_str().to_owned()),
    )
}

/// Files left in the plugin's directory under the runtime dir.
fn leftovers(runtime: &TempDir) -> Vec<PathBuf> {
    match std::fs::read_dir(runtime.path().join("opendeck-utilities")) {
        Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_loads_runs_and_unloads_a_private_copy_of_the_source() {
    let bus = PrivateBus::start();
    let kwin = serve_kwin(&bus).await;
    let runtime = tempfile::tempdir().unwrap();

    port(&bus, &runtime).run_script(SOURCE).await.unwrap();

    let loaded = kwin.loaded();
    assert_eq!(loaded.len(), 1);
    let script = &loaded[0];
    assert_eq!(script.source, SOURCE);
    assert_eq!(script.file_mode, 0o600);
    assert_eq!(script.dir_mode, 0o700);
    assert_eq!(
        Path::new(&script.path).parent().unwrap(),
        runtime.path().join("opendeck-utilities")
    );
    assert_eq!(
        kwin.calls(),
        vec![
            format!("loadScript {} -> 1", script.name),
            "run 1".to_string(),
            format!("unloadScript {}", script.name),
        ]
    );
    assert_eq!(leftovers(&runtime), Vec::<PathBuf>::new());
}

/// KWin answers -1 for a name it already has loaded (checked live).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_negative_id_is_an_error_and_nothing_is_run() {
    let bus = PrivateBus::start();
    let kwin = serve_kwin(&bus).await;
    *kwin.refuse_load.lock().unwrap() = true;
    let runtime = tempfile::tempdir().unwrap();

    let err = port(&bus, &runtime).run_script(SOURCE).await.unwrap_err();

    assert!(err.contains("loadScript"), "{err}");
    let name = &kwin.loaded()[0].name;
    assert_eq!(kwin.calls(), vec![format!("loadScript {name} -> -1")]);
    assert_eq!(leftovers(&runtime), Vec::<PathBuf>::new());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_run_is_an_error_and_still_cleans_up() {
    let bus = PrivateBus::start();
    let kwin = serve_kwin(&bus).await;
    *kwin.fail_run.lock().unwrap() = true;
    let runtime = tempfile::tempdir().unwrap();

    let err = port(&bus, &runtime).run_script(SOURCE).await.unwrap_err();

    assert!(err.contains("run"), "{err}");
    let name = &kwin.loaded()[0].name;
    assert_eq!(
        kwin.calls(),
        vec![
            format!("loadScript {name} -> 1"),
            "run 1".to_string(),
            format!("unloadScript {name}"),
        ]
    );
    assert_eq!(leftovers(&runtime), Vec::<PathBuf>::new());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_kwin_on_the_bus_is_an_error_within_the_timeout() {
    let bus = PrivateBus::start();
    let runtime = tempfile::tempdir().unwrap();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        port(&bus, &runtime).run_script(SOURCE),
    )
    .await
    .expect("gave up within the timeout");

    assert!(result.unwrap_err().contains("loadScript"));
    assert_eq!(leftovers(&runtime), Vec::<PathBuf>::new());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_bus_at_all_is_an_error_within_the_timeout() {
    let runtime = tempfile::tempdir().unwrap();
    let address = format!("unix:path={}", runtime.path().join("no-bus").display());
    let port = KwinScripting::new(Some(address), Some(runtime.path().as_os_str().to_owned()));

    let result = tokio::time::timeout(Duration::from_secs(5), port.run_script(SOURCE))
        .await
        .expect("gave up within the timeout");

    let err = result.unwrap_err();
    assert!(err.contains("connect"), "{err}");
    assert_eq!(leftovers(&runtime), Vec::<PathBuf>::new());
}

/// A name still loaded makes KWin refuse the next load, so every run has
/// its own name and file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_runs_use_different_names_and_files() {
    let bus = PrivateBus::start();
    let kwin = serve_kwin(&bus).await;
    let runtime = tempfile::tempdir().unwrap();
    let port = port(&bus, &runtime);

    port.run_script(SOURCE).await.unwrap();
    port.run_script(SOURCE).await.unwrap();

    let loaded = kwin.loaded();
    assert_eq!(loaded.len(), 2);
    assert_ne!(loaded[0].name, loaded[1].name);
    assert_ne!(loaded[0].path, loaded[1].path);
    assert_eq!(leftovers(&runtime), Vec::<PathBuf>::new());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_runtime_dir_is_an_error_before_anything_is_called() {
    let bus = PrivateBus::start();
    let kwin = serve_kwin(&bus).await;
    let port = KwinScripting::new(Some(bus.address.clone()), None);

    let err = port.run_script(SOURCE).await.unwrap_err();

    assert!(err.contains("XDG_RUNTIME_DIR"), "{err}");
    assert!(kwin.calls().is_empty());
}
