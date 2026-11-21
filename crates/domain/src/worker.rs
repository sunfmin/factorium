//! A worker: one harness session working one issue in its own worktree.
//! It works until it asks to land (`done`); a red landing sends it back to
//! work, and a second red in a row, an escalation, or a failure waits for
//! the watch. A branch that does not merge because proving moved again
//! since the last red is not the second (factorium#171). A green landing, or the watch, ends it.

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
    /// The proving tip its last red did not merge onto; `None`: its last
    /// red was not that, or there was none.
    #[serde(default)]
    pub red_tip: Option<String>,
    /// Reds in a row that did not count: each did not merge onto a proving
    /// tip that moved since the one before, which its branch holds.
    #[serde(default)]
    pub moved_reds: u32,
    /// Why it waits, or why it ended.
    pub why: Option<String>,
    /// When its status last changed, in ms.
    pub since: i64,
    /// The landed commit.
    pub landed: Option<String>,
    /// Since when its processes have been over their share, in ms.
    #[serde(default)]
    pub over_since: Option<i64>,
    /// Its turn open, its processes near idle and its screen unchanged:
    /// that screen's hash, and since when, in ms.
    #[serde(default)]
    pub quiet: Option<(u64, i64)>,
    /// A heavy entry of its own is in flight.
    #[serde(default)]
    pub heavy: bool,
    /// When its last heavy entry in flight ended, in ms: its silence counts
    /// from no earlier.
    #[serde(default)]
    pub freed_at: Option<i64>,
    /// Waiting, its issue was blocked: the watch parked it behind what its
    /// issue waits for, and it goes back to work once all of that is met.
    #[serde(default)]
    pub parked: bool,
    /// The tokens its session spent, as its session files said once it
    /// ended; `None`: not recorded (its harness's files do not say, or it
    /// is live).
    #[serde(default)]
    pub spent: Option<Tokens>,
}

/// Tokens a session spent: those it was given (cached ones too) and those
/// it wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(rename = "in")]
    pub input: u64,
    #[serde(rename = "out")]
    pub output: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WorkerEvent {
    WorkerStarted(Box<Started>),
    LandingAsked {
        at: i64,
    },
    LandingRed {
        why: String,
        at: i64,
        /// The proving tip its branch did not merge onto; `None`: it merged,
        /// and the gate was red.
        #[serde(default)]
        tip: Option<String>,
        /// It does not count: proving moved since the last red.
        #[serde(default)]
        moved: bool,
    },
    /// Written before the outbox (#47), which now records what reached
    /// its session; read and ignored.
    RedTold {
        reached: bool,
        at: i64,
    },
    WorkerLanded {
        commit: String,
        at: i64,
    },
    WorkerWaits {
        why: String,
        at: i64,
    },
    WorkerResumed {
        at: i64,
    },
    WorkerEnded {
        why: String,
        at: i64,
    },
    /// Its processes went over their share.
    WentOver {
        at: i64,
    },
    /// Its processes are back within their share.
    CameUnder,
    /// Written before the outbox (#47); read and ignored.
    BriefHanded,
    /// Its turn open, its processes near idle, showing this screen.
    WentQuiet {
        screen: u64,
        at: i64,
    },
    /// Its processes or its screen moved again, or its turn closed.
    Stirred,
    /// A heavy entry of its own went in flight: waiting or running.
    HeavyHeld,
    /// Its heavy entries in flight all ended.
    HeavyFreed {
        at: i64,
    },
    /// Its processes held over their share past `hot_for`: what, and where
    /// the evidence of them is.
    WentHot {
        what: String,
        evidence: String,
    },
    /// Its account is used up, as its screen said: the account rests until
    /// then, in ms.
    UsedUp {
        said: String,
        until: i64,
    },
    /// Written before what a waiting worker heard was read from its
    /// issue's comments (#161); read and ignored.
    WorkerHeard {
        text: String,
        at: i64,
    },
    /// Waiting, its issue was blocked.
    WorkerParked,
    /// No longer parked: what its issue waited for was met, or it was
    /// sent back to work.
    WorkerUnparked,
    /// Its terminal was closed and Orca no longer lists it; `failed`: why
    /// it was not (Orca refused, or lists it still).
    SessionClosed {
        failed: Option<String>,
    },
    /// What its session spent, read from its session files as it ends.
    Spent(Tokens),
}

/// A worker's status, why, and since when, in ms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    pub status: Status,
    pub why: Option<String>,
    pub since: i64,
}

impl WorkerEvent {
    /// Where the worker stands after this event, if the event moves it: the
    /// one place a worker's status changes, for the worker and its row.
    pub fn standing(&self) -> Option<Standing> {
        let (status, why, since) = match self {
            WorkerEvent::WorkerStarted(st) => (Status::Working, None, st.at),
            WorkerEvent::LandingAsked { at } => (Status::Landing, None, *at),
            WorkerEvent::LandingRed { why, at, .. } => (Status::Working, Some(why.clone()), *at),
            WorkerEvent::WorkerLanded { at, .. } => (Status::Ended, Some("landed".into()), *at),
            WorkerEvent::WorkerWaits { why, at } => (Status::Waiting, Some(why.clone()), *at),
            WorkerEvent::WorkerResumed { at } => (Status::Working, None, *at),
            WorkerEvent::WorkerEnded { why, at } => (Status::Ended, Some(why.clone()), *at),
            _ => return None,
        };
        Some(Standing { status, why, since })
    }
}

pub struct Workers;

impl Aggregate for Workers {
    const NAME: &'static str = "worker";
    type State = Worker;
    type Event = WorkerEvent;

    fn evolve(s: &mut Worker, event: WorkerEvent) {
        if let Some(Standing { status, why, since }) = event.standing() {
            (s.status, s.why, s.since) = (status, why, since);
        }
        match event {
            WorkerEvent::WorkerStarted(started) => s.started = Some(*started),
            WorkerEvent::LandingRed { tip, moved, .. } => {
                s.red_tip = tip;
                if moved {
                    s.moved_reds += 1;
                } else {
                    (s.reds, s.moved_reds) = (s.reds + 1, 0);
                }
            }
            WorkerEvent::WorkerLanded { commit, .. } => s.landed = Some(commit),
            WorkerEvent::WorkerWaits { .. } => (s.over_since, s.quiet) = (None, None),
            WorkerEvent::WorkerResumed { .. } => {
                (s.reds, s.red_tip, s.moved_reds, s.quiet, s.parked) = (0, None, 0, None, false)
            }
            WorkerEvent::WorkerParked => s.parked = true,
            WorkerEvent::WorkerUnparked => s.parked = false,
            WorkerEvent::WentQuiet { screen, at } => s.quiet = Some((screen, at)),
            WorkerEvent::Stirred => s.quiet = None,
            WorkerEvent::WentOver { at } => s.over_since = Some(at),
            WorkerEvent::CameUnder => s.over_since = None,
            WorkerEvent::HeavyHeld => s.heavy = true,
            WorkerEvent::HeavyFreed { at } => (s.heavy, s.freed_at) = (false, Some(at)),
            WorkerEvent::Spent(tokens) => s.spent = Some(tokens),
            _ => {}
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Red {
    pub worker: Uuid,
    pub why: String,
    /// The proving tip its branch did not merge onto; `None`: the gate was red.
    pub tip: Option<String>,
    /// Proving moved since the last red's tip, which its branch holds.
    pub moved: bool,
    /// At most this many reds in a row are let off as moved (`moved_reds_max`).
    pub moved_max: u32,
}

impl Red {
    /// Whether this red does not count: its branch does not merge onto a
    /// tip newer than the last red's, which it holds, and the last red was
    /// one onto a tip too; within `moved_max` of these in a row.
    fn moved(&self, s: &Worker) -> bool {
        self.tip.is_some() && s.red_tip.is_some() && self.moved && s.moved_reds < self.moved_max
    }
}

command!(
    /// A red landing: back to work, or, the second in a row, to the watch.
    /// One that does not merge because proving moved again is not the second.
    RedLanding,
    "worker.red",
    Workers,
    Red,
    |p| p.worker,
    |s, p, now| {
        live(s)?;
        if s.status != Status::Landing {
            return no("not_landing", "the worker is not landing");
        }
        let moved = p.moved(s);
        let mut events = vec![WorkerEvent::LandingRed { why: p.why.clone(), at: now.0, tip: p.tip.clone(), moved }];
        if !moved && s.reds + 1 >= 2 {
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

command!(
    /// Its issue was blocked: a waiting worker is parked behind what its
    /// issue waits for. One that does not wait is not.
    Park,
    "worker.park",
    Workers,
    Of,
    |p| p.worker,
    |s, _p, _now| Ok::<_, Rejection>(if s.status == Status::Waiting && !s.parked {
        vec![WorkerEvent::WorkerParked]
    } else {
        Vec::new()
    })
);

command!(
    /// What a parked worker's issue waited for is met: it is no longer
    /// parked, whether or not its restart reaches it.
    Unpark,
    "worker.unpark",
    Workers,
    Of,
    |p| p.worker,
    |s, _p, _now| Ok::<_, Rejection>(if s.parked { vec![WorkerEvent::WorkerUnparked] } else { Vec::new() })
);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Closed {
    pub worker: Uuid,
    /// Why its terminal is not closed; `None`: Orca no longer lists it.
    pub failed: Option<String>,
}

command!(
    /// How closing its terminal went. Recorded before it ends: an ended
    /// worker takes no more commands.
    NoteClosed,
    "worker.closed",
    Workers,
    Closed,
    |p| p.worker,
    |s, p, _now| {
        if s.started.is_none() {
            return no("no_worker", "no such worker");
        }
        Ok(vec![WorkerEvent::SessionClosed { failed: p.failed.clone() }])
    }
);

command!(EndWorker, "worker.end", Workers, Because, |p| p.worker, |s, p, now| {
    live(s)?;
    Ok(vec![WorkerEvent::WorkerEnded { why: p.why.clone(), at: now.0 }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spend {
    pub worker: Uuid,
    pub tokens: Tokens,
}

command!(
    /// What its session spent, as its session files say. Recorded before it
    /// ends: an ended worker takes no more commands.
    NoteSpent,
    "worker.spent",
    Workers,
    Spend,
    |p| p.worker,
    |s, p, _now| {
        live(s)?;
        Ok(vec![WorkerEvent::Spent(p.tokens)])
    }
);
