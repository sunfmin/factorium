//! A worker: one harness session working one issue in its own worktree.
//! It works until it asks to land (`done`); a red landing sends it back to
//! work, and a second red in a row, an escalation, or a failure waits for
//! the watch. A green landing, or the watch, ends it.

use factum_core::{Aggregate, Rejection, Uuid};
use serde::{Deserialize, Serialize};

use crate::{command, no};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Working,
    /// Asked to land; the lander has not answered.
    Landing,
    /// Waits for the watch: escalated, failed, or red twice.
    Waiting,
    Ended,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Landing => "landing",
            Status::Waiting => "waiting",
            Status::Ended => "ended",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Started {
    pub checkout: String,
    pub number: u64,
    pub attempt: u32,
    pub harness: String,
    pub account: Option<String>,
    pub model: Option<String>,
    pub worktree: String,
    pub branch: String,
    pub terminal: String,
    pub brief: String,
    /// The proving tip it was branched from.
    pub base: String,
    /// When it started, in ms.
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Worker {
    pub started: Option<Started>,
    pub status: Status,
    /// Red landings since its last green or the watch's word.
    pub reds: u32,
    /// Why it waits, or why it ended.
    pub why: Option<String>,
    /// When its status last changed, in ms.
    pub since: i64,
    /// The landed commit.
    pub landed: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WorkerEvent {
    WorkerStarted(Box<Started>),
    LandingAsked { at: i64 },
    LandingRed { why: String, at: i64 },
    WorkerLanded { commit: String, at: i64 },
    WorkerWaits { why: String, at: i64 },
    WorkerResumed { at: i64 },
    WorkerEnded { why: String, at: i64 },
}

pub struct Workers;

impl Aggregate for Workers {
    const NAME: &'static str = "worker";
    type State = Worker;
    type Event = WorkerEvent;

    fn evolve(s: &mut Worker, event: WorkerEvent) {
        match event {
            WorkerEvent::WorkerStarted(started) => {
                s.since = started.at;
                s.started = Some(*started);
            }
            WorkerEvent::LandingAsked { at } => (s.status, s.since) = (Status::Landing, at),
            WorkerEvent::LandingRed { why, at } => {
                s.reds += 1;
                (s.status, s.why, s.since) = (Status::Working, Some(why), at);
            }
            WorkerEvent::WorkerLanded { commit, at } => {
                (s.status, s.since, s.landed, s.why) = (Status::Ended, at, Some(commit), Some("landed".into()))
            }
            WorkerEvent::WorkerWaits { why, at } => (s.status, s.why, s.since) = (Status::Waiting, Some(why), at),
            WorkerEvent::WorkerResumed { at } => (s.status, s.reds, s.why, s.since) = (Status::Working, 0, None, at),
            WorkerEvent::WorkerEnded { why, at } => (s.status, s.why, s.since) = (Status::Ended, Some(why), at),
        }
    }

    fn ending(kind: &str) -> bool {
        matches!(kind, "WorkerLanded" | "WorkerEnded")
    }
}

/// Which worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Of {
    pub worker: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    pub worker: Uuid,
    pub started: Started,
}

command!(StartWorker, "worker.start", Workers, Start, |p| p.worker, |s, p, now| {
    if s.started.is_some() {
        return no("started", "this worker started already");
    }
    let mut started = p.started.clone();
    started.at = now.0;
    Ok(vec![WorkerEvent::WorkerStarted(Box::new(started))])
});

fn live(s: &Worker) -> Result<(), Rejection> {
    match (&s.started, s.status) {
        (None, _) => no("no_worker", "no such worker"),
        (_, Status::Ended) => no("ended", "the worker has ended"),
        _ => Ok(()),
    }
}

command!(AskLanding, "worker.done", Workers, Of, |p| p.worker, |s, p, now| {
    live(s)?;
    match s.status {
        Status::Working => Ok(vec![WorkerEvent::LandingAsked { at: now.0 }]),
        Status::Landing => Ok(Vec::new()),
        _ => no("waiting", "the worker waits for the watch"),
    }
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Because {
    pub worker: Uuid,
    pub why: String,
}

command!(
    /// A red landing: back to work, or, the second in a row, to the watch.
    RedLanding,
    "worker.red",
    Workers,
    Because,
    |p| p.worker,
    |s, p, now| {
        live(s)?;
        if s.status != Status::Landing {
            return no("not_landing", "the worker is not landing");
        }
        let mut events = vec![WorkerEvent::LandingRed { why: p.why.clone(), at: now.0 }];
        if s.reds + 1 >= 2 {
            events.push(WorkerEvent::WorkerWaits { why: format!("red twice in a row: {}", p.why), at: now.0 });
        }
        Ok(events)
    }
);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Land {
    pub worker: Uuid,
    pub commit: String,
}

command!(GreenLanding, "worker.landed", Workers, Land, |p| p.worker, |s, p, now| {
    live(s)?;
    if s.status != Status::Landing {
        return no("not_landing", "the worker is not landing");
    }
    Ok(vec![WorkerEvent::WorkerLanded { commit: p.commit.clone(), at: now.0 }])
});

command!(
    /// The worker waits for the watch: it escalated, or it failed.
    Wait,
    "worker.wait",
    Workers,
    Because,
    |p| p.worker,
    |s, p, now| {
        live(s)?;
        if s.status == Status::Waiting {
            return Ok(Vec::new());
        }
        Ok(vec![WorkerEvent::WorkerWaits { why: p.why.clone(), at: now.0 }])
    }
);

command!(
    /// The watch sends a waiting worker back to work.
    Resume,
    "worker.resume",
    Workers,
    Of,
    |p| p.worker,
    |s, p, now| {
        live(s)?;
        Ok(if s.status == Status::Waiting { vec![WorkerEvent::WorkerResumed { at: now.0 }] } else { Vec::new() })
    }
);

command!(EndWorker, "worker.end", Workers, Because, |p| p.worker, |s, p, now| {
    live(s)?;
    Ok(vec![WorkerEvent::WorkerEnded { why: p.why.clone(), at: now.0 }])
});
