//! A swap's proof (factorium#141) beside the ticks, on a copy a writer does
//! not restart (factorium#241): seen 2026-11-22, the daemon's proof of
//! 0.1.80_1 made one tick take 118764 ms, 107864 ms of it proving, in an
//! online backup of the live 340 MB database that restarted on each write.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::fixture::{NEEDS_NEWER, OPENS, STUCK, World, program, proved};

/// One that answers `doctor` on the database, as [`OPENS`] does, seconds later.
const SLOW: &str = r#"#!/bin/sh
sleep 6
echo '{"findings": []}'
"#;

/// Ticks until one asks for the restart onto the newer binary, each within
/// `tick_bound`: answers how many it took. A tick that blocks on the proof
/// cannot be timed out on its own runtime: past [`STUCK`] a watchdog ends the
/// run, red, so it never holds the heavy queue (factorium#241's old copy never ended).
async fn ticks_until_restart(w: &World, tick_bound: Duration) -> usize {
    let (_done, watched) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = watched.recv_timeout(STUCK) {
            eprintln!("a tick blocked past {}s: the proof does not run beside the ticks", STUCK.as_secs());
            std::process::exit(101);
        }
    });
    let begun = Instant::now();
    for ticks in 1.. {
        let at = Instant::now();
        let t = w.tick().await;
        assert!(at.elapsed() < tick_bound, "tick {ticks} waited on the proof: {:?}", at.elapsed());
        assert!(!t.slow, "{t:?}");
        if t.restart.is_some() {
            return ticks;
        }
        assert!(begun.elapsed() < STUCK, "the proof ends within its bound");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    unreachable!()
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
