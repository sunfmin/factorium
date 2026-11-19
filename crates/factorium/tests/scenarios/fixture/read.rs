//! Readers of what the CLI prints: the workers it lists, doctor's findings
//! and heavy queue, an issue's comments. A scenario reads nothing else.

use std::path::PathBuf;

use factorium::App;
use factum::Uuid;
use serde_json::Value;

use super::World;

/// The rows of a list the CLI printed.
fn rows(list: &Value) -> &[Value] {
    list.as_array().unwrap_or_else(|| panic!("a list: {list}"))
}

/// A live worker, as the CLI lists it.
pub async fn worker(app: &App) -> Value {
    rows(&app.workers(false).await.unwrap()).first().cloned().expect("a worker")
}

/// A worker's worktree, as the CLI lists it.
pub fn worktree(w: &Value) -> PathBuf {
    PathBuf::from(w["worktree"].as_str().unwrap())
}

pub fn id(w: &Value) -> Uuid {
    w["worker"].as_str().unwrap().parse().unwrap()
}

/// doctor's findings, as it printed them.
pub fn findings(doctor: &Value) -> &[Value] {
    rows(&doctor["findings"])
}

/// The keys of doctor's findings.
pub fn finding_keys(doctor: &Value) -> Vec<String> {
    findings(doctor).iter().map(|f| f["key"].as_str().unwrap().to_string()).collect()
}

/// What follows `prefix` in the keys of doctor's findings that start with it.
pub fn keyed(doctor: &Value, prefix: &str) -> Vec<String> {
    finding_keys(doctor).iter().filter_map(|k| k.strip_prefix(prefix).map(String::from)).collect()
}

/// The heavy queue's entries doctor printed, oldest first.
pub fn heavy(doctor: &Value) -> &[Value] {
    rows(&doctor["heavy"])
}

/// An issue's comments, as `issue show` (or a command that answers the
/// issue) printed them.
pub fn comments(issue: &Value) -> Vec<Value> {
    rows(&issue["comments"]).to_vec()
}

/// Whether factorium closed terminal `handle`, as the CLI prints it: the
/// worker it was opened for, if one was, shows it `closed` in `workers
/// --all`, and doctor names it neither an orphan nor unclosed.
pub async fn closed(app: &App, handle: &str) -> bool {
    let all = app.workers(true).await.unwrap();
    let row = rows(&all).iter().find(|r| r["terminal"] == handle);
    let keys = finding_keys(&app.doctor().await.unwrap());
    let named = keys.iter().any(|k| *k == format!("orphan:{handle}") || *k == format!("unclosed:{handle}"));
    row.is_none_or(|r| r["closed"] == true) && !named
}

/// The shell word that opens at `at` in `line`, unquoted: `'…'` with `'\''` inside.
pub fn quoted_word(line: &str, at: usize) -> String {
    let (mut word, mut rest) = (String::new(), &line[at + 1..]);
    loop {
        let end = rest.find('\'').unwrap();
        word.push_str(&rest[..end]);
        rest = &rest[end + 1..];
        let Some(more) = rest.strip_prefix("\\''") else { return word };
        word.push('\'');
        rest = more;
    }
}

impl World {
    /// The live workers, as `workers` lists them.
    pub async fn workers(&self) -> Vec<Value> {
        rows(&self.app.workers(false).await.unwrap()).to_vec()
    }

    /// The live worker on `checkout`#`number`.
    pub async fn worker_on(&self, checkout: &str, number: u64) -> Value {
        let all = self.workers().await;
        let found = all.iter().find(|r| r["checkout"] == checkout && r["number"] == number).cloned();
        found.unwrap_or_else(|| panic!("no worker on {checkout}#{number}: {all:?}"))
    }

    /// What doctor prints, run as another CLI run.
    pub async fn doctor(&self) -> Value {
        self.run().doctor().await.unwrap()
    }

    /// doctor's findings.
    pub async fn findings(&self) -> Vec<Value> {
        findings(&self.doctor().await).to_vec()
    }

    /// The finding doctor answers under `key`, if any.
    pub async fn finding(&self, key: &str) -> Option<Value> {
        self.findings().await.into_iter().find(|f| f["key"] == key)
    }

    /// The keys of doctor's findings.
    pub async fn finding_keys(&self) -> Vec<String> {
        finding_keys(&self.doctor().await)
    }

    /// The heavy queue's entries, as doctor shows them.
    pub async fn heavy(&self) -> Vec<Value> {
        heavy(&self.doctor().await).to_vec()
    }

    /// The heavy commands queued, waiting or running, as `what`.
    pub async fn queued(&self) -> Vec<String> {
        self.heavy().await.iter().map(|e| e["what"].as_str().unwrap().to_string()).collect()
    }

    /// The heavy command `what` doctor shows queued.
    pub async fn queued_entry(&self, what: &str) -> Value {
        let all = self.heavy().await;
        all.iter().find(|e| e["what"] == what).cloned().unwrap_or_else(|| panic!("{what} is not queued: {all:?}"))
    }

    /// The comments on `checkout`#`number`, as `issue show` prints them.
    pub async fn comments(&self, checkout: &str, number: u64) -> Vec<Value> {
        comments(&self.app.issue_show(checkout, number).await.unwrap())
    }
}
