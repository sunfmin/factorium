//! One path through the heavy-command queue: every heavy command, gate runs
//! and cuts included, is held to the hold bound, and the daemon's tick asks
//! the queue once and goes on rather than waiting on it.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::{World, id, worker};

/// Long enough that only a broken rule gets there.
const STUCK: Duration = Duration::from_secs(20);

async fn until_file(path: &Path) {
    while !path.exists() {
        tokio::task::yield_now().await;
    }
}

/// The heavy commands `doctor` shows, as `what`.
async fn heavy(w: &World) -> Vec<String> {
    let d = w.run().doctor().await.unwrap();
    d["heavy"].as_array().unwrap().iter().map(|e| e["what"].as_str().unwrap().to_string()).collect()
}

/// Runs `during` while another CLI run's heavy command holds the queue, then
/// lets that command end.
async fn while_busy<T>(w: &World, during: impl Future<Output = T>) -> T {
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
    assert!(red.contains("held others up past the hold bound"), "{red}");
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
