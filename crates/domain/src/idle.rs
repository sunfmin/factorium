//! Idle work: what a checkout declares under `[idle]` to run only while the
//! host is idle, at most once per `every` — a check too slow for any gate.
//! It runs as a heavy command on the base's tip, never ahead of anything
//! else the queue holds, and yields to whatever queues behind it. Each run
//! that ends is recorded; a red one is a finding, as a red promotion is.
//!
//! The tasks a run finds are filed as the checkout's issues (§1.16): what
//! `[idle] tasks` prints is one or more answers shaped as slimfit's `next
//! --all`, and of each kind the first task is filed, while no issue of that
//! kind is open. A task whose issue closed by landing is held back only
//! while that landing is not in the tip the run read.

use factum_core::{Aggregate, Uuid};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::heavy::Queue;
use crate::issue::{Issue, Status as IssueStatus};
use crate::worker::Status;
use crate::{command, id};

/// The id of the stream of `checkout`'s idle work.
pub fn idle_id(checkout: &str) -> Uuid {
    id(&format!("idle:{checkout}"))
}

/// What the queue calls idle work: this, then its checkout.
const JOB: &str = "idle work of ";

/// What the queue calls the idle work of `checkout`.
pub fn job(checkout: &str) -> String {
    format!("{JOB}{checkout}")
}

/// The idle work the queue holds, waiting or running, if any: the host runs
/// one at a time, however it was started.
pub fn queued(queue: &Queue) -> Option<&crate::heavy::Entry> {
    queue.entries.iter().find(|e| e.what.starts_with(JOB))
}

/// One run that ended: when it started, on which base tip, and how it went.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub tip: String,
    pub at: i64,
    pub green: bool,
    pub took_ms: u64,
    /// The last lines it printed, and why it ended.
    pub tail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Idle {
    /// The last run that ended green or red; one that yielded is not one.
    pub last: Option<Run>,
}

impl Idle {
    /// Whether a run is due at `now`, `every` ms after the last one started.
    pub fn due(&self, every_ms: i64, now: i64) -> bool {
        self.last.as_ref().is_none_or(|r| now - r.at >= every_ms)
    }

    /// The last run, if it was red.
    pub fn red(&self) -> Option<&Run> {
        self.last.as_ref().filter(|r| !r.green)
    }
}

/// Whether the host is idle, or why not: no live worker works (each waits
/// for the watch), the `host` queue holds nothing, and the host has room.
pub fn host_idle(workers: &[Status], queue: &Queue, room: Result<(), String>) -> Result<(), String> {
    if workers.iter().any(|s| *s != Status::Waiting && *s != Status::Ended) {
        return Err("a worker works".into());
    }
    // A named resource held or waited for (a device) loads nothing of the host's.
    if let Some(e) = queue.entries.iter().find(|e| e.asks_for(crate::heavy::HOST)) {
        return Err(format!("the heavy queue holds `{}`", e.what));
    }
    room
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IdleEvent {
    IdleRan { run: Run },
}

pub struct Idles;

impl Aggregate for Idles {
    const NAME: &'static str = "idle";
    type State = Idle;
    type Event = IdleEvent;

    fn evolve(s: &mut Idle, IdleEvent::IdleRan { run }: IdleEvent) {
        s.last = Some(run);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ran {
    pub checkout: String,
    pub run: Run,
}

command!(RecordIdle, "idle.ran", Idles, Ran, |p| idle_id(&p.checkout), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![IdleEvent::IdleRan { run: p.run.clone() }])
});

/// A task as it is told apart: its kind and the files it works on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub kind: String,
    pub files: Vec<String>,
}

/// An issue to file for a task.
#[derive(Clone, Debug, PartialEq)]
pub struct ToFile {
    pub title: String,
    pub body: String,
    pub task: Task,
}

/// The issues to file from what the tasks line `said` at `tip`: the lines
/// before its JSON name what read the tasks (`slimfit --version`), then come
/// answers listing `tasks`: `slimfit next --all`'s, and any other's that
/// names itself in `by` (`./lines.sh --tasks`, SPEC §5; `truefit next
/// --all`), each maybe after lines of its own (`truefit --version`).
/// `issues` are the checkout's, open and closed; `in_tip` tells whether a
/// commit is an ancestor of `tip`.
pub fn to_file(said: &str, tip: &str, issues: &[Issue], in_tip: impl Fn(&str) -> bool) -> Result<Vec<ToFile>, String> {
    let at = said.find("\n{").map_or(0, |i| i + 1);
    let (by, json) = said.split_at(at);
    let answers = answers(json);
    let mut tasks = Vec::new();
    for a in &answers {
        let listed = a["tasks"].as_array().ok_or("the tasks line printed an answer listing no `tasks`")?;
        tasks.extend(listed.iter().map(|t| (a["by"].as_str(), t)));
    }
    if answers.is_empty() {
        return Err("the tasks line printed no answer listing `tasks`".into());
    }
    let by = by.trim();
    let open = |kind: &str| {
        issues.iter().any(|i| i.status != IssueStatus::Closed && i.task.as_ref().is_some_and(|t| t.kind == kind))
    };
    // Closed by a landing not yet in the tip read: its fix is on its way to
    // the base. Closed without one, it holds nothing back (a tool's keep does).
    let landing = |task: &Task| {
        issues.iter().any(|i| i.task.as_ref() == Some(task) && i.landed_as().is_some_and(|sha| !in_tip(sha)))
    };
    let mut filed: Vec<ToFile> = Vec::new();
    for (own, t) in tasks {
        let kind = t["kind"].as_str().unwrap_or_default().to_string();
        let files = t["files"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string);
        let task = Task { kind, files: files.collect() };
        if task.kind.is_empty() || open(&task.kind) || filed.iter().any(|f| f.task.kind == task.kind) || landing(&task)
        {
            continue;
        }
        let shown = serde_json::to_string_pretty(t).unwrap_or_default();
        let summary = t["summary"].as_str().unwrap_or_default();
        let (from, rules) = match own {
            Some(own) => (own, ""),
            None => ("slimfit", " slimfit's rules: `slimfit help`."),
        };
        let body = format!(
            "The next `{kind}` task, as `{read}` read it at {tip}:\n\n```json\n{shown}\n```\n\n\
             Do this one task, by its `steps`; accept it as its `accept` says, then land. \
             Your judgment is accepted (slimfit#44): nothing is checked beyond the task's own accept.{rules}\n\n\
             Filed by idle work (SPEC §1.16): one open issue per kind; once this closes, the next idle run files the next.",
            kind = task.kind,
            read = own.unwrap_or(by),
        );
        filed.push(ToFile { title: format!("{from} {}: {summary}", task.kind), body, task });
    }
    Ok(filed)
}

/// The JSON answers in `json`, past any line between them that is no JSON:
/// each answer starts a line with `{`.
fn answers(json: &str) -> Vec<Value> {
    let (mut answers, mut rest) = (Vec::new(), Some(json));
    while let Some(json) = rest {
        let mut read = serde_json::Deserializer::from_str(json).into_iter::<Value>();
        answers.extend(read.by_ref().map_while(Result::ok));
        let at = read.byte_offset();
        rest = json[at..].find("\n{").map(|i| &json[at + i + 1..]);
    }
    answers
}
