//! `MprisMedia` against fake MPRIS players on a private bus.

use super::fake_player::serve_player;
use super::mpris::MprisMedia;
use super::{MediaPort, Players, Status, pick, view};
use crate::test_bus::PrivateBus;
use std::time::Duration;
use tokio::sync::watch;

/// Waits (real time, bounded) until `ok` holds for the reported players.
async fn until(rx: &mut watch::Receiver<Players>, what: &str, ok: impl Fn(&Players) -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|p| ok(p)))
        .await
        .map(|r| r.is_ok());
    assert_eq!(waited, Ok(true), "never {what}; last {:?}", *rx.borrow());
}

fn followed_title(players: &Players) -> Option<String> {
    match players {
        Players::Listed(ps) => pick(ps, None).map(|p| p.title.clone()),
        _ => None,
    }
}

/// A fake player drives title, artist and progress through to the strip
/// view; press and tap reach it as PlayPause and Next.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_player_reaches_the_strip_and_takes_commands() {
    let bus = PrivateBus::start();
    let player = serve_player(&bus, "fake").await;
    let port = MprisMedia::start(Some(bus.address.clone()));
    let mut rx = port.players();
    until(&mut rx, "listed the player", |p| {
        followed_title(p).as_deref() == Some("Song")
    })
    .await;
    let players = rx.borrow().clone();
    let Players::Listed(ps) = &players else {
        unreachable!()
    };
    let shown = view(&players, pick(ps, None));
    assert_eq!(shown.title, "Song");
    assert_eq!(shown.artist, "Artist");
    assert_eq!(shown.progress, 25.0);
    assert!(shown.playing);

    let bus_name = "org.mpris.MediaPlayer2.fake";
    port.play_pause(bus_name).await.unwrap();
    until(
        &mut rx,
        "saw the pause",
        |p| matches!(p, Players::Listed(ps) if ps[0].status == Status::Paused),
    )
    .await;
    port.next(bus_name).await.unwrap();
    until(&mut rx, "saw the next track", |p| {
        followed_title(p).as_deref() == Some("Next Song")
    })
    .await;
    port.seek(bus_name, 10_000_000).await.unwrap();
    until(
        &mut rx,
        "saw the seek",
        |p| matches!(p, Players::Listed(ps) if ps[0].position == Duration::from_secs(10)),
    )
    .await;
    assert_eq!(player.calls(), vec!["PlayPause", "Next", "Seek 10000000"]);
}

/// Players coming and going are picked up from name-owner changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn players_appear_and_disappear() {
    let bus = PrivateBus::start();
    let port = MprisMedia::start(Some(bus.address.clone()));
    let mut rx = port.players();
    until(&mut rx, "listed no players", |p| {
        *p == Players::Listed(vec![])
    })
    .await;
    let player = serve_player(&bus, "late").await;
    until(&mut rx, "saw the new player", |p| {
        followed_title(p).as_deref() == Some("Song")
    })
    .await;
    *player.title.lock().unwrap() = "Retitled".into();
    player.announce().await;
    until(&mut rx, "saw the property change", |p| {
        followed_title(p).as_deref() == Some("Retitled")
    })
    .await;
    player.quit("late").await;
    until(&mut rx, "saw it go", |p| *p == Players::Listed(vec![])).await;
}

/// Anything that isn't an MPRIS name is refused before it reaches the bus.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commands_only_go_to_mpris_names() {
    let bus = PrivateBus::start();
    let port = MprisMedia::start(Some(bus.address.clone()));
    assert!(port.play_pause("org.freedesktop.DBus").await.is_err());
}

/// A hung player costs one call timeout and keeps its last known state;
/// another player's change still gets through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hung_player_neither_vanishes_nor_stalls_the_others() {
    let bus = PrivateBus::start();
    let stuck = serve_player(&bus, "a").await;
    let fine = serve_player(&bus, "b").await;
    let port = MprisMedia::start(Some(bus.address.clone()));
    let mut rx = port.players();
    until(
        &mut rx,
        "listed both",
        |p| matches!(p, Players::Listed(ps) if ps.len() == 2),
    )
    .await;
    stuck.hang.store(true, std::sync::atomic::Ordering::SeqCst);
    *fine.title.lock().unwrap() = "Retitled".into();
    fine.announce().await;
    let started = std::time::Instant::now();
    until(&mut rx, "saw b's change", |p| {
        matches!(p, Players::Listed(ps) if ps.len() == 2 && ps[1].title == "Retitled" && ps[0].title == "Song")
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "took {:?}",
        started.elapsed()
    );
}
