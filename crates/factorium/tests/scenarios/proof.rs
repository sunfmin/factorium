//! A swap's proof (factorium#141) beside the ticks, on a copy a writer does
//! not restart (factorium#241): seen 2026-11-22, the daemon's proof of
//! 0.1.80_1 made one tick take 118764 ms, 107864 ms of it proving, in an
//! online backup of the live 340 MB database that restarted on each write.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use factorium::Tick;
use serde_json::Value;

use crate::fixture::{CLAUDE, NEEDS_NEWER, OPENS, STUCK, World, program, proved};

/// One that answers `doctor` on the database, as [`OPENS`] does, seconds later.
const SLOW: &str = r#"#!/bin/sh
sleep 6
echo '{"findings": []}'
"#;

/// One that answers `doctor` seconds later that it cannot open the database, as [`NEEDS_NEWER`].
const SLOW_NEEDS_NEWER: &str = r#"#!/bin/sh
echo "$2" >> "$(dirname "$0")/proved"
sleep 6
echo 'Error: no such column: handed_back'; exit 1
"#;

/// Ticks until one asks for the restart onto the newer binary, each within
/// `tick_bound`: answers how many it took.
async fn ticks_until_restart(w: &World, tick_bound: Duration) -> usize {
    ticks_until(w, tick_bound, |t| t.restart.is_some()).await.len()
}

/// Ticks until one is `done`, each within `tick_bound`: answers them all. A
/// tick that blocks on a proof cannot be timed out on its own runtime: past
/// [`STUCK`] a watchdog ends the run, red, so it never holds the heavy queue
/// (factorium#241's old copy never ended).
async fn ticks_until(w: &World, tick_bound: Duration, done: impl Fn(&Tick) -> bool) -> Vec<Tick> {
    let (_done, watched) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = watched.recv_timeout(STUCK) {
            eprintln!("a tick blocked past {}s: the proof does not run beside the ticks", STUCK.as_secs());
            std::process::exit(101);
        }
    });
    let (begun, mut ticks) = (Instant::now(), Vec::new());
    loop {
        let at = Instant::now();
        let t = w.tick_alone().await;
        assert!(at.elapsed() < tick_bound, "tick {} waited on the proof: {:?}", ticks.len() + 1, at.elapsed());
        assert!(!t.slow, "{t:?}");
        let ended = done(&t);
        ticks.push(t);
        if ended {
            return ticks;
        }
        assert!(begun.elapsed() < STUCK, "the proof ends within its bound");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_swaps_proof_of_a_slow_binary_runs_beside_the_ticks() {
    let w = World::new();
    w.running_as(&w.app, "web");
    w.links_newer("web", SLOW);
    let begun = Instant::now();
    let ticks = ticks_until_restart(&w, Duration::from_secs(3)).await;
    assert!(ticks > 1, "the first tick only started it");
    assert!(begun.elapsed() >= Duration::from_secs(6), "it swaps once proven");
}

#[tokio::test]
async fn a_proof_on_a_database_written_to_all_the_time_ends_within_its_bound() {
    let w = World::new();
    w.running_as(&w.app, "web");
    // 64 MB to copy, written to every 5 ms (a busy host's every process writes it): an online
    // backup starts over on each write from another connection, and never ends.
    let ballast = "CREATE TABLE ballast(b BLOB); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n \
                   WHERE i < 16000) INSERT INTO ballast SELECT randomblob(4096) FROM n;";
    rusqlite::Connection::open(&w.db).unwrap().execute_batch(ballast).unwrap();
    let (stop, db) = (Arc::new(AtomicBool::new(false)), w.db.clone());
    let writes = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let c = rusqlite::Connection::open(db).unwrap();
            c.busy_timeout(Duration::from_secs(5)).unwrap();
            while !stop.load(Ordering::Relaxed) {
                c.execute("INSERT INTO ballast VALUES (randomblob(16))", []).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    w.links_newer("web", OPENS);
    let begun = Instant::now();
    ticks_until_restart(&w, Duration::from_secs(3)).await;
    stop.store(true, Ordering::Relaxed);
    writes.join().unwrap();
    assert!(begun.elapsed() < Duration::from_secs(10), "copied in one read: {:?}", begun.elapsed());
}

/// Homebrew links `version` of `formula`, a binary that is `script`.
fn links(w: &World, formula: &str, version: &str, script: &str) {
    let keg = w.dir.join("brew/Cellar").join(formula).join(version);
    program(&keg.join("bin/factorium"), script);
    let opt = w.dir.join("brew/opt").join(formula);
    std::fs::remove_file(&opt).unwrap();
    std::os::unix::fs::symlink(&keg, &opt).unwrap();
}

#[tokio::test]
async fn a_binary_proven_failing_on_the_host_leaves_the_next_version_to_be_proven() {
    let w = World::new();
    w.running_as(&w.app, "web");
    links(&w, "web", "0.1.0", NEEDS_NEWER);
    // Its proof fails, kept on the host: no tick swaps to it.
    let kept = || {
        std::fs::read_dir(w.data().proofs())
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "json"))
    };
    let begun = Instant::now();
    while !kept() {
        assert_eq!(w.tick().await.restart, None);
        assert!(begun.elapsed() < STUCK, "the proof ends");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(w.tick().await.restart, None, "not to a binary that cannot open the database");
    assert_eq!(proved(&w, "web", "0.1.0").len(), 1);
    // The next version is a binary of its own: proven, and swapped to.
    links(&w, "web", "0.1.1", OPENS);
    ticks_until_restart(&w, Duration::from_secs(3)).await;
    assert_eq!(proved(&w, "web", "0.1.1").len(), 1);
}

/// Lands, promotes and cuts one change of `web`, whose cut installs the
/// daemon's own formula, its binary `script`: ticks, each within 3 s, until
/// one records the cut, at least 6 s on (the binary's `doctor`). Answers the
/// cut as recorded, and the restart the tick that recorded it asks for.
async fn cut_proven_beside_the_ticks(w: &World, script: &str) -> (Value, Option<String>) {
    w.running_as(&w.app, "web");
    w.next_binary("web", script);
    let wk = w.working(CLAUDE, "a").await;
    w.done(&wk, "a", "x").await;
    let begun = Instant::now();
    let cut = |t: &Tick| t.lines.iter().find(|l| l["cut"] == "web").cloned();
    let ticks = ticks_until(w, Duration::from_secs(3), |t| cut(t).is_some()).await;
    assert!(begun.elapsed() >= Duration::from_secs(6), "recorded once its proof ended: {:?}", begun.elapsed());
    let proving = ticks.iter().rev().skip(1).take_while(|t| cut(t).is_none()).count();
    assert!(proving > 1, "ticks came while it was proven: {proving}");
    let last = ticks.last().unwrap();
    (cut(last).unwrap(), last.restart.clone())
}

/// Seen in factorium#241: a cut's proof ran in the tick that finished the
/// cut, and a `doctor` of the new binary has up to 120 s, past half the hang
/// bound (factorium#242).
#[tokio::test]
async fn a_cuts_proof_of_a_slow_binary_runs_beside_the_ticks_and_the_cut_is_then_made() {
    let w = World::new();
    let (cut, restart) = cut_proven_beside_the_ticks(&w, SLOW).await;
    assert!(cut["failed"].is_null() && cut["installed"] == "web" && cut["version"] == "v0.1.0", "{cut}");
    assert_eq!(w.recorded("CutMade"), 1);
    assert!(restart.is_some(), "the tick that records it swaps to the proven binary");
}

#[tokio::test]
async fn a_cuts_proof_of_a_slow_binary_that_fails_records_the_cut_failed_with_the_proofs_reason() {
    let w = World::new();
    let (cut, restart) = cut_proven_beside_the_ticks(&w, SLOW_NEEDS_NEWER).await;
    let why = cut["failed"].as_str().expect("the cut fails");
    assert!(why.contains("installed web, but") && why.contains("no such column: handed_back"), "{why}");
    assert_eq!(restart, None, "the old binary runs on");
    let finding = w.findings().await[0]["what"].as_str().unwrap().to_string();
    assert!(finding.contains("cutting v0.1.0 failed"), "{finding}");
}
