//! The heavy-command queue as landings meet it: what runs holds the rest
//! while the host has no room; the oldest waiting starts first, one per
//! reading.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::{World, commit, id};

/// A heavy command (`factorium worker exclusive`) of another CLI run, held
/// running until released.
struct Blocker {
    run: Option<tokio::task::JoinHandle<Value>>,
    go: PathBuf,
}

impl Blocker {
    async fn start(w: &World) -> Blocker {
        let go = w.dir.join("go");
        assert!(Command::new("mkfifo").arg(&go).status().unwrap().success());
        let (app, dir) = (w.run(), w.dir.clone());
        let run = tokio::spawn(async move {
            app.worker_exclusive(None, &dir, "touch running; perl -e 'alarm 20; exec @ARGV' cat go").await.unwrap()
        });
        while !w.dir.join("running").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Blocker { run: Some(run), go }
    }

    async fn release(mut self) -> Value {
        release(&self.go);
        self.run.take().unwrap().await.unwrap()
    }
}

/// A scenario that fails lets its command end too.
impl Drop for Blocker {
    fn drop(&mut self) {
        release(&self.go);
    }
}

/// A worker of `checkout` asking to land one commit.
async fn landing(w: &World, checkout: &str) {
    let all = w.app.workers(false).await.unwrap();
    let wk = all.as_array().unwrap().iter().find(|x| x["checkout"] == checkout).cloned().unwrap();
    commit(Path::new(wk["worktree"].as_str().unwrap()), "f", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
}

/// What a tick landed, as `checkout#number`.
async fn landed(w: &World) -> Vec<String> {
    w.tick().await.landed.iter().map(|l| l["landed"].as_str().unwrap_or_default().to_string()).collect()
}

async fn two_checkouts(w: &World) {
    w.checkout("api").await;
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "w", "", false).await.unwrap();
    w.app.issue_add("api", "a", "", false).await.unwrap();
    w.tick().await;
    w.tick().await;
}

#[tokio::test]
async fn a_landing_waits_behind_what_runs_and_the_oldest_waiting_starts_first_one_per_reading() {
    let w = World::new();
    two_checkouts(&w).await;
    let blocker = Blocker::start(&w).await;

    landing(&w, "web").await;
    w.host.reading.lock().unwrap().cpu_busy = 90;
    assert!(landed(&w).await.is_empty(), "no room, and something runs");

    // api is asked for first in a tick (by name), but web waited longer.
    w.advance(1);
    landing(&w, "api").await;
    assert!(landed(&w).await.is_empty());
    w.host.reading.lock().unwrap().cpu_busy = 10;
    w.advance(1);
    assert_eq!(landed(&w).await, ["web#1"], "room, a new reading: the oldest starts");
    assert!(landed(&w).await.is_empty(), "the same reading starts no second");
    w.advance(1);
    assert_eq!(landed(&w).await, ["api#1"]);

    let ran = blocker.release().await;
    assert_eq!(ran["code"], 0, "{ran}");
}

/// Lets whatever reads the fifo `go` end; never waits for a reader.
fn release(go: &Path) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    const O_NONBLOCK: i32 = 0x0004;
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).custom_flags(O_NONBLOCK).open(go) {
        let _ = f.write_all(b"");
    }
}
