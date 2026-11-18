//! One path through the heavy-command queue: every heavy command, gate runs
//! and cuts included, is held to the hold bound, and the daemon's tick asks
//! the queue once and goes on rather than waiting on it.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use serde_json::json;

use crate::{CLAUDE, World, id, worker, worktree};

/// A hang guard, not a timing bound: far above a loaded run of any one
/// scenario (the slowest: 6s at load 19, 17s under 36 CPU hogs on 18 cores),
/// so it fails only on a hang.
/// How fast the suite runs is the gate's bound to keep.
pub(crate) const STUCK: Duration = Duration::from_secs(180);

pub(crate) async fn until_file(path: &Path) {
    while !path.exists() {
        tokio::task::yield_now().await;
    }
}

/// The heavy commands `doctor` shows, as `what`.
pub(crate) async fn heavy(w: &World) -> Vec<String> {
    let d = w.run().doctor().await.unwrap();
    d["heavy"].as_array().unwrap().iter().map(|e| e["what"].as_str().unwrap().to_string()).collect()
}

/// Runs `during` while another CLI run's heavy command holds the queue, then
/// lets that command end.
pub(crate) async fn while_busy<T>(w: &World, during: impl Future<Output = T>) -> T {
    let dir = w.dir.join("busy");
    let line = &w.holds(&dir);
    let cli = w.run();
    // No room for a second heavy command beside it.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let body = async {
        until_file(&dir.join("started")).await;
        let t = during.await;
        w.host.reading.lock().unwrap().cpu_busy = 10;
        std::fs::write(dir.join("release"), "").unwrap();
        t
    };
    let both = async { tokio::join!(cli.worker_exclusive(None, &dir, line), body) };
    let (held, t) = tokio::time::timeout(STUCK, both).await.expect("nothing waits on the busy queue");
    assert_eq!(held.unwrap()["ok"], true);
    t
}

/// A worker of `web` with a commit of `file`, asking to land.
async fn done(w: &World, file: &str, text: &str) {
    let wk = worker(&w.app).await;
    w.commit(&worktree(&wk), file, text);
    w.app.worker_done(id(&wk)).await.unwrap();
}

#[tokio::test]
async fn a_gate_run_holding_others_up_past_the_hold_bound_is_ended() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    let started = w.dir.join("gate-started");
    let toml = format!("[gate]\nfast = \"touch {} && sleep 30\"\n", started.display());
    done(&w, "factorium.toml", &toml).await;
    // No room for a second heavy command beside the gate.
    w.host.reading.lock().unwrap().cpu_busy = 99;

    let cli = w.run();
    let waiter = cli.worker_exclusive(None, &w.dir, "echo after");
    let others = async {
        until_file(&started).await;
        let mut waiter = std::pin::pin!(waiter);
        let queued = async {
            while !heavy(&w).await.contains(&"echo after".to_string()) {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! { _ = &mut waiter => panic!("it ran past the gate"), _ = queued => {} }
        w.advance(31);
        waiter.await
    };
    let (t, after) =
        tokio::time::timeout(STUCK, async { tokio::join!(w.tick(), others) }).await.expect("the gate is ended");
    let red = t.landed[0]["red"].as_str().unwrap();
    assert!(red.contains("held others up past the hold bound of 1800s"), "{red}");
    assert!(!red.contains("past its limit"), "ended by the hold bound, not its own limit: {red}");
    assert_eq!(after.unwrap()["ok"], true, "the waiting command runs once the gate is ended");
}

#[tokio::test]
async fn a_cut_waits_its_turn_without_holding_the_tick_and_is_made_next_tick() {
    let w = World::new();
    // #2 keeps working all along: the checkout is never idle.
    crate::line::two(&w).await;
    let one = w.app.workers(false).await.unwrap().as_array().unwrap().iter().find(|x| x["number"] == 1).cloned();
    let one = one.unwrap();
    w.commit(&worktree(&one), "a", "x");
    w.app.worker_done(id(&one)).await.unwrap();
    assert_eq!(w.tick().await.landed.len(), 1);
    assert!(w.app.promote("web").await.unwrap()["promoted"] == "web");

    let t = while_busy(&w, async {
        w.advance(24 * 60 + 1);
        w.tick().await
    })
    .await;
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "the cut waits its turn: {:?}", t.lines);
    let t = w.tick().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("the cut is made next tick");
    assert_eq!(cut["version"], "v0.1.0", "{cut}");
}

#[tokio::test]
async fn a_queued_gate_no_longer_due_leaves_the_queue() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    done(&w, "a", "x").await;
    while_busy(&w, async {
        assert_eq!(w.tick().await.landed, Vec::<serde_json::Value>::new(), "the landing's gate waits its turn");
        assert_eq!(heavy(&w).await.len(), 2, "the busy command and the landing's gate");
        // Held, the landing is no longer asked for: its place goes.
        w.app.hold("web", Some("by hand")).await.unwrap();
        w.tick().await;
        assert_eq!(heavy(&w).await.len(), 1, "only the busy command");
    })
    .await;
    assert_eq!(heavy(&w).await, Vec::<String>::new());
    let next = tokio::time::timeout(STUCK, w.run().worker_exclusive(None, &w.dir, "echo next")).await;
    assert_eq!(next.expect("nothing stale waits ahead of it").unwrap()["ok"], json!(true));
}

/// Seen 2026-11-19 (factorium#112): the daemon queued a slow gate, then
/// execed the binary its own cut installed, as the same pid. The new process
/// never asked for the gate again nor ended it, so it held the queue's head.
#[tokio::test]
async fn a_daemon_swapped_in_place_ends_its_queued_job_no_longer_due() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    done(&w, "a", "x").await;
    while_busy(&w, async {
        assert_eq!(w.tick().await.landed, Vec::<serde_json::Value>::new(), "the landing's gate waits its turn");
        assert_eq!(heavy(&w).await.len(), 2, "the busy command and the landing's gate");
        // The swap: a new process, the same pid, none of the old one's memory.
        let swapped = w.run();
        swapped.daemon_start().await.unwrap();
        w.app.hold("web", Some("by hand")).await.unwrap();
        swapped.tick().await.unwrap();
        assert_eq!(heavy(&w).await.len(), 1, "only the busy command");
    })
    .await;
    assert_eq!(heavy(&w).await, Vec::<String>::new());
    let next = tokio::time::timeout(STUCK, w.run().worker_exclusive(None, &w.dir, "echo next")).await;
    assert_eq!(next.expect("nothing stale waits ahead of it").unwrap()["ok"], json!(true));
}

#[tokio::test]
async fn a_landing_waiting_out_a_long_heavy_command_keeps_one_entry_and_starts_next_tick() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    done(&w, "a", "x").await;
    while_busy(&w, async {
        for _ in 0..3 {
            assert_eq!(w.tick().await.landed, Vec::<serde_json::Value>::new(), "the landing waits its turn");
            let gates = heavy(&w).await.into_iter().filter(|h| h == "fast gate of web").count();
            assert_eq!(gates, 1, "one entry for the landing, however many ticks ask");
            // A new second: the landing's merge is a new commit each tick.
            tokio::time::sleep(Duration::from_millis(1100)).await;
        }
    })
    .await;
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1", "its gate starts on the next tick");
    assert_eq!(heavy(&w).await, Vec::<String>::new());
}

/// Stages a heavy command that started and whose asker went away: its
/// entry runs on, and no hold-bound check ends it.
pub(crate) async fn running_on(w: &World) {
    let started = w.dir.join("running-on");
    let cli = w.run();
    let line = format!("touch {}; sleep 1", started.display());
    let mut runs = std::pin::pin!(cli.worker_exclusive(None, &w.dir, &line));
    tokio::select! { _ = &mut runs => panic!("it ended before its asker went"), _ = until_file(&started) => {} }
}

#[tokio::test]
async fn beside_what_runs_one_heavy_command_starts_per_reading() {
    let w = World::new();
    running_on(&w).await;
    // Room, but the reading is the one the running command started on.
    w.host.reading.lock().unwrap().cpu_busy = 10;
    let cli = w.run();
    let mut asks = std::pin::pin!(cli.worker_exclusive(None, &w.dir, "echo next"));
    let entry = async || {
        let d = w.run().doctor().await.unwrap();
        d["heavy"].as_array().unwrap().iter().find(|e| e["what"] == "echo next").cloned()
    };
    let queued = async {
        while entry().await.is_none() {
            tokio::task::yield_now().await;
        }
    };
    tokio::select! { _ = &mut asks => panic!("two starts on one reading"), _ = queued => {} }
    assert_eq!(entry().await.unwrap()["started_at"], json!(null), "it waits for a new reading");
    w.advance(1);
    let ran = tokio::time::timeout(STUCK, asks).await.expect("a new reading starts it");
    assert_eq!(ran.unwrap()["ok"], true);
}

#[tokio::test]
async fn a_landing_whose_gate_runs_long_is_no_stall() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    let gate = w.dir.join("gate");
    let toml = format!("[gate]\nfast = \"{}\"\n", w.holds(&gate));
    let (started, release) = (gate.join("started"), gate.join("release"));
    done(&w, "factorium.toml", &toml).await;
    let me = id(&worker(&w.app).await);
    let looks = async {
        until_file(&started).await;
        w.advance(11);
        let d = w.run().doctor().await.unwrap();
        std::fs::write(&release, "").unwrap();
        d
    };
    let (t, d) = tokio::time::timeout(STUCK, async { tokio::join!(w.tick(), looks) }).await.expect("the gate ends");
    let keys: Vec<_> = d["findings"].as_array().unwrap().iter().map(|f| f["key"].clone()).collect();
    assert!(!keys.contains(&json!(format!("landing:{me}"))), "its gate runs: {keys:?}");
    assert_eq!(t.landed[0]["landed"], "web#1");
}

#[tokio::test]
async fn promote_by_hand_waits_out_a_busy_queue_and_then_promotes() {
    let w = World::new();
    // #2 keeps working all along: the checkout is never idle, so no tick promotes.
    crate::line::two(&w).await;
    let one = w.app.workers(false).await.unwrap().as_array().unwrap().iter().find(|x| x["number"] == 1).cloned();
    let one = one.unwrap();
    w.commit(&worktree(&one), "a", "x");
    w.app.worker_done(id(&one)).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.landed.len(), 1);
    assert_eq!(t.lines, Vec::<serde_json::Value>::new(), "landed, not promoted");

    let dir = w.dir.join("busy");
    let line = w.holds(&dir);
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let cli = w.run();
    let busy = cli.worker_exclusive(None, &dir, &line);
    let promoting = async {
        until_file(&dir.join("started")).await;
        let mut promote = std::pin::pin!(w.app.promote("web"));
        let queued = async {
            while !heavy(&w).await.contains(&"slow gate of web".to_string()) {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! {
            p = &mut promote => panic!("promote answered while the queue was busy: {}", p.unwrap()),
            _ = queued => {}
        }
        w.host.reading.lock().unwrap().cpu_busy = 10;
        std::fs::write(dir.join("release"), "").unwrap();
        promote.await
    };
    let (held, promoted) =
        tokio::time::timeout(STUCK, async { tokio::join!(busy, promoting) }).await.expect("promote gets its turn");
    assert_eq!(held.unwrap()["ok"], true);
    assert_eq!(promoted.unwrap()["promoted"], "web");
    assert_eq!(w.app.promote("web").await.unwrap()["why"], "nothing to promote");
}

/// `web` busy with #2 at work, and #1 landed: answers the landed commit.
async fn busy_with_one_landed(w: &World) -> String {
    w.checkout("web").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    crate::line::land(w, 1, "a").await
}

/// The doctor's finding keys.
async fn finding_keys(w: &World) -> Vec<String> {
    let d = w.run().doctor().await.unwrap();
    d["findings"].as_array().unwrap().iter().map(|f| f["key"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn a_busy_checkout_queues_its_slow_gate_a_hold_bound_early_and_promotes_within_the_bound() {
    let w = World::new();
    let landed = busy_with_one_landed(&w).await;
    // Promotion bound 60 min, hold bound 30 min: due however busy at 30 min.
    w.advance(29);
    assert!(w.tick().await.lines.is_empty(), "busy, within the bound less the lead");
    let t = while_busy(&w, async {
        w.advance(2);
        let t = w.tick().await;
        assert!(heavy(&w).await.contains(&"slow gate of web".to_string()), "queued a hold bound early");
        // The heavy command runs on, within its hold bound, to just short of the promotion bound.
        w.advance(27);
        let keys = finding_keys(&w).await;
        assert!(!keys.iter().any(|k| k.starts_with("unpromoted:") || k.starts_with("queue:")), "{keys:?}");
        t
    })
    .await;
    assert!(!t.lines.iter().any(|l| l["promoted"] == "web"), "the slow gate waited its turn: {:?}", t.lines);
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "promoted within the bound: {:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert_eq!(finding_keys(&w).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_busy_checkout_queues_its_cut_a_hold_bound_early_and_cuts_within_the_bound() {
    let w = World::new();
    busy_with_one_landed(&w).await;
    assert!(w.app.promote("web").await.unwrap()["promoted"] == "web");
    // Cut bound 24 h, hold bound 30 min: due however busy at 23 h 30 min.
    w.advance(24 * 60 - 31);
    assert!(!w.tick().await.lines.iter().any(|l| l["cut"] == "web"), "busy, within the bound less the lead");
    let t = while_busy(&w, async {
        w.advance(2);
        let t = w.tick().await;
        assert_eq!(heavy(&w).await.len(), 2, "the busy command and the cut, queued a hold bound early");
        w.advance(27);
        let keys = finding_keys(&w).await;
        assert!(!keys.iter().any(|k| k.starts_with("uncut:") || k.starts_with("queue:")), "{keys:?}");
        t
    })
    .await;
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "the cut waited its turn: {:?}", t.lines);
    let t = w.tick().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("cut within the bound");
    assert_eq!(cut["version"], "v0.1.0", "{cut}");
    assert_eq!(finding_keys(&w).await, Vec::<String>::new());
}

/// A factorium.toml whose slow gate holds in `dir` and counts its runs
/// there, in `dir/runs`.
fn holding_slow_gate(w: &World, dir: &Path) -> String {
    let line = format!("echo run >> {}/runs; {}", dir.display(), w.holds(dir));
    format!("[gate]\nfast = \"true\"\nslow = \"{line}\"\n")
}

/// How many times the slow gate holding in `dir` started.
fn runs(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("runs")).map_or(0, |s| s.lines().count())
}

#[tokio::test]
async fn promote_by_hand_while_the_daemons_promotion_runs_is_refused_and_leaves_it_be() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    let gate = w.dir.join("slow");
    done(&w, "factorium.toml", &holding_slow_gate(&w, &gate)).await;
    let slow_before = w.slow_gates_ran();
    let by_hand = async {
        until_file(&gate.join("started")).await;
        let refused = w.run().promote("web").await.unwrap();
        std::fs::write(gate.join("release"), "").unwrap();
        refused
    };
    let (t, refused) = tokio::time::timeout(STUCK, async { tokio::join!(w.tick(), by_hand) }).await.expect("ends");
    assert_eq!(refused["promoted"], json!(null), "{refused}");
    let why = refused["why"].as_str().unwrap();
    assert!(why.contains("a promotion of web runs: the daemon's promotion of "), "{why}");
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "the running one goes on: {:?}", t.lines);
    assert_eq!(runs(&gate), 1, "one slow gate ran");
    assert_eq!(w.slow_gates_ran() - slow_before, 1, "one GateRan of the tip");
}

#[tokio::test]
async fn the_daemon_starts_no_promotion_while_promote_by_hand_runs() {
    let w = World::new();
    // #2 keeps working all along: the daemon promotes only once the bound passed.
    crate::line::two(&w).await;
    let one = w.app.workers(false).await.unwrap().as_array().unwrap().iter().find(|x| x["number"] == 1).cloned();
    let one = one.unwrap();
    let gate = w.dir.join("slow");
    w.commit(&worktree(&one), "factorium.toml", &holding_slow_gate(&w, &gate));
    w.app.worker_done(id(&one)).await.unwrap();
    assert_eq!(w.tick().await.landed.len(), 1);
    let slow_before = w.slow_gates_ran();
    let cli = w.run();
    let by_hand = cli.promote("web");
    let ticks = async {
        until_file(&gate.join("started")).await;
        // Past the promotion bound: the daemon's promotion is due.
        w.advance(31);
        let t = w.tick().await;
        std::fs::write(gate.join("release"), "").unwrap();
        t
    };
    let (promoted, t) = tokio::time::timeout(STUCK, async { tokio::join!(by_hand, ticks) }).await.expect("ends");
    assert!(!t.lines.iter().any(|l| l["promoted"] == "web" || l["promotion"] == "web"), "{:?}", t.lines);
    assert_eq!(promoted.unwrap()["promoted"], "web");
    assert_eq!(runs(&gate), 1, "one slow gate ran");
    assert_eq!(w.slow_gates_ran() - slow_before, 1, "one GateRan of the tip");
    assert!(!w.tick().await.lines.iter().any(|l| l["promoted"] == "web"), "nothing left to promote");
}

/// The heavy entries `doctor` shows started.
async fn started(w: &World) -> Vec<String> {
    let d = w.run().doctor().await.unwrap();
    let entries = d["heavy"].as_array().unwrap().iter();
    entries.filter(|e| !e["started_at"].is_null()).map(|e| e["what"].as_str().unwrap().to_string()).collect()
}

/// Seen 2026-11-18 (factorium#102): three `worker exclusive` asked in the
/// same second beside a running entry, and all three started on readings
/// that could not yet show each other's load.
#[tokio::test]
async fn beside_what_runs_asks_in_one_second_start_one_per_settled_reading() {
    let w = World::new();
    running_on(&w).await;
    let settle = 30_000;
    // The running one started a settle interval ago; the host has room.
    w.clock.advance(settle);
    w.host.reading.lock().unwrap().cpu_busy = 10;
    let dirs: Vec<_> = (0..3).map(|k| w.dir.join(format!("ask{k}"))).collect();
    let lines: Vec<_> = dirs.iter().map(|d| w.holds(d)).collect();
    let clis: Vec<_> = (0..3).map(|_| w.run()).collect();
    let ask = |k: usize| clis[k].worker_exclusive(None, &w.dir, &lines[k]);
    let asks = async { tokio::join!(ask(0), ask(1), ask(2)) };
    let looks = async {
        // Each look waits until `n` of the three started, then for a few more
        // asks (they ask every 500ms), and answers how many started.
        let count = || dirs.iter().filter(|d| d.join("started").exists()).count();
        let after = async |n: usize| {
            while count() < n {
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(Duration::from_millis(1200)).await;
            count()
        };
        assert_eq!(after(1).await, 1, "one starts on the reading the others saw");
        w.clock.advance(settle - 1);
        assert_eq!(after(1).await, 1, "a reading short of the settle interval starts none");
        w.clock.advance(1);
        assert_eq!(after(2).await, 2, "a reading the settle interval later starts the next");
        assert_eq!(started(&w).await.len(), 3, "the running one and two of the three");
        w.clock.advance(settle);
        assert_eq!(after(3).await, 3);
        for d in &dirs {
            std::fs::write(d.join("release"), "").unwrap();
        }
    };
    let (ran, ()) = tokio::time::timeout(STUCK, async { tokio::join!(asks, looks) }).await.expect("all three run");
    for r in [ran.0, ran.1, ran.2] {
        assert_eq!(r.unwrap()["ok"], true);
    }
}

#[tokio::test]
async fn with_nothing_running_the_oldest_starts_at_once_however_recent_the_last_start() {
    let w = World::new();
    // No room: nothing running is reason enough.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    for line in ["echo one", "echo two"] {
        let ran = tokio::time::timeout(STUCK, w.run().worker_exclusive(None, &w.dir, line)).await;
        assert_eq!(ran.expect("starts at once").unwrap()["ok"], true, "{line}");
    }
}

/// Seen 2026-11-18 (factorium#106): two `worker exclusive` of binaries older
/// than the one installed started 3 ms apart beside a running entry, under
/// the rules they were built with, which lacked #102's one start per reading.
/// Refused, a waiter execs the binary Homebrew links now in its place.
#[tokio::test]
async fn a_waiter_under_rules_a_newer_binary_replaced_does_not_start() {
    let w = World::new();
    let (busy, old, new) = (w.dir.join("busy"), w.dir.join("old"), w.dir.join("new"));
    let lines = [w.holds(&busy), w.holds(&old), w.holds(&new)];
    // No room beside what runs: both wait.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let (b, o, n) = (w.run(), w.run().under_rules("1.0.0"), w.run().under_rules("1.0.1"));
    // The old one runs as Homebrew's 0.0.9; Homebrew links a newer one since.
    let link = w.running_as(&o, "factorium");
    let newer_keg = w.dir.join("brew/Cellar/factorium/0.1.0/bin");
    std::fs::create_dir_all(&newer_keg).unwrap();
    std::fs::write(newer_keg.join("factorium"), "a binary that may not be run").unwrap();
    std::fs::remove_file(w.dir.join("brew/opt/factorium")).unwrap();
    std::os::unix::fs::symlink(newer_keg.parent().unwrap(), w.dir.join("brew/opt/factorium")).unwrap();
    // As the waiter finds its link once refused: after the upgrade.
    o.runs_as(&w.dir.join("brew/Cellar/factorium/0.0.9/bin/factorium").canonicalize().unwrap());
    let runs = async {
        let held = b.worker_exclusive(None, &w.dir, &lines[0]);
        let waits = async {
            until_file(&busy.join("started")).await;
            let older = async {
                let r = o.worker_exclusive(None, &w.dir, &lines[1]).await;
                std::fs::write(w.dir.join("old-asked"), "").unwrap();
                r
            };
            let newer = async {
                // Queued after the old one, so it waits behind it.
                while heavy(&w).await.len() < 2 {
                    tokio::task::yield_now().await;
                }
                n.worker_exclusive(None, &w.dir, &lines[2]).await
            };
            tokio::join!(older, newer)
        };
        tokio::join!(held, waits)
    };
    let looks = async {
        while heavy(&w).await.len() < 3 {
            tokio::task::yield_now().await;
        }
        // The upgrade: a settled reading with room, for one start.
        w.clock.advance(30_000);
        w.host.reading.lock().unwrap().cpu_busy = 10;
        until_file(&new.join("started")).await;
        until_file(&w.dir.join("old-asked")).await;
        assert!(!old.join("started").exists(), "the old waiter never starts");
        for d in [&new, &busy] {
            std::fs::write(d.join("release"), "").unwrap();
        }
    };
    let ((held, (older, newer)), ()) =
        tokio::time::timeout(STUCK, async { tokio::join!(runs, looks) }).await.expect("the newer one runs");
    assert_eq!(held.unwrap()["ok"], true);
    assert_eq!(newer.unwrap()["ok"], true);
    let refused = older.expect_err("refused under replaced rules");
    assert_eq!(refused.code, "stale", "{refused}");
    let exec = format!("and exec {}: Permission denied", link.display());
    assert!(refused.message.contains(&exec), "it execs the binary Homebrew links now: {refused}");
    assert!(heavy(&w).await.is_empty(), "the refused waiter left the queue");
}
