//! A store the daemon cannot read: its looks fail, and the watch hears of
//! it outside the store, once and again each hour; doctor says so.

use crate::fixture::World;

/// The watch's session up, then the store stops reading.
async fn unreadable(w: &World) -> String {
    w.kit().await;
    w.tick().await;
    w.watch_session(false);
    w.advance(5);
    w.tick().await;
    let watch = {
        let t = w.host.orca.terminals.lock().unwrap();
        t.iter().find(|(_, t)| t.title == "factorium watch").map(|(h, _)| h.clone()).expect("the watch's terminal")
    };
    w.store_reads_fail("data");
    watch
}

#[tokio::test]
async fn a_look_that_cannot_read_the_store_briefs_the_watch_once_an_hour() {
    let w = World::new();
    let watch = unreadable(&w).await;
    let before = w.terminal(&watch).sent.len();

    w.advance(5);
    let failed = w.app.tick().await.expect_err("the store does not read");
    assert_eq!(failed.code, "store", "{failed}");
    let sent = w.terminal(&watch).sent;
    assert_eq!(sent.len(), before + 1, "one brief: {sent:#?}");
    let brief = sent.last().unwrap();
    assert!(brief.contains("outside its store") && brief.contains("no such column"), "the error: {brief}");
    assert!(brief.contains(env!("CARGO_PKG_VERSION")), "the version: {brief}");
    assert!(brief.contains("daemon.log") && brief.contains("(1 so far)"), "{brief}");

    for _ in 0..11 {
        w.advance(5);
        w.app.tick().await.unwrap_err();
    }
    assert_eq!(w.terminal(&watch).sent.len(), before + 1, "no second brief within the hour");

    w.advance(5);
    w.app.tick().await.unwrap_err();
    let sent = w.terminal(&watch).sent;
    assert_eq!(sent.len(), before + 2, "again once the hour is past");
    assert!(sent.last().unwrap().contains("(13 so far)"), "{}", sent.last().unwrap());

    // Another error is an episode of its own: briefed at once.
    w.store_reads_fail("kind");
    w.advance(5);
    w.app.tick().await.unwrap_err();
    let sent = w.terminal(&watch).sent;
    assert_eq!(sent.len(), before + 3, "a new error, a new brief");
    assert!(sent.last().unwrap().contains("(1 so far)"), "{}", sent.last().unwrap());
}

#[tokio::test]
async fn a_daemon_whose_start_cannot_record_goes_on_to_its_ticks() {
    let w = World::new();
    unreadable(&w).await;
    let ran = w.app.run(&std::sync::atomic::AtomicBool::new(true)).await.unwrap();
    assert_eq!(ran["ticks"], 0, "{ran}");
}

#[tokio::test]
async fn doctor_that_cannot_read_the_store_says_so_as_its_one_finding() {
    let w = World::new();
    unreadable(&w).await;
    let doctor = w.app.doctor().await.unwrap();
    let findings = doctor["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{doctor}");
    let what = findings[0]["what"].as_str().unwrap();
    assert!(what.contains("no such column") && what.contains(&*w.data().root().to_string_lossy()), "{what}");
    assert!(doctor["unreadable"].is_string(), "{doctor}");
}
