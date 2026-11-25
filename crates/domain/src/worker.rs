//! A worker: one harness session working one issue in its own worktree.
//! It works until it asks to land (`done`), or, its work changing no code,
//! to finish without a landing (`done --no-landing`); a red landing sends it back to
//! work, and a second red in a row, an escalation, or a failure waits for
//! the watch. A branch that does not merge because proving moved again
//! since the last red is not the second (factorium#171). A green landing, or the watch, ends it.

use factum_core::{Aggregate, Rejection, Uuid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{command, no, only};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Working,
    /// Asked to land; the lander has not answered.
    Landing,
    /// Asked to finish without a landing, its work changing no code; the
    /// daemon has not closed its issue yet. `why` holds its evidence.
    Finishing,
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
    /// The role it started with; `None`: none (factorium#250).
    #[serde(default)]
    pub role: Option<StartedRole>,
}

/// The role an attempt started with, as it was at its start: a role
/// directory is linked live, so what it holds may change under the worker.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StartedRole {
    /// The role's directory.
    pub dir: String,
    /// Whose it is: `"issue"` or `"checkout"` (the checkout's default).
    pub from: String,
    /// What the role gave it, read through the link at the start.
    #[serde(flatten)]
    pub read: RoleRead,
}

/// What a role's directory holds, as read once.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoleRead {
    /// A hash of its rules and every file under its skills.
    pub hash: String,
    /// Each of those files by its path under the role's directory
    /// (`CLAUDE.md`, `.claude/skills/…`), and a hash of its bytes; empty in
    /// an attempt started before factorium#273.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

impl RoleRead {
    /// The files whose bytes differ from `before`'s, or that only one of
    /// the two holds, in path order.
    pub fn changed_from(&self, before: &BTreeMap<String, String>) -> Vec<String> {
        let mut paths: Vec<&String> = self.files.keys().chain(before.keys()).collect();
        paths.sort();
        paths.dedup();
        paths.into_iter().filter(|p| self.files.get(*p) != before.get(*p)).cloned().collect()
    }
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
    /// Something holds its silence clock: a heavy entry of its own in
    /// flight, or its issue blocked.
    #[serde(default)]
    pub heavy: bool,
    /// When what last held its silence clock let go, in ms: its silence
    /// counts from no earlier.
    #[serde(default)]
    pub freed_at: Option<i64>,
    /// Its issue was blocked: the watch parked it behind what its issue
    /// waits for, and once it is idle (waiting, or in a wait it declared)
    /// it goes back to work as soon as all of that is met; or, waiting, its
    /// engine is at its limit, and it goes back at the reset.
    #[serde(default)]
    pub parked: bool,
    /// The tokens its session spent, as its session files said once it
    /// ended; `None`: not recorded (its harness's files do not say, or it
    /// is live).
    #[serde(default)]
    pub spent: Option<Tokens>,
    /// The wait on a long command of its own it declared, while its status
    /// stays as it was then: no silence is found in it before its bound.
    #[serde(default)]
    pub declared: Option<Declared>,
    /// The last limit its engine was seen at: kept past its reset, so the
    /// same words read again are not a new one.
    #[serde(default)]
    pub limit: Option<Limit>,
    /// Its role as it was last told it changed; `None`: as it started
    /// (factorium#273).
    #[serde(default)]
    pub role_told: Option<RoleRead>,
}

/// A limit its engine reached, as its screen said, until when, and when it
/// was seen (ms).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub said: String,
    pub until: i64,
    pub at: i64,
}

impl Worker {
    /// What its role held as it last knew it: as it was last told it
    /// changed, else as it started; `None`: it has no role.
    pub fn role_known(&self) -> Option<&RoleRead> {
        let role = self.started.as_ref()?.role.as_ref()?;
        Some(self.role_told.as_ref().unwrap_or(&role.read))
    }

    /// The limit it waits on, parked on it since it was seen, until its
    /// reset: no finding meanwhile.
    pub fn waits_on_limit(&self) -> Option<&Limit> {
        let waits = self.parked && self.status == Status::Waiting;
        self.limit.as_ref().filter(|l| waits && l.at == self.since)
    }
}

/// The longest wait a worker can declare: a bound past it is held to it.
pub const DECLARED_MAX_MS: i64 = 2 * 60 * 60 * 1000;

/// A wait a worker declared: what for, when, and until when, in ms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Declared {
    pub why: String,
    pub at: i64,
    pub until: i64,
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
    /// It asked to finish without a landing, for this evidence.
    FinishAsked {
        evidence: String,
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
        /// What kind of red; `None`: one before factorium#308.
        #[serde(default)]
        kind: Option<RedKind>,
        /// The files it names: those that conflict, or the regenerated
        /// records its merge took proving's copy of.
        #[serde(default)]
        files: Vec<String>,
    },
    /// Waiting its turn, its branch was found to merge onto the proving
    /// tip (factorium#307's check); one that does not is red instead.
    MergeChecked {
        tip: String,
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
    /// Its silence clock is held: a heavy entry of its own went in flight
    /// (waiting or running), or its issue was blocked.
    HeavyHeld,
    /// What held its silence clock all let go.
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
    /// Its issue was blocked, while it waited or worked: before factorium#308,
    /// which records why ([`WorkerEvent::ParkedBehind`]).
    WorkerParked,
    /// Its issue was blocked, while it waited or worked, for this.
    ParkedBehind {
        why: String,
    },
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
    /// It declared a wait on a long command of its own, until then (ms).
    WaitDeclared(Declared),
    /// Sent back to work within the wait it declared: the wait is over.
    WaitEnded,
    /// Its engine is at its limit, as its screen said, until then (ms): it
    /// waits, parked until the reset.
    WorkerLimited(Limit),
    /// It was told its role's directory changed to hold this.
    RoleTold(RoleRead),
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
            WorkerEvent::FinishAsked { evidence, at } => (Status::Finishing, Some(evidence.clone()), *at),
            WorkerEvent::LandingRed { why, at, .. } => (Status::Working, Some(why.clone()), *at),
            WorkerEvent::WorkerLanded { at, .. } => (Status::Ended, Some("landed".into()), *at),
            WorkerEvent::WorkerWaits { why, at } => (Status::Waiting, Some(why.clone()), *at),
            WorkerEvent::WorkerResumed { at } => (Status::Working, None, *at),
            WorkerEvent::WorkerEnded { why, at } => (Status::Ended, Some(why.clone()), *at),
            WorkerEvent::WorkerLimited(l) => (
                Status::Waiting,
                Some(format!("its engine is at its limit, parked until it resets at {} (ms): {}", l.until, l.said)),
                l.at,
            ),
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
            (s.status, s.why, s.since, s.declared) = (status, why, since, None);
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
            WorkerEvent::WorkerParked | WorkerEvent::ParkedBehind { .. } => s.parked = true,
            WorkerEvent::WorkerUnparked => s.parked = false,
            WorkerEvent::WentQuiet { screen, at } => s.quiet = Some((screen, at)),
            WorkerEvent::Stirred => s.quiet = None,
            WorkerEvent::WentOver { at } => s.over_since = Some(at),
            WorkerEvent::CameUnder => s.over_since = None,
            WorkerEvent::HeavyHeld => s.heavy = true,
            WorkerEvent::HeavyFreed { at } => (s.heavy, s.freed_at) = (false, Some(at)),
            WorkerEvent::Spent(tokens) => s.spent = Some(tokens),
            WorkerEvent::WaitDeclared(d) => s.declared = Some(d),
            WorkerEvent::WaitEnded => s.declared = None,
            WorkerEvent::WorkerLimited(l) => (s.limit, s.parked) = (Some(l), true),
            WorkerEvent::RoleTold(read) => s.role_told = Some(read),
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

/// Defines a command on one worker, whose params carry `worker`.
macro_rules! on_worker {
    ($command:ident, $name:literal, $params:ty, |$s:ident, $p:ident, $now:ident| $decide:expr) => {
        command!($command, $name, Workers, $params, |p| p.worker, |$s, $p, $now| $decide);
    };
}

on_worker!(AskLanding, "worker.done", Of, |s, p, now| {
    live(s)?;
    match s.status {
        Status::Working => Ok(vec![WorkerEvent::LandingAsked { at: now.0 }]),
        Status::Landing => Ok(Vec::new()),
        _ => no("waiting", "the worker waits for the watch"),
    }
});

// It asks to finish without a landing: its issue's work changes no code
// (a publish, a measurement), and `why` is the evidence that closes it.
on_worker!(AskFinish, "worker.finish", Because, |s, p, now| {
    live(s)?;
    if p.why.trim().is_empty() {
        return no("no_evidence", "say what you did and how you verified it: it closes your issue");
    }
    if s.status != Status::Working {
        return no("not_working", "only a working worker finishes");
    }
    Ok(vec![WorkerEvent::FinishAsked { evidence: p.why.clone(), at: now.0 }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Because {
    pub worker: Uuid,
    pub why: String,
}

/// What kind of red a landing was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedKind {
    /// Its branch does not merge onto the proving tip.
    NoMerge,
    /// The fast gate is red on the merge.
    GateRed,
    /// The fast gate ran past its limit on the merge.
    PastLimit,
    /// The fast gate is red on a merge whose only conflicts were records
    /// the gate regenerates, taken from proving's copy.
    Regenerated,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Red {
    pub worker: Uuid,
    pub why: String,
    pub kind: RedKind,
    /// The files it names: those that conflict, or the regenerated records taken.
    pub files: Vec<String>,
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

/// Refused unless the worker is live and landing.
fn landing(s: &Worker) -> Result<(), Rejection> {
    live(s)?;
    if s.status == Status::Landing { Ok(()) } else { no("not_landing", "the worker is not landing") }
}

// A red landing: back to work, or, the second in a row, to the watch.
// One that does not merge because proving moved again is not the second.
on_worker!(RedLanding, "worker.red", Red, |s, p, now| {
    landing(s)?;
    let moved = p.moved(s);
    let (kind, files) = (Some(p.kind), p.files.clone());
    let mut events =
        vec![WorkerEvent::LandingRed { why: p.why.clone(), at: now.0, tip: p.tip.clone(), moved, kind, files }];
    if !moved && s.reds + 1 >= 2 {
        events.push(WorkerEvent::WorkerWaits { why: format!("red twice in a row: {}", p.why), at: now.0 });
    }
    Ok(events)
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Land {
    pub worker: Uuid,
    pub commit: String,
}

on_worker!(GreenLanding, "worker.landed", Land, |s, p, now| {
    landing(s)?;
    Ok(vec![WorkerEvent::WorkerLanded { commit: p.commit.clone(), at: now.0 }])
});

// The worker waits for the watch: it escalated, or it failed.
on_worker!(Wait, "worker.wait", Because, |s, p, now| {
    live(s)?;
    only(s.status != Status::Waiting, WorkerEvent::WorkerWaits { why: p.why.clone(), at: now.0 })
});

// The watch sends a waiting worker back to work; one working within a
// wait it declared, the wait is over.
on_worker!(Resume, "worker.resume", Of, |s, p, now| {
    live(s)?;
    Ok(match s.status {
        Status::Waiting => vec![WorkerEvent::WorkerResumed { at: now.0 }],
        Status::Working if s.declared.is_some() => vec![WorkerEvent::WaitEnded],
        _ => Vec::new(),
    })
});

// Its issue was blocked: its worker is parked behind what its issue waits
// for, waiting now or working, to go back once it is idle (it waits, or is
// in a wait it declared) and all of that is met.
on_worker!(Park, "worker.park", Because, |s, p, now| {
    let why = p.why.clone();
    only(s.started.is_some() && s.status != Status::Ended && !s.parked, WorkerEvent::ParkedBehind { why })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checked {
    pub worker: Uuid,
    /// The proving tip its branch merges onto.
    pub tip: String,
}

// Waiting its turn, its branch merges onto the proving tip.
on_worker!(NoteMergeChecked, "worker.merge_checked", Checked, |s, p, now| {
    landing(s)?;
    Ok(vec![WorkerEvent::MergeChecked { tip: p.tip.clone() }])
});

// What a parked worker's issue waited for is met, or its engine's limit
// reset: it is no longer parked, whether or not its restart reaches it.
on_worker!(Unpark, "worker.unpark", Of, |s, p, now| only(s.parked, WorkerEvent::WorkerUnparked));

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Closed {
    pub worker: Uuid,
    /// Why its terminal is not closed; `None`: Orca no longer lists it.
    pub failed: Option<String>,
}

// How closing its terminal went. Recorded before it ends: an ended worker
// takes no more commands.
on_worker!(NoteClosed, "worker.closed", Closed, |s, p, now| {
    if s.started.is_none() {
        return no("no_worker", "no such worker");
    }
    Ok(vec![WorkerEvent::SessionClosed { failed: p.failed.clone() }])
});

on_worker!(EndWorker, "worker.end", Because, |s, p, now| {
    live(s)?;
    Ok(vec![WorkerEvent::WorkerEnded { why: p.why.clone(), at: now.0 }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Declare {
    pub worker: Uuid,
    pub why: String,
    /// How long it waits, in ms; held to [`DECLARED_MAX_MS`].
    pub for_ms: i64,
}

// A working worker declares a wait on a long command of its own: its
// silence is not found until the bound, held to [`DECLARED_MAX_MS`].
on_worker!(DeclareWait, "worker.declare_wait", Declare, |s, p, now| {
    live(s)?;
    if s.status != Status::Working {
        return no("not_working", "only a working worker declares a wait");
    }
    if p.why.trim().is_empty() {
        return no("no_reason", "say what you wait for");
    }
    if p.for_ms <= 0 {
        return no("no_bound", "say until when: a duration past now");
    }
    let until = now.0 + p.for_ms.min(DECLARED_MAX_MS);
    Ok(vec![WorkerEvent::WaitDeclared(Declared { why: p.why.clone(), at: now.0, until })])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spend {
    pub worker: Uuid,
    pub tokens: Tokens,
}

// What its session spent, as its session files say. Recorded before it
// ends: an ended worker takes no more commands.
on_worker!(NoteSpent, "worker.spent", Spend, |s, p, now| {
    live(s)?;
    Ok(vec![WorkerEvent::Spent(p.tokens)])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TellRole {
    pub worker: Uuid,
    pub read: RoleRead,
}

// It was told its role changed to hold this: told once per change.
on_worker!(NoteRoleTold, "worker.role_told", TellRole, |s, p, now| {
    live(s)?;
    only(s.role_known().is_none_or(|k| k.hash != p.read.hash), WorkerEvent::RoleTold(p.read.clone()))
});
