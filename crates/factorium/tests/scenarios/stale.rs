//! The heavy queue under a binary whose proof failed (factorium#241): seen
//! 2026-11-22, v0.1.80 installed but its proof failed, so the daemon ran on
//! as 0.1.79. A 0.1.80 worker's `worker exclusive` asked the queue and set
//! its rules to 0.1.80; every gate of the daemon's was refused as stale
//! from then on and, past its bound, held the head: the heavy queue started
//! nothing for 14 minutes, and the fix could not land.

use crate::fixture::{CLAUDE, NEEDS_NEWER, POLL, STUCK, World, until_file};

#[tokio::test]
async fn a_newer_binary_whose_proof_failed_asking_the_queue_leaves_the_daemons_gates_to_start() {
    let w = World::new();
    w.running_as(&w.app, "web");
    w.app.daemon_start().await.unwrap();
    w.next_binary("web", NEEDS_NEWER);
    let wk = w.working(CLAUDE, "a").await;
    w.done(&wk, "a", "x").await;
    let t = w.tick().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    assert!(cut["failed"].as_str().is_some_and(|f| f.contains("no such column")), "its proof fails: {cut}");
    assert_eq!(t.restart, None, "the old binary runs on");
    // A worker's process of the new binary asks the queue: it runs.
    let newer = w.run().under_rules("99.0.0");
    let ran = tokio::time::timeout(STUCK, newer.worker_exclusive(None, &w.dir, "echo new")).await;
    assert_eq!(ran.expect("it starts at once").unwrap()["ok"], true);
    // The fix: the old daemon's gates still start, and it lands.
    w.app.issue_add("web", "the fix", "", false, &[]).await.unwrap();
    w.tick().await;
    let landed = tokio::time::timeout(STUCK, w.lands("web", 2, "fix")).await.expect("the fix lands");
    assert!(!landed.commit.is_empty());
}

#[tokio::test]
async fn a_stale_entry_at_the_head_holds_none_behind_it_up() {
    let w = World::new();
    let busy = w.dir.join("busy");
    let holds = w.holds(&busy);
    // No room beside what runs: the old waiter waits.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let (held, old) = (w.run(), w.run().under_rules("1.0.0"));
    let flow = async {
        until_file(&busy.join("started")).await;
        let rest = async {
            w.until(async || w.queued().await.len() == 2 && w.clock.asleep() == 2).await;
            // The daemon restarts as a new version; what ran ends.
            w.run().under_rules("1.0.1").daemon_start().await.unwrap();
            std::fs::write(busy.join("release"), "").unwrap();
            w.gone(&holds).await;
            // The old waiter has not asked again: its entry is the oldest. A fresh one starts at once.
            assert_eq!(w.queued().await, ["echo old"]);
            let fresh = w.run().under_rules("1.0.1").worker_exclusive(None, &w.dir, "echo fresh").await;
            assert_eq!(fresh.unwrap()["ok"], true);
            // The old one asks again, and is refused.
            w.clock.advance(POLL);
        };
        tokio::join!(old.worker_exclusive(None, &w.dir, "echo old"), rest).0
    };
    let both = async { tokio::join!(held.worker_exclusive(None, &w.dir, &holds), flow) };
    let (held, old) =
        tokio::time::timeout(STUCK, both).await.expect("the fresh one does not wait behind the stale one");
    assert_eq!(held.unwrap()["ok"], true);
    assert_eq!(old.expect_err("refused under replaced rules").code, "stale");
    assert!(w.queued().await.is_empty());
}
