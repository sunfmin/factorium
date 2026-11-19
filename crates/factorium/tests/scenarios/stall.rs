//! A line that stopped: whatever it owes past its bound is a finding with a
//! fix the watch can carry out, and once the watch carries it out the line
//! moves. The stall staged here is a heavy-queue entry whose owner no longer
//! asks for it (as on 2026-11-17, when 119 entries waited and none ran), and
//! a worker hung in an open turn.

use std::path::PathBuf;

use factorium::host::Proc;
use factum::Uuid;
use serde_json::Value;

use crate::fixture::{CLAUDE, STUCK, UP, World, id, until_file, worker};

/// Stages an entry at the head of the heavy queue whose owner no longer asks
/// for it: a CLI run's command queued behind a running one, then given up
/// while its process lives on. Answers its id.
async fn forgotten_entry(w: &World) -> Uuid {
    w.while_busy(async {
        let cli = w.run();
        let mut asks = std::pin::pin!(cli.worker_exclusive(None, &w.dir, "echo forgotten"));
        let queued = w.until_queued("echo forgotten");
        tokio::select! { _ = &mut asks => panic!("it ran beside the busy command"), _ = queued => {} }
    })
    .await;
    w.queued_entry("echo forgotten").await["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn a_heavy_queue_that_starts_nothing_past_the_stall_bound_is_a_finding_until_its_head_is_taken_off() {
    let w = World::new();
    let stale = forgotten_entry(&w).await;
    let key = format!("queue:{stale}");
    w.advance(10);
    assert_eq!(w.finding(&key).await, None, "at the stall bound of 10 min, not past it");
    w.advance(1);
    let found = w.finding(&key).await.expect("nothing runs and its oldest has waited 11 min");
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
    assert_eq!(w.finding(&key).await, None);
    assert_eq!(w.run().unqueue(stale).await.unwrap_err().code, "no_entry");
}

#[tokio::test]
async fn a_heavy_queue_held_by_what_runs_is_a_finding_only_past_the_hold_bound_and_the_stall_bound() {
    let w = World::new();
    // No room: nothing starts beside what runs.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    w.running_on().await;
    let cli = w.run();
    let waiting = {
        let mut asks = std::pin::pin!(cli.worker_exclusive(None, &w.dir, "echo behind"));
        let queued = w.until_queued("echo behind");
        tokio::select! { _ = &mut asks => panic!("it ran beside the running one"), _ = queued => {} }
        w.queued_entry("echo behind").await["id"].as_str().unwrap().to_string()
    };
    let key = format!("queue:{waiting}");
    // The hold bound (30 min) and the stall bound (10 min) after it queued.
    w.advance(40);
    assert_eq!(w.finding(&key).await, None, "what runs may yet end");
    w.advance(1);
    let found = w.finding(&key).await.expect("past both bounds");
    assert!(found["what"].as_str().unwrap().contains("for 41 min: 1 waiting and running: `"), "{found}");
}

#[tokio::test]
async fn a_landing_no_gate_runs_for_past_the_stall_bound_is_a_finding_and_lands_once_the_queue_moves() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    let stale = forgotten_entry(&w).await;
    let wk = worker(&w.app).await;
    w.done(&wk, "a", "x").await;
    assert!(w.tick().await.landed.is_empty(), "its gate waits behind the forgotten entry");
    let key = format!("landing:{}", id(&wk));
    w.advance(9);
    w.tick().await;
    assert_eq!(w.finding(&key).await, None, "within the stall bound");
    w.advance(2);
    w.tick().await;
    let found = w.finding(&key).await.expect("landing 11 min and no gate runs");
    let what = found["what"].as_str().unwrap();
    assert!(what.contains("web#1") && what.contains(&stale.to_string()), "it names what holds it: {found}");
    assert!(what.contains("waited 11 min to land") && what.contains("queued 11 min ago"), "{found}");
    // Held, its landings wait by the operator's word, not by a stall.
    w.app.hold("web", Some("repairs")).await.unwrap();
    assert_eq!(w.finding(&key).await, None);
    w.app.hold("web", None).await.unwrap();

    w.run().unqueue(stale).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1", "the line moves");
    assert_eq!(w.finding(&key).await, None);
}

#[tokio::test]
async fn a_landing_unpromoted_past_the_promotion_bound_is_a_finding_and_is_promoted_once_the_queue_moves() {
    let w = World::new();
    // #2 keeps working: the checkout is never idle, so only the bound promotes.
    w.two_at_work().await;
    let landed = w.lands("web", 1, "a").await.commit;
    let stale = forgotten_entry(&w).await;
    w.advance(60);
    w.tick().await;
    assert_eq!(w.finding("unpromoted:web").await, None, "at the promotion bound, not past it");
    w.advance(1);
    let t = w.tick().await;
    assert!(t.lines.is_empty(), "its slow gate waits behind the forgotten entry: {:?}", t.lines);
    let found = w.finding("unpromoted:web").await.expect("past the promotion bound and no promotion runs");
    assert!(found["what"].as_str().unwrap().contains("web#1"), "{found}");

    w.run().unqueue(stale).await.unwrap();
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "the line moves: {:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert_eq!(w.finding("unpromoted:web").await, None);
}

#[tokio::test]
async fn a_promotion_uncut_past_the_cut_bound_is_a_finding_and_is_cut_once_the_queue_moves() {
    let w = World::new();
    w.two_at_work().await;
    w.lands("web", 1, "a").await;
    assert_eq!(w.app.promote("web").await.unwrap()["promoted"], "web");
    let stale = forgotten_entry(&w).await;
    w.advance(24 * 60);
    w.tick().await;
    assert_eq!(w.finding("uncut:web").await, None, "at the cut bound, not past it");
    w.advance(1);
    assert!(w.tick().await.lines.is_empty(), "the cut waits behind the forgotten entry");
    let found = w.finding("uncut:web").await.expect("past the cut bound and no cut runs");
    assert!(found["fix"].as_str().unwrap().contains("factorium cut web"), "{found}");

    w.run().unqueue(stale).await.unwrap();
    let t = w.tick().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("the line moves");
    assert_eq!(cut["version"], "v0.1.0", "{cut}");
    assert_eq!(w.finding("uncut:web").await, None);
}

/// Doctor knows each line move by the name its heavy command is queued by:
/// a move whose command runs long is no stall, and a landing waiting beside
/// it, with no fast gate running for it, is one. #1 lands `declared`, whose
/// `{long}` runs until the doctor has looked; after `minutes` the tick runs
/// the move (a cut once promoted by hand); and `key` is the move's stall.
async fn runs_long_beside_a_landing(w: &World, declared: &str, minutes: i64, key: &str) -> Vec<Value> {
    // #2 keeps working: the checkout is never idle, so only the bound moves it.
    w.two_at_work().await;
    let dir = w.dir.join("long");
    let long = w.holds(&dir);
    let (started, release) = (dir.join("started"), dir.join("release"));
    w.done(&w.worker_on("web", 1).await, "factorium.toml", &declared.replace("{long}", &long)).await;
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
    if key == "uncut:web" {
        assert_eq!(w.app.promote("web").await.unwrap()["promoted"], "web");
    }
    w.advance(minutes);
    let looks = async {
        until_file(&started).await;
        w.done(&w.worker_on("web", 2).await, "b", "x").await;
        w.advance(11);
        let landing = w.finding(&format!("landing:{}", id(&w.worker_on("web", 2).await))).await;
        let stalled = w.finding(key).await;
        std::fs::write(&release, "").unwrap();
        (landing, stalled)
    };
    let (t, (landing, stalled)) =
        tokio::time::timeout(STUCK, async { tokio::join!(w.tick(), looks) }).await.expect("the long command ends");
    assert_eq!(stalled, None, "its command runs");
    let landing = landing.expect("no fast gate runs for #2");
    assert!(landing["what"].as_str().unwrap().contains("web#2"), "{landing}");
    t.lines
}

#[tokio::test]
async fn a_promotion_whose_slow_gate_runs_long_is_no_stall_and_a_landing_beside_it_is_one() {
    let w = World::new();
    let lines = runs_long_beside_a_landing(&w, "[gate]\nslow = \"{long}\"\n", 61, "unpromoted:web").await;
    assert!(lines.iter().any(|l| l["promoted"] == "web"), "{lines:?}");
}

#[tokio::test]
async fn a_cut_that_runs_long_is_no_stall_and_a_landing_beside_it_is_one() {
    let w = World::new();
    let declared = "[cut]\nsteps = [\"{long}\"]\nformula = \"web\"\n";
    let lines = runs_long_beside_a_landing(&w, declared, 24 * 60 + 1, "uncut:web").await;
    assert!(lines.iter().any(|l| l["cut"] == "web"), "{lines:?}");
}

#[tokio::test]
async fn a_worker_hung_in_an_open_turn_past_the_silent_bound_goes_to_the_watch_and_lands_once_restarted() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    let (cwd, me, terminal) = (wk["worktree"].as_str().unwrap(), id(&wk), wk["terminal"].as_str().unwrap());
    w.claude_session(cwd, true, 1_800_000_000);
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
    let found = w.finding(&format!("worker:{me}")).await.expect("doctor answers it");
    assert!(found["what"].as_str().unwrap().contains("hung"), "{found}");

    // The watch's fix: it tells the worker what blocks it.
    w.app
        .watch_restart(me, "your test blocks reading a FIFO nobody writes; end it and fix the test.", false)
        .await
        .unwrap();
    assert_eq!(status().await, "working");
    assert_eq!(w.finding(&format!("worker:{me}")).await, None);
    w.done(&wk, "a", "x").await;
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1", "the line moves");
}

#[tokio::test]
async fn a_promotion_past_when_it_falls_due_goes_ahead_of_workers_waiting_entries_and_one_just_due_keeps_its_place() {
    let w = World::new();
    // #2 keeps working: the checkout is never idle, so only the bound promotes.
    w.two_at_work().await;
    let two = id(&w.worker_on("web", 2).await);
    let landed = w.lands("web", 1, "a").await.commit;
    // It falls due however busy at the promotion bound less the hold bound: 30 min.
    w.advance(29);
    // No room beside what runs: one heavy command at a time.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let dirs: Vec<PathBuf> = ["running", "w1", "w2"].iter().map(|d| w.dir.join(d)).collect();
    let holds: Vec<String> = dirs.iter().map(|d| w.holds(d)).collect();
    let started = |d: &PathBuf| d.join("started").exists();
    let (a, w1, w2) = (w.run(), w.run(), w.run());
    let running = a.worker_exclusive(None, &dirs[0], &holds[0]);
    let line = async {
        until_file(&dirs[0].join("started")).await;
        // Two of #2's entries wait behind what runs, asleep on the clock.
        let one = w1.worker_exclusive(Some(two), &dirs[1], &holds[1]);
        let other = w2.worker_exclusive(Some(two), &dirs[2], &holds[2]);
        let (mut one, mut other) = (std::pin::pin!(one), std::pin::pin!(other));
        let queued = async {
            while w.queued().await.len() < 3 || w.clock.asleep() != 3 {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! {
            _ = &mut one => panic!("it ran beside what runs"),
            _ = &mut other => panic!("it ran beside what runs"),
            _ = queued => {}
        }
        // What runs ends while the clock stands: nothing asks until it moves.
        std::fs::write(dirs[0].join("release"), "").unwrap();
        while w.queued().await.len() > 2 || w.clock.asleep() != 2 {
            tokio::task::yield_now().await;
        }
        w.advance(1);
        // Just due, the slow gate keeps its FIFO place: it queues behind the
        // workers' entries, which ask (at the same time) once the tick did.
        assert!(w.tick().await.lines.is_empty(), "just due, a worker's entry starts ahead of it");
        assert!(w.queued().await.contains(&"slow gate of web".to_string()));
        let script = async {
            let first = loop {
                if let Some(d) = dirs[1..].iter().find(|d| started(d)) {
                    break d.clone();
                }
                tokio::task::yield_now().await;
            };
            let second = dirs[1..].iter().find(|d| **d != first).unwrap().clone();

            // Past when it fell due, it goes ahead of the worker's entry still waiting.
            w.advance(1);
            std::fs::write(first.join("release"), "").unwrap();
            while w.queued().await.len() > 2 {
                tokio::task::yield_now().await;
            }
            let t = w.tick().await;
            assert!(
                t.lines.iter().any(|l| l["promoted"] == "web"),
                "the slow gate starts next, ahead of the worker's entry: {:?}",
                t.lines
            );
            std::fs::write(second.join("release"), "").unwrap();
            w.poll(1).await;
        };
        let (one, other, ()) = tokio::join!(one, other, script);
        assert_eq!(one.unwrap()["ok"], true);
        assert_eq!(other.unwrap()["ok"], true);
    };
    let (ran, ()) = tokio::time::timeout(STUCK, async { tokio::join!(running, line) }).await.expect("nothing hangs");
    assert_eq!(ran.unwrap()["ok"], true);
    assert_eq!(w.remote_tip("web", "main"), landed);
}
