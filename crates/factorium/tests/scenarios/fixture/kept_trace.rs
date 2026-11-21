//! An issue's worktrees, as a trace of their mode (models/Worktrees.tla;
//! `models/worktrees.sh trace` checks one with `truefit model worktrees-run
//! --trace`): after each step a scenario takes (a whole tick, command or
//! worker's step), issue `web`#1 as the database holds it, and each tree's
//! worktree, branch and kept branch, and the proving tip, as git holds
//! them. Tree t is the worktree attempt t made (`web-1-<t>`); work is the
//! files `w<n>.txt` a scenario's worker writes, token n.

use std::path::Path;

use factorium_domain::worker_id;
use serde_json::{Value, json};

use super::World;

/// The environment variable naming where a scenario keeps its run's trace.
const TRACE: &str = "FACTORIUM_KEPT_TRACE";
/// The model's attempts, and so its trees.
const ATTEMPTS: u32 = 3;

/// A run's records so far.
#[derive(Default)]
pub struct KeptTrace {
    lines: Vec<String>,
    made: u64,
}

impl KeptTrace {
    /// Records the state after `event`.
    pub async fn step(&mut self, w: &World, event: &str) {
        let state = self.state(w).await;
        let step = self.lines.len() + 1;
        self.lines.push(json!({ "step": step, "event": event, "state": state }).to_string());
    }

    /// Writes the run's trace where [`TRACE`] names, if it does.
    pub fn keep(&self) {
        if let Ok(path) = std::env::var(TRACE) {
            std::fs::write(path, self.lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        }
    }

    async fn state(&mut self, w: &World) -> Value {
        let issue = w.app.issue_show("web", 1).await.unwrap();
        let attempt_of =
            |id: &Value| (1..=ATTEMPTS).find(|a| json!(worker_id("web", 1, *a)) == *id).map_or(0, |a| a as u64);
        let status = match issue["status"].as_str().unwrap() {
            "waiting_for_human" => "human",
            s => s,
        };
        let workers = w.app.workers(true).await.unwrap();
        let row = |a: u32| workers.as_array().unwrap().iter().find(|r| r["attempt"] == json!(a)).cloned();
        let tree_of = |r: &Value| {
            let name = Path::new(r["worktree"].as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned();
            name.rsplit('-').next().unwrap().parse::<u64>().unwrap()
        };
        let attempts: i64 =
            w.sql().query_row("SELECT COUNT(*) FROM events WHERE kind = 'IssueTaken'", [], |r| r.get(0)).unwrap();
        let (repo, remotes) = (w.dir.join("repos").join("web"), w.dir.join("remotes"));
        let remote = [remotes.join("web.git"), remotes.join("web.git.away")].into_iter().find(|p| p.exists()).unwrap();
        let (mut disk, mut dirty, mut hasbr, mut br, mut kept) = (vec![], vec![], vec![], vec![], vec![]);
        for t in 1..=ATTEMPTS {
            let tree = w.dir.join("worktrees").join(format!("web-1-{t}"));
            let branch = format!("factorium/web-1-{t}");
            disk.push(tree.exists());
            let status = if tree.exists() {
                w.git(&tree, &["status", "--porcelain", "--untracked-files=all"]).unwrap()
            } else {
                String::new()
            };
            dirty.push(tokens(status.lines().filter_map(|l| l.get(3..))));
            let has = w.git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok();
            hasbr.push(has);
            br.push(files(w, &repo, &branch));
            kept.push(files(w, &repo, &format!("{branch}-abandoned")));
        }
        let proving = files(w, &remote, "proving");
        let seen = [&dirty, &br, &kept].into_iter().flatten().chain([&proving]).flatten().copied().max();
        self.made = self.made.max(seen.unwrap_or(0));
        json!({
            "status": status,
            "taken": attempt_of(&issue["worker"]),
            "attempts": attempts,
            "started": (1..=ATTEMPTS).map(|a| row(a).map_or(0, |r| tree_of(&r))).collect::<Vec<_>>(),
            "live": (1..=ATTEMPTS)
                .map(|a| row(a).is_some_and(|r| r["status"] == "working" || r["status"] == "landing"))
                .collect::<Vec<_>>(),
            "disk": disk,
            "dirty": dirty,
            "hasbr": hasbr,
            "br": br,
            "kept": kept,
            "proving": proving,
            "tipok": remotes.join("web.git").exists(),
            "made": (1..=self.made).collect::<Vec<_>>(),
        })
    }
}

/// The tokens `w<n>.txt` among `names`, in order.
fn tokens<'a>(names: impl Iterator<Item = &'a str>) -> Vec<u64> {
    let mut found: Vec<u64> =
        names.filter_map(|n| n.trim().strip_prefix('w')?.strip_suffix(".txt")?.parse().ok()).collect();
    found.sort();
    found
}

/// The tokens the tree of `rev` in the repo at `dir` holds; none if no such rev.
fn files(w: &World, dir: &Path, rev: &str) -> Vec<u64> {
    let listed = w.git(dir, &["ls-tree", "-r", "--name-only", rev]).unwrap_or_default();
    tokens(listed.lines())
}

/// Writes the run's trace where [`TRACE`] names, if it does: as the run
/// ends, passed or failed, so a failed one keeps the step it failed at.
impl Drop for KeptTrace {
    fn drop(&mut self) {
        if let Ok(path) = std::env::var(TRACE) {
            let _ = std::fs::write(path, self.lines.iter().map(|l| format!("{l}\n")).collect::<String>());
        }
    }
}
