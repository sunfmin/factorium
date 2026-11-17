//! One path through the heavy-command queue: every heavy command, gate runs
//! and cuts included, is held to the hold bound, and the daemon's tick asks
//! the queue once and goes on rather than waiting on it.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::{World, id, worker};

/// Long enough that only a broken rule gets there.
pub(crate) const STUCK: Duration = Duration::from_secs(20);

async fn until_file(path: &Path) {
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
    std::fs::create_dir_all(&dir).unwrap();
    let cli = w.run();
    // No room for a second heavy command beside it.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    // It ends by itself after about 20s, so a scenario that fails inside does not hang.
    let line = "touch started; for i in $(seq 1000); do [ -e release ] && exit 0; sleep 0.02; done; exit 1";
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
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), file, text);
    w.app.worker_done(id(&wk)).await.unwrap();
}

#[tokio::test]
async fn a_gate_run_holding_others_up_past_the_hold_bound_is_ended() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
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
    w.checkout("web").await;
    w.kit().await;
    // #2 keeps working all along: the checkout is never idle.
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let one = w.app.workers(false).await.unwrap().as_array().unwrap().iter().find(|x| x["number"] == 1).cloned();
    let one = one.unwrap();
    w.commit(&PathBuf::from(one["worktree"].as_str().unwrap()), "a", "x");
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
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
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

#[tokio::test]
async fn a_landing_waiting_out_a_long_heavy_command_keeps_one_entry_and_starts_next_tick() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
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
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let (started, release) = (w.dir.join("gate-started"), w.dir.join("gate-release"));
    let toml = format!(
        "[gate]\nfast = \"touch {}; while [ ! -e {} ]; do sleep 0.02; done\"\n",
        started.display(),
        release.display()
    );
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
    w.checkout("web").await;
    w.kit().await;
    // #2 keeps working all along: the checkout is never idle, so no tick promotes.
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let one = w.app.workers(false).await.unwrap().as_array().unwrap().iter().find(|x| x["number"] == 1).cloned();
    let one = one.unwrap();
    w.commit(&PathBuf::from(one["worktree"].as_str().unwrap()), "a", "x");
    w.app.worker_done(id(&one)).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.landed.len(), 1);
    assert_eq!(t.lines, Vec::<serde_json::Value>::new(), "landed, not promoted");

    let dir = w.dir.join("busy");
    std::fs::create_dir_all(&dir).unwrap();
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let line = "touch started; for i in $(seq 1000); do [ -e release ] && exit 0; sleep 0.02; done; exit 1";
    let cli = w.run();
    let busy = cli.worker_exclusive(None, &dir, line);
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
