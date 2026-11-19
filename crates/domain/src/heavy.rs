//! The heavy-command queue: one host-wide line of the commands that load
//! the machine — every gate run, and what a worker runs through
//! `factorium worker exclusive`. The oldest waiting starts when the host
//! has room, or when nothing runs, so it never starves; one start per
//! reading settled after the last start; and one that holds others up past the hold bound is ended. A
//! line move past its bound goes ahead of workers' waiting entries. The
//! rules are those of the newest version that asked: a binary older than it
//! is refused, however long it has waited (factorium#106).
//! That start rule is neutral: [`crate::neutral::queue`]; here are the
//! queue's stream, the worker an entry is for, and the line jobs' names.

use factum_core::{Aggregate, Uuid};
use serde::{Deserialize, Serialize};

use crate::declared::Kind;
use crate::neutral::queue::{self, Queued};
use crate::{command, id, no};

pub fn queue_id() -> Uuid {
    id("heavy")
}

/// The version of the rules this binary asks under: its own, which a cut sets.
pub const RULES: &str = env!("CARGO_PKG_VERSION");

/// Whether rules `a` are older than rules `b`: versions compared part by part.
pub fn older(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(a) < parts(b)
}

/// A proving line's heavy command. Its name is made here alone: the binary
/// queues it by that name, and doctor knows by it that a line's move runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineJob {
    Gate(Kind),
    Cut,
}

impl LineJob {
    /// What the queue calls this job of `checkout`.
    pub fn of(self, checkout: &str) -> String {
        match self {
            LineJob::Gate(kind) => format!("{} gate of {checkout}", crate::named(kind)),
            LineJob::Cut => format!("cut of {checkout}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: Uuid,
    pub what: String,
    /// The process that waits for it, to forget it when that dies.
    pub pid: u32,
    pub worker: Option<Uuid>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    /// When the line move it is for falls due however busy its checkout is
    /// (a hold bound before its bound), ms: past then, it goes ahead of
    /// workers' waiting entries.
    #[serde(default)]
    pub bound_at: Option<i64>,
    /// The session the running step runs in, once its waiter noted it:
    /// every process the step started, wherever it moved, so ending the
    /// entry from another process ends them all.
    #[serde(default)]
    pub session: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Queue {
    pub entries: Vec<Entry>,
    /// When the last one started: one start per reading.
    pub last_start: i64,
    /// The newest rules that asked: older ones may not start.
    #[serde(default)]
    pub rules: String,
}

impl Queued for Entry {
    fn id(&self) -> Uuid {
        self.id
    }

    fn what(&self) -> &str {
        &self.what
    }

    fn queued_at(&self) -> i64 {
        self.queued_at
    }

    fn started_at(&self) -> Option<i64> {
        self.started_at
    }

    // A line move past its bound goes ahead of workers' entries.
    fn yields(&self) -> bool {
        self.worker.is_some()
    }

    fn bound_at(&self) -> Option<i64> {
        self.bound_at
    }
}

impl Queue {
    pub fn running(&self) -> impl Iterator<Item = &Entry> {
        queue::running(&self.entries)
    }

    pub fn waiting(&self) -> impl Iterator<Item = &Entry> {
        queue::waiting(&self.entries)
    }

    /// The waiting entry queued first.
    pub fn oldest(&self) -> Option<&Entry> {
        queue::oldest(&self.entries)
    }

    /// Whether the line job `job` of `checkout` runs now.
    pub fn runs(&self, checkout: &str, job: LineJob) -> bool {
        let what = job.of(checkout);
        self.running().any(|e| e.what == what)
    }

    /// Whether `id` may start at `now`, given whether the host has room, when
    /// the reading was taken, and how long a start takes to show; else why not.
    pub fn may_start(
        &self,
        id: Uuid,
        room: Result<(), String>,
        read_at: i64,
        settle: i64,
        now: i64,
    ) -> Result<(), String> {
        let starts = queue::Starts { last: self.last_start, settle };
        queue::may_start(&self.entries, starts, id, room, read_at, now)
    }

    /// Whether `id` is queued, waiting or running.
    pub fn holds(&self, id: Uuid) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// Whether the running `id` holds others up past `bound` and must end.
    pub fn overheld(&self, id: Uuid, now: i64, bound: i64) -> bool {
        queue::overheld(&self.entries, id, now, bound)
    }

    /// Why the running `id` must end at `now`, if it must: unqueued, held
    /// past `bound`, or one that `yields` with anything else queued.
    pub fn ends(&self, id: Uuid, now: i64, bound: i64, yields: bool) -> Option<Ends> {
        if !self.holds(id) {
            Some(Ends::Unqueued)
        } else if self.overheld(id, now, bound) {
            Some(Ends::Held)
        } else {
            (yields && self.entries.iter().any(|e| e.id != id)).then_some(Ends::GaveWay)
        }
    }
}

/// Why a running entry is ended before its steps are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ends {
    Unqueued,
    /// It held others up past the hold bound.
    Held,
    /// It yields, and something else came first.
    GaveWay,
}

impl std::fmt::Display for Ends {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Ends::Unqueued => "unqueued",
            Ends::Held => "it held others up past the hold bound",
            Ends::GaveWay => "it gave way: the host is no longer idle",
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum HeavyEvent {
    HeavyQueued {
        id: Uuid,
        what: String,
        pid: u32,
        worker: Option<Uuid>,
        at: i64,
        #[serde(default)]
        bound_at: Option<i64>,
    },
    HeavyStarted {
        id: Uuid,
        at: i64,
    },
    HeavyEnded {
        id: Uuid,
    },
    /// A started entry's step runs in session `session`.
    HeavyRuns {
        id: Uuid,
        session: u32,
    },
    /// A binary with newer rules than any before asked.
    HeavyRules {
        version: String,
    },
}

pub struct Heavy;

impl Aggregate for Heavy {
    const NAME: &'static str = "heavy";
    type State = Queue;
    type Event = HeavyEvent;

    fn evolve(s: &mut Queue, event: HeavyEvent) {
        match event {
            HeavyEvent::HeavyQueued { id, what, pid, worker, at, bound_at } => s.entries.push(Entry {
                id,
                what,
                pid,
                worker,
                queued_at: at,
                started_at: None,
                bound_at,
                session: None,
            }),
            HeavyEvent::HeavyStarted { id, at } => {
                s.entries.iter_mut().filter(|e| e.id == id).for_each(|e| e.started_at = Some(at));
                s.last_start = at;
            }
            HeavyEvent::HeavyEnded { id } => s.entries.retain(|e| e.id != id),
            HeavyEvent::HeavyRuns { id, session } => {
                s.entries.iter_mut().filter(|e| e.id == id).for_each(|e| e.session = Some(session))
            }
            HeavyEvent::HeavyRules { version } => s.rules = version,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Enqueue {
    pub id: Uuid,
    pub what: String,
    pub pid: u32,
    pub worker: Option<Uuid>,
    /// When the line move it is for passes its bound, ms.
    #[serde(default)]
    pub bound_at: Option<i64>,
}

command!(EnqueueHeavy, "heavy.queue", Heavy, Enqueue, |_p| queue_id(), |s, p, now| {
    if s.entries.iter().any(|e| e.id == p.id) {
        return Ok(Vec::new());
    }
    Ok::<_, factum_core::Rejection>(vec![HeavyEvent::HeavyQueued {
        id: p.id,
        what: p.what.clone(),
        pid: p.pid,
        worker: p.worker,
        at: now.0,
        bound_at: p.bound_at,
    }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Begin {
    pub id: Uuid,
    /// Whether the host had room, and why not.
    pub room: Result<(), String>,
    /// When that reading was taken, ms: before the host was read.
    pub read_at: i64,
    /// How long after the last start a reading shows its load, ms.
    pub settle: i64,
    /// The rules it asks under: [`RULES`] of the binary that asks.
    #[serde(default)]
    pub rules: String,
}

command!(StartHeavy, "heavy.start", Heavy, Begin, |_p| queue_id(), |s, p, now| {
    if older(&p.rules, &s.rules) {
        let why = format!("asks under the rules of {}, which {} replaced", p.rules, s.rules);
        return no("stale", why);
    }
    match s.may_start(p.id, p.room.clone(), p.read_at, p.settle, now.0) {
        Ok(()) => Ok(vec![HeavyEvent::HeavyStarted { id: p.id, at: now.0 }]),
        Err(why) => no("wait", why),
    }
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct End {
    pub id: Uuid,
}

command!(EndHeavy, "heavy.end", Heavy, End, |_p| queue_id(), |s, p, now| {
    Ok::<_, factum_core::Rejection>(if s.entries.iter().any(|e| e.id == p.id) {
        vec![HeavyEvent::HeavyEnded { id: p.id }]
    } else {
        Vec::new()
    })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Runs {
    pub id: Uuid,
    pub session: u32,
}

command!(NoteRun, "heavy.runs", Heavy, Runs, |_p| queue_id(), |s, p, _now| {
    let started = s.entries.iter().any(|e| e.id == p.id && e.started_at.is_some());
    Ok::<_, factum_core::Rejection>(if started {
        vec![HeavyEvent::HeavyRuns { id: p.id, session: p.session }]
    } else {
        Vec::new()
    })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rules {
    pub version: String,
}

command!(NoteRules, "heavy.rules", Heavy, Rules, |_p| queue_id(), |s, p, _now| {
    Ok::<_, factum_core::Rejection>(if older(&s.rules, &p.version) {
        vec![HeavyEvent::HeavyRules { version: p.version.clone() }]
    } else {
        Vec::new()
    })
});
