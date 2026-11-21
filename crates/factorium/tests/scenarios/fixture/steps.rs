//! What a scenario does through the CLI, as one step each: a worker is
//! done, a worker lands, two workers keep a checkout busy, a picker is
//! asked.

use std::path::PathBuf;

use factorium::Tick;
use factorium_harness::{Asked, Harness};
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
        if clone.exists() {
            self.git(&clone, &["fetch", "-q", "origin"]).unwrap();
        } else {
            self.git(&self.dir, &["clone", "-q", &remote.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
        }
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

/// A mutant picker as a scenario stands one in: the program the kit names,
/// what it was asked, and with what.
pub struct Picker {
    pub program: PathBuf,
    dir: PathBuf,
}

impl Picker {
    /// What its own user directory held, one name a line.
    pub fn user_dir(&self) -> String {
        std::fs::read_to_string(self.dir.join("user_dir")).unwrap_or_default()
    }

    /// What it read on stdin, its arguments (one a line) and its environment.
    pub fn asked(&self) -> (String, String, String) {
        let read = |f: &str| std::fs::read_to_string(self.dir.join(f)).unwrap_or_default();
        (read("asked"), read("args"), read("env"))
    }
}

impl World {
    /// slimfit as the scenario has it: `goal` its `help picker`, `listing`
    /// its `check --mutants`.
    pub fn slimfit(&self, goal: &str, listing: &str) {
        let mut said = self.host.slimfit.lock().unwrap();
        said.insert("help picker".into(), goal.into());
        said.insert("check".into(), listing.into());
    }

    /// A stand-in for a session of `harness` asked headless: it keeps what it
    /// was asked, then runs `then` (shell), then prints `answered` as that
    /// harness would.
    pub fn picker(&self, harness: Harness, then: &str, answered: &Asked) -> Picker {
        let dir = self.dir.join(format!("picker-{}", harness.key()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("said"), harness.asked_stand_in(answered).unwrap()).unwrap();
        let d = dir.display();
        let script = format!(
            "#!/bin/sh\ncat > '{d}/asked'\nls -A \"$CODEX_HOME$GROK_HOME\" > '{d}/user_dir' 2>/dev/null\nprintf '%s\\n' \"$@\" > '{d}/args'\nenv > '{d}/env'\n{then}\ncat '{d}/said'\n"
        );
        let program = dir.join("program");
        super::program(&program, &script);
        Picker { program, dir }
    }
}
