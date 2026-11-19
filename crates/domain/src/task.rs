//! Tasks a checkout's idle work finds, filed as its issues (§1.16): what
//! `[idle] tasks` prints is slimfit's `next --all` answer, and of each kind
//! the first task is filed, while no issue of that kind is open. A task
//! closed in the last week is not filed again.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::issue::{Issue, Status};

/// A task as it is told apart: its kind and the files it works on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub kind: String,
    pub files: Vec<String>,
}

/// How long a closed task is not filed again, in ms.
pub const AGAIN_MS: i64 = 7 * 24 * 3600 * 1000;

/// An issue to file for a task.
#[derive(Clone, Debug, PartialEq)]
pub struct ToFile {
    pub title: String,
    pub body: String,
    pub task: Task,
}

/// The issues to file from what the tasks line `said` at `tip`: the lines
/// before its JSON name what read the tasks (`slimfit --version`), the JSON
/// is `slimfit next --all`'s answer. `issues` are the checkout's, open and
/// closed; `now` is in ms.
pub fn to_file(said: &str, tip: &str, issues: &[Issue], now: i64) -> Result<Vec<ToFile>, String> {
    let at = said.find("\n{").map_or(0, |i| i + 1);
    let (by, json) = said.split_at(at);
    let answer = serde_json::from_str::<Value>(json).unwrap_or_default();
    let tasks = answer["tasks"].as_array().ok_or("the tasks line printed no answer listing `tasks`")?;
    let by = by.trim();
    let open = |kind: &str| {
        issues.iter().any(|i| i.status != Status::Closed && i.task.as_ref().is_some_and(|t| t.kind == kind))
    };
    let lately = |task: &Task| {
        issues.iter().any(|i| i.task.as_ref() == Some(task) && i.closed_at.is_some_and(|at| now - at < AGAIN_MS))
    };
    let mut filed: Vec<ToFile> = Vec::new();
    for t in tasks {
        let kind = t["kind"].as_str().unwrap_or_default().to_string();
        let files = t["files"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string);
        let task = Task { kind, files: files.collect() };
        if task.kind.is_empty() || open(&task.kind) || filed.iter().any(|f| f.task.kind == task.kind) || lately(&task) {
            continue;
        }
        let shown = serde_json::to_string_pretty(t).unwrap_or_default();
        let summary = t["summary"].as_str().unwrap_or_default();
        let body = format!(
            "The next `{kind}` task, as `{by}` read it at {tip}:\n\n```json\n{shown}\n```\n\n\
             Do this one task, by its `steps`; accept it as its `accept` says, then land. \
             Your judgment is accepted (slimfit#44): nothing is checked beyond the task's own accept. \
             slimfit's rules: `slimfit help`.\n\n\
             Filed by idle work (SPEC §1.16): one open issue per kind; once this closes, the next idle run files the next.",
            kind = task.kind,
        );
        filed.push(ToFile { title: format!("slimfit {}: {summary}", task.kind), body, task });
    }
    Ok(filed)
}
