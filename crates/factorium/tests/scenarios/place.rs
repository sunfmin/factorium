//! A heavy entry keeps its place when its asker asks again (factorium#193):
//! seen 2026-11-21, a cut's new daemon refused thirteen waiters of the old
//! binary as stale; each left the queue, and asked again, queued behind
//! all the rest (a 5-second test waited 1231 s).

use factum::Uuid;

use crate::fixture::{POLL, STUCK, World, until_file};

#[tokio::test]
async fn waiters_a_restarted_daemon_refuses_start_in_the_order_they_were_queued_once_asked_again() {
    let w = World::new();
    let (busy, log) = (w.dir.join("busy"), w.dir.join("log"));
    let line = |n: &str| format!("echo {n} >> {}", log.display());
    let ran = || -> Vec<String> {
        std::fs::read_to_string(&log).unwrap_or_default().split_whitespace().map(String::from).collect()
    };
    // No room beside what runs: one at a time, oldest first.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let [a, b, c, d] = std::array::from_fn(|_| Some(Uuid::now_v7()));
    let (held, holds) = (w.run(), w.holds(&busy));
    let old: [_; 3] = std::array::from_fn(|_| w.run().under_rules("1.0.0"));
    let new: [_; 4] = std::array::from_fn(|_| w.run().under_rules("1.0.1"));
    let asks = async {
        until_file(&busy.join("started")).await;
        // Three waiters of the old binary, queued in turn.
        let w = &w;
        let [la, lb, lc, ld] = ["a", "b", "c", "d"].map(line);
        // Once `n` are queued, a millisecond on: each asks at a time of its own, as on a host.
        let queued = |n: usize| async move {
            w.until(async || w.queued().await.len() > n).await;
            w.clock.advance(1);
        };
        let refused = tokio::join!(
            old[0].worker_exclusive(a, &w.dir, &la),
            async {
                queued(1).await;
                old[1].worker_exclusive(b, &w.dir, &lb).await
            },
            async {
                queued(2).await;
                old[2].worker_exclusive(c, &w.dir, &lc).await
            },
            async {
                w.until(async || w.queued().await.len() == 4 && w.clock.asleep() == 4).await;
                // The daemon restarts as a new version: the old waiters are refused as they ask.
                w.run().under_rules("1.0.1").daemon_start().await.unwrap();
                w.clock.advance(POLL);
            },
        );
        for r in [refused.0, refused.1, refused.2] {
            assert_eq!(r.expect_err("refused under replaced rules").code, "stale");
        }
        assert_eq!(w.queued().await.len(), 1, "they left the queue: only the busy one is in it");
        // A new waiter queues after the restart; then the three ask again, the last first.
        let again = tokio::join!(
            new[3].worker_exclusive(d, &w.dir, &ld),
            async {
                queued(1).await;
                new[2].worker_exclusive(c, &w.dir, &lc).await
            },
            async {
                queued(2).await;
                new[1].worker_exclusive(b, &w.dir, &lb).await
            },
            async {
                queued(3).await;
                new[0].worker_exclusive(a, &w.dir, &la).await
            },
            async {
                queued(4).await;
                std::fs::write(busy.join("release"), "").unwrap();
                // Each asks again a poll on, until all four ran.
                while ran().len() < 4 {
                    w.clock.advance(POLL);
                    tokio::time::sleep(crate::fixture::LOOK).await;
                }
            },
        );
        for r in [again.0, again.1, again.2, again.3] {
            assert_eq!(r.unwrap()["ok"], true);
        }
    };
    let (held, ()) =
        tokio::time::timeout(STUCK, async { tokio::join!(held.worker_exclusive(None, &w.dir, &holds), asks) })
            .await
            .expect("all four run");
    assert_eq!(held.unwrap()["ok"], true);
    assert_eq!(ran(), ["a", "b", "c", "d"] as [&str; 4], "in the order first queued, none at the back");
}
