//! `DbusNotifications` against a fake notification server on a private
//! bus (`test_bus`).

use super::NotificationsPort;
use super::dbus::DbusNotifications;
use super::test_bus::{PrivateBus, serve_notifications};
use crate::dnd::Inhibition;
use std::time::Duration;
use tokio::sync::watch;

/// Waits (real time, bounded) until the port reports `want`.
async fn until(rx: &mut watch::Receiver<Inhibition>, want: Inhibition) {
    let waited = tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|s| *s == want))
        .await
        .map(|r| r.is_ok());
    assert_eq!(
        waited,
        Ok(true),
        "never reported {want:?}, last {:?}",
        *rx.borrow()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_and_uninhibit_round_trip_with_one_cookie() {
    let bus = PrivateBus::start();
    let server = serve_notifications(&bus).await;
    let port = DbusNotifications::start(Some(bus.address.clone()));
    let mut rx = port.inhibition();
    until(&mut rx, Inhibition::Off).await;

    let cookie = port.inhibit().await.unwrap();
    until(&mut rx, Inhibition::On).await;
    port.uninhibit(cookie).await.unwrap();
    until(&mut rx, Inhibition::Off).await;

    assert_eq!(
        server.calls(),
        vec![
            format!("Inhibit opendeck \"Do Not Disturb key\" -> {cookie}"),
            format!("UnInhibit {cookie}"),
        ]
    );
}

/// The desktop's own DND (no cookie of ours) reaches the key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inhibited_change_from_elsewhere_is_reported() {
    let bus = PrivateBus::start();
    let server = serve_notifications(&bus).await;
    let port = DbusNotifications::start(Some(bus.address.clone()));
    let mut rx = port.inhibition();
    until(&mut rx, Inhibition::Off).await;
    server.set_other(true).await;
    until(&mut rx, Inhibition::On).await;
    server.set_other(false).await;
    until(&mut rx, Inhibition::Off).await;
    assert!(server.calls().is_empty(), "the plugin only listened");
}

/// No server: unknown, and a press fails cleanly. A server that appears
/// later is picked up from the name-owner change, as a new generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_appears_later_is_picked_up() {
    let bus = PrivateBus::start();
    let port = DbusNotifications::start(Some(bus.address.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(*port.inhibition().borrow(), Inhibition::Unknown);
    assert!(port.inhibit().await.is_err());
    let before = port.generation();
    let _server = serve_notifications(&bus).await;
    let mut rx = port.inhibition();
    until(&mut rx, Inhibition::Off).await;
    assert!(port.generation() > before);
}
