//! A line that stopped: whatever it owes past its bound is a finding with a
//! fix the watch can carry out, and once the watch carries it out the line
//! moves. The stall staged here is a heavy-queue entry whose owner no longer
//! asks for it (as on 2026-11-17, when 119 entries waited and none ran), and
//! a worker hung in an open turn.

use std::path::PathBuf;

use factorium::host::Proc;
use factum::Uuid;
use serde_json::Value;

use crate::line::land;
use crate::queue::{STUCK, heavy, running_on, while_busy};
use crate::{UP, World, id, worker};

/// The finding whose key is `key`, if doctor answers one.
async fn finding(w: &World, key: &str) -> Option<Value> {
    let d = w.run().doctor().await.unwrap();
    d["findings"].as_array().unwrap().iter().find(|f| f["key"] == key).cloned()
}

/// Stages an entry at the head of the heavy queue whose owner no longer asks
/// for it: a CLI run's command queued behind a running one, then given up
/// while its process lives on. Answers its id.
async fn forgotten_entry(w: &World) -> Uuid {
    while_busy(w, async {
        let cli = w.run();
        let mut asks = std::pin::pin!(cli.worker_exclusive(None, &w.dir, "echo forgotten"));
        let queued = async {
            while !heavy(w).await.contains(&"echo forgotten".to_string()) {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! { _ = &mut asks => panic!("it ran beside the busy command"), _ = queued => {} }
    })
    .await;
    let d = w.run().doctor().await.unwrap();
    let entry = d["heavy"].as_array().unwrap().iter().find(|e| e["what"] == "echo forgotten").cloned();
    entry.expect("it stays queued")["id"].as_str().unwrap().parse().unwrap()
}

/// A checkout, a kit and issues `titles`, each started by a tick.
async fn working(w: &World, titles: &[&str]) {
    w.checkout("web").await;
    w.kit().await;
    // The workers keep working all along: none is silent.
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    for t in titles {
        w.app.issue_add("web", t, "", false, &[]).await.unwrap();
        w.tick().await;
    }
}

#[tokio::test]
async fn a_heavy_queue_that_starts_nothing_past_the_stall_bound_is_a_finding_until_its_head_is_taken_off() {
    let w = World::new();
    let stale = forgotten_entry(&w).await;
    let key = format!("queue:{stale}");
    w.advance(10);
    assert_eq!(finding(&w, &key).await, None, "at the stall bound of 10 min, not past it");
    w.advance(1);
    let found = finding(&w, &key).await.expect("nothing runs and its oldest has waited 11 min");
    assert!(
        found["what"].as_str().unwrap().contains("started nothing for 11 min: 1 waiting and nothing runs"),
        "{found}"
    );
    assert!(found["what"].as_str().unwrap().contains("echo forgotten"), "{found}");
    assert!(found["fix"].as_str().unwrap().contains("factorium unqueue"), "{found}");

    // The watch's fix.
    assert_eq!(w.run().unqueue(stale).await.unwrap()["unqueued"]["what"], "echo forgotten");
    let next = tokio::time::timeout(STUCK, w.run().worker_exclusive(None, &w.dir, "echo next")).await;
    assert_eq!(next.expect("the queue moves").unwrap()["ok"], true);
    assert_eq!(finding(&w, &key).await, None);
    assert_eq!(w.run().unqueue(stale).await.unwrap_err().code, "no_entry");
}

#[tokio::test]
async fn a_heavy_queue_held_by_what_runs_is_a_finding_only_past_the_hold_bound_and_the_stall_bound() {
    let w = World::new();
    // No room: nothing starts beside what runs.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    running_on(&w).await;
    let cli = w.run();
    let waiting = {
        let mut asks = std::pin::pin!(cli.worker_exclusive(None, &w.dir, "echo behind"));
        let queued = async {
            while !heavy(&w).await.contains(&"echo behind".to_string()) {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! { _ = &mut asks => panic!("it ran beside the running one"), _ = queued => {} }
        let d = w.run().doctor().await.unwrap();
        let e = d["heavy"].as_array().unwrap().iter().find(|e| e["what"] == "echo behind").cloned().unwrap();
        e["id"].as_str().unwrap().to_string()
    };
    let key = format!("queue:{waiting}");
    // The hold bound (30 min) and the stall bound (10 min) after it queued.
    w.advance(40);
    assert_eq!(finding(&w, &key).await, None, "what runs may yet end");
    w.advance(1);
    let found = finding(&w, &key).await.expect("past both bounds");
    assert!(found["what"].as_str().unwrap().contains("for 41 min: 1 waiting and running: `"), "{found}");
}

#[tokio::test]
async fn a_landing_no_gate_runs_for_past_the_stall_bound_is_a_finding_and_lands_once_the_queue_moves() {
    let w = World::new();
    working(&w, &["a"]).await;
    let stale = forgotten_entry(&w).await;
    let wk = worker(&w.app).await;
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "a", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    assert!(w.tick().await.landed.is_empty(), "its gate waits behind the forgotten entry");
    let key = format!("landing:{}", id(&wk));
    w.advance(9);
    w.tick().await;
    assert_eq!(finding(&w, &key).await, None, "within the stall bound");
    w.advance(2);
    w.tick().await;
    let found = finding(&w, &key).await.expect("landing 11 min and no gate runs");
    let what = found["what"].as_str().unwrap();
    assert!(what.contains("web#1") && what.contains(&stale.to_string()), "it names what holds it: {found}");
    assert!(what.contains("waited 11 min to land") && what.contains("queued 11 min ago"), "{found}");
    // Held, its landings wait by the operator's word, not by a stall.
    w.app.hold("web", Some("repairs")).await.unwrap();
    assert_eq!(finding(&w, &key).await, None);
    w.app.hold("web", None).await.unwrap();

    w.run().unqueue(stale).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1", "the line moves");
    assert_eq!(finding(&w, &key).await, None);
}

#[tokio::test]
async fn a_landing_unpromoted_past_the_promotion_bound_is_a_finding_and_is_promoted_once_the_queue_moves() {
    let w = World::new();
    // #2 keeps working: the checkout is never idle, so only the bound promotes.
    working(&w, &["a", "b"]).await;
    let landed = land(&w, 1, "a").await;
    let stale = forgotten_entry(&w).await;
    w.advance(60);
    w.tick().await;
    assert_eq!(finding(&w, "unpromoted:web").await, None, "at the promotion bound, not past it");
    w.advance(1);
    let t = w.tick().await;
    assert!(t.lines.is_empty(), "its slow gate waits behind the forgotten entry: {:?}", t.lines);
    let found = finding(&w, "unpromoted:web").await.expect("past the promotion bound and no promotion runs");
    assert!(found["what"].as_str().unwrap().contains("web#1"), "{found}");

    w.run().unqueue(stale).await.unwrap();
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "the line moves: {:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert_eq!(finding(&w, "unpromoted:web").await, None);
}

#[tokio::test]
async fn a_promotion_uncut_past_the_cut_bound_is_a_finding_and_is_cut_once_the_queue_moves() {
    let w = World::new();
    working(&w, &["a", "b"]).await;
    land(&w, 1, "a").await;
    assert_eq!(w.app.promote("web").await.unwrap()["promoted"], "web");
    let stale = forgotten_entry(&w).await;
    w.advance(24 * 60);
    w.tick().await;
    assert_eq!(finding(&w, "uncut:web").await, None, "at the cut bound, not past it");
    w.advance(1);
    assert!(w.tick().await.lines.is_empty(), "the cut waits behind the forgotten entry");
    let found = finding(&w, "uncut:web").await.expect("past the cut bound and no cut runs");
    assert!(found["fix"].as_str().unwrap().contains("factorium cut web"), "{found}");

    w.run().unqueue(stale).await.unwrap();
    let t = w.tick().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("the line moves");
    assert_eq!(cut["version"], "v0.1.0", "{cut}");
    assert_eq!(finding(&w, "uncut:web").await, None);
}

#[tokio::test]
async fn a_worker_hung_in_an_open_turn_past_the_silent_bound_goes_to_the_watch_and_lands_once_restarted() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let (cwd, me, terminal) = (wk["worktree"].as_str().unwrap(), id(&wk), wk["terminal"].as_str().unwrap());
    crate::harnesses::write_session(&w, factorium_harness::Harness::ClaudeCode, None, cwd, true, 1_800_000_000);
    let cpu = |percent: f64| {
        let p = Proc { pid: 7, ppid: 1, cpu: percent, rss_kib: 1 << 20, command: "cargo test".into(), worker: None };
        *w.host.procs.lock().unwrap() = vec![Proc { worker: Some(me.to_string()), ..p }];
    };
    let status = || async { worker(&w.app).await["status"].as_str().unwrap().to_string() };

    // Busy in its turn: working, however long. 2% of a core is the least
    // that is not idle.
    cpu(2.0);
    w.tick().await;
    w.advance(31);
    w.tick().await;
    assert_eq!(status().await, "working", "its processes use the CPU");
    // Idle, but its screen moves: still working.
    cpu(0.1);
    w.tick().await;
    w.advance(20);
    w.set_screen(terminal, &[UP, "test result: ok. 3 passed"]);
    w.tick().await;
    w.advance(20);
    w.tick().await;
    assert_eq!(status().await, "working", "its screen changed 20 min ago");
    // Idle and its screen still: hung once past the silent bound of 30 min.
    w.advance(10);
    w.tick().await;
    assert_eq!(status().await, "working", "30 min is the bound, not past it");
    w.advance(1);
    w.tick().await;
    let hung = worker(&w.app).await;
    assert_eq!(hung["status"], "waiting");
    assert!(hung["why"].as_str().unwrap().starts_with("hung for 31 min"), "{hung}");
    let found = finding(&w, &format!("worker:{me}")).await.expect("doctor answers it");
    assert!(found["what"].as_str().unwrap().contains("hung"), "{found}");

    // The watch's fix: it tells the worker what blocks it.
    w.app
        .watch_restart(me, "your test blocks reading a FIFO nobody writes; end it and fix the test.", false)
        .await
        .unwrap();
    assert_eq!(status().await, "working");
    assert_eq!(finding(&w, &format!("worker:{me}")).await, None);
    w.commit(&PathBuf::from(cwd), "a", "x");
    w.app.worker_done(me).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1", "the line moves");
}
