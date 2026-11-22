//! A swap's proof (factorium#141) beside the ticks, on a copy a writer does
//! not restart (factorium#241): seen 2026-11-22, the daemon's proof of
//! 0.1.80_1 made one tick take 118764 ms, 107864 ms of it proving, in an
//! online backup of the live 340 MB database that restarted on each write.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::fixture::{OPENS, STUCK, World};

/// One that answers `doctor` on the database, as [`OPENS`] does, seconds later.
const SLOW: &str = r#"#!/bin/sh
sleep 6
echo '{"findings": []}'
"#;

/// Ticks until one asks for the restart onto the newer binary, each within
/// `tick_bound`: answers how many it took.
async fn ticks_until_restart(w: &World, tick_bound: Duration) -> usize {
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
async fn a_proof_on_a_database_written_to_every_second_ends_within_its_bound() {
    let w = World::new();
    w.running_as(&w.app, "web");
    // 16 MB to copy: an online backup's 100 pages a step, 250 ms apart, takes 10 s.
    let ballast = "CREATE TABLE ballast(b BLOB); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n \
                   WHERE i < 4000) INSERT INTO ballast SELECT randomblob(4096) FROM n;";
    rusqlite::Connection::open(&w.db).unwrap().execute_batch(ballast).unwrap();
    let (stop, db) = (Arc::new(AtomicBool::new(false)), w.db.clone());
    let writes = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let c = rusqlite::Connection::open(db).unwrap();
            c.busy_timeout(Duration::from_secs(5)).unwrap();
            while !stop.load(Ordering::Relaxed) {
                c.execute("INSERT INTO ballast VALUES (randomblob(16))", []).unwrap();
                std::thread::sleep(Duration::from_secs(1));
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
