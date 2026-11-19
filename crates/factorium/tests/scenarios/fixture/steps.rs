//! What a scenario does through the CLI, as one step each: a worker is
//! done, a worker lands, two workers keep a checkout busy.

use std::path::PathBuf;

use factorium::Tick;
use serde_json::Value;

use super::{World, id, worktree};

/// What a landing step answers: the tick that landed it, and its commit.
pub struct Landed {
    pub tick: Tick,
    pub commit: String,
}

impl World {
    /// Worker `wk` commits `file` (`text`) in its worktree and is done:
    /// it asks to land.
    pub async fn done(&self, wk: &Value, file: &str, text: &str) {
        self.commit(&worktree(wk), file, text);
        self.app.worker_done(id(wk)).await.unwrap();
    }

    /// The worker on `checkout`#`number` commits `file` and is done, and
    /// the next tick lands it.
    pub async fn lands(&self, checkout: &str, number: u64, file: &str) -> Landed {
        self.done(&self.worker_on(checkout, number).await, file, "x").await;
        let tick = self.tick().await;
        let landed = tick.landed.iter().find(|l| l["landed"] == format!("{checkout}#{number}"));
        let commit = landed.unwrap_or_else(|| panic!("{checkout}#{number} landed: {tick:?}"))["commit"].as_str();
        Landed { commit: commit.unwrap().into(), tick }
    }

    /// Someone commits `file` (`text`) on `branch` of repo `name` and pushes
    /// it, by hand, from a clone of their own; answers that clone.
    pub fn push_by_hand(&self, name: &str, branch: &str, file: &str, text: &str) -> PathBuf {
        let (remote, clone) = (self.dir.join("remotes").join(format!("{name}.git")), self.dir.join("by-hand"));
        self.git(&self.dir, &["clone", "-q", &remote.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
        self.git(&clone, &["checkout", "-q", "-B", branch, &format!("origin/{branch}")]).unwrap();
        self.commit(&clone, file, text);
        self.git(&clone, &["push", "-q", "origin", branch]).unwrap();
        clone
    }

    /// Workers on #1 and #2 of `web`, neither ever silent: the checkout is
    /// never idle.
    pub async fn two_at_work(&self) {
        self.checkout("web").await;
        self.kit().await;
        self.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
        self.app.issue_add("web", "a", "", false, &[]).await.unwrap();
        self.app.issue_add("web", "b", "", false, &[]).await.unwrap();
        self.tick().await;
        self.tick().await;
    }
}

/// This process's nice value: what a command run at its priority has.
pub fn own_nice() -> i32 {
    let out = std::process::Command::new("ps").args(["-o", "ni=", "-p", &std::process::id().to_string()]).output();
    String::from_utf8(out.unwrap().stdout).unwrap().trim().parse().unwrap()
}

/// A shell test that a command's process, and one it starts, has a nice
/// value `op` (`-eq`, `-gt`) `n`.
pub fn nice_is(op: &str, n: i32) -> String {
    format!("test $(sh -c 'ps -o ni= -p $$') {op} {n}")
}
