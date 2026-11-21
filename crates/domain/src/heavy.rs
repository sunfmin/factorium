//! The heavy-command queue: one host-wide line of the commands that load
//! the machine — every gate run, what a worker runs through
//! `factorium worker exclusive`, and what a program of the owner's that is
//! not a worker runs through `factorium heavy run`. The oldest waiting starts when the host
//! has room, or when nothing runs, so it never starves; one start per
//! reading settled after the last start; and one that holds others up past the hold bound is ended. A
//! line move past its bound goes ahead of workers' waiting entries. The
//! rules are those of the newest version that asked: a binary older than it
//! is refused, however long it has waited (factorium#106). A worker's or a
//! program's entry that left the queue unstarted (its waiter refused, or
//! gone) keeps its place for the keep bound: the same asker asking the same
//! command again takes it (factorium#193).
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
    /// Queued by a program of the owner's that is not a worker (`heavy
    /// run`): it yields to a line move past its bound as a worker's does.
    #[serde(default)]
    pub program: bool,
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
    /// The places of entries that left unstarted, kept for their askers.
    #[serde(default)]
    pub left: Vec<Left>,
}

/// The place of a worker's or a program's entry that left the queue
/// unstarted, not taken off by the owner: its asker asking the same command
/// again within the keep bound takes it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Left {
    pub what: String,
    pub worker: Option<Uuid>,
    pub program: bool,
    /// The place it had: when it was queued (or the place it took), ms.
    pub queued_at: i64,
    /// When it left, ms.
    pub left_at: i64,
}

impl Left {
    /// Whether it is the place of `what` asked by `worker` (or a program).
    fn of(&self, what: &str, worker: Option<Uuid>, program: bool) -> bool {
        self.what == what && self.worker == worker && self.program == program
    }
}

/// How long a left entry's place is kept at most, whatever the keep bound:
/// so the kept places stay few. A day, ms.
const LEFT_KEPT_MAX: i64 = 24 * 3600 * 1000;

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

    // A line move past its bound goes ahead of workers' and programs' entries.
    fn yields(&self) -> bool {
        self.worker.is_some() || self.program
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

    /// The place `what`, asked by `worker` (or a program), takes at `now`: that of its entry
    /// that left unstarted at most `keep` ms before, if one did; else none, and it queues at the back.
    pub fn place(&self, what: &str, worker: Option<Uuid>, program: bool, now: i64, keep: i64) -> Option<i64> {
        // Only a worker's or a program's is kept: none other matches.
        let kept = self.left.iter().filter(|l| l.of(what, worker, program) && now - l.left_at <= keep);
        kept.map(|l| l.queued_at).min()
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
        #[serde(default)]
        program: bool,
        at: i64,
        #[serde(default)]
        bound_at: Option<i64>,
        /// The place it took, kept from its asker's entry that left
        /// unstarted: its `queued_at`, not `at`.
        #[serde(default)]
        place: Option<i64>,
    },
    HeavyStarted {
        id: Uuid,
        at: i64,
    },
    HeavyEnded {
        id: Uuid,
        /// When it left unstarted with its place kept for its asker, ms;
        /// none: its place goes.
        #[serde(default)]
        kept_at: Option<i64>,
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
            HeavyEvent::HeavyQueued { id, what, pid, worker, program, at, bound_at, place } => {
                s.left.retain(|l| !l.of(&what, worker, program));
                let queued_at = place.unwrap_or(at);
                let session = None;
                s.entries.push(Entry { id, what, pid, worker, program, queued_at, started_at: None, bound_at, session })
            }
            HeavyEvent::HeavyStarted { id, at } => {
                s.entries.iter_mut().filter(|e| e.id == id).for_each(|e| e.started_at = Some(at));
                s.last_start = at;
            }
            HeavyEvent::HeavyEnded { id, kept_at } => {
                if let Some(at) = kept_at {
                    s.left.retain(|l| at - l.left_at <= LEFT_KEPT_MAX);
                    let left = s.entries.iter().find(|e| e.id == id && e.started_at.is_none() && e.yields());
                    s.left.extend(left.map(|e| Left {
                        what: e.what.clone(),
                        worker: e.worker,
                        program: e.program,
                        queued_at: e.queued_at,
                        left_at: at,
                    }));
                }
                s.entries.retain(|e| e.id != id)
            }
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
    /// Queued by a program of the owner's that is not a worker.
    #[serde(default)]
    pub program: bool,
    /// When the line move it is for passes its bound, ms.
    #[serde(default)]
    pub bound_at: Option<i64>,
    /// How long the place of its asker's entry that left unstarted is kept
    /// for it, ms: `heavy_keep`.
    #[serde(default)]
    pub keep: i64,
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
        program: p.program,
        at: now.0,
        bound_at: p.bound_at,
        place: s.place(&p.what, p.worker, p.program, now.0, p.keep),
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
    /// Whether its place is kept for its asker, if it leaves unstarted: its
    /// waiter was refused or is gone, and asked again takes it. The owner's
    /// unqueue, or one no longer due, does not keep it.
    #[serde(default)]
    pub keeps: bool,
}

command!(EndHeavy, "heavy.end", Heavy, End, |_p| queue_id(), |s, p, now| {
    // Whether its place is kept (unstarted, a worker's or a program's) is evolve's.
    let ended = HeavyEvent::HeavyEnded { id: p.id, kept_at: p.keeps.then_some(now.0) };
    Ok::<_, factum_core::Rejection>(if s.holds(p.id) { vec![ended] } else { Vec::new() })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u128, queued_at: i64) -> Entry {
        Entry { id: Uuid::from_u128(n), what: n.to_string(), queued_at, ..Entry::default() }
    }

    #[test]
    fn a_line_move_past_its_bound_goes_ahead_of_a_programs_entry_as_of_a_workers() {
        let line = Entry { bound_at: Some(5), ..entry(3, 2) };
        for asker in [Entry { program: true, ..entry(1, 1) }, Entry { worker: Some(Uuid::from_u128(9)), ..entry(1, 1) }]
        {
            let q = Queue { entries: vec![asker, line.clone()], ..Queue::default() };
            assert_eq!(queue::next(&q.entries, 6).map(|e| e.id), Some(line.id));
        }
        // A cut, say, neither a worker's nor a program's: it keeps its place.
        let q = Queue { entries: vec![entry(1, 1), line.clone()], ..Queue::default() };
        assert_eq!(queue::next(&q.entries, 6).map(|e| e.id), Some(Uuid::from_u128(1)));
    }

    fn queued(n: u128, what: &str, worker: Option<Uuid>, at: i64, place: Option<i64>) -> HeavyEvent {
        let (id, what) = (Uuid::from_u128(n), what.to_string());
        HeavyEvent::HeavyQueued { id, what, pid: 1, worker, program: false, at, bound_at: None, place }
    }

    #[test]
    fn an_entry_that_left_unstarted_keeps_its_place_for_its_asker_alone_within_the_keep_bound() {
        let (me, other) = (Some(Uuid::from_u128(7)), Some(Uuid::from_u128(8)));
        let mut q = Queue::default();
        Heavy::evolve(&mut q, queued(1, "check", me, 10, None));
        Heavy::evolve(&mut q, HeavyEvent::HeavyEnded { id: Uuid::from_u128(1), kept_at: Some(20) });
        assert_eq!(q.place("check", me, false, 30, 10), Some(10), "asked again within the bound");
        assert_eq!(q.place("check", me, false, 31, 10), None, "past it");
        assert_eq!(q.place("check", other, false, 30, 10), None, "another worker's");
        assert_eq!(q.place("gate", me, false, 30, 10), None, "another command");
        assert_eq!(q.place("check", None, false, 30, 10), None, "neither a worker's nor a program's");
        // Taken, it is gone.
        Heavy::evolve(&mut q, queued(2, "check", me, 30, Some(10)));
        assert_eq!(q.entries[0].queued_at, 10, "it takes the place it had");
        assert!(q.left.is_empty());
        // Ended with its place gone (the owner's unqueue), or once started, none is kept.
        Heavy::evolve(&mut q, HeavyEvent::HeavyEnded { id: Uuid::from_u128(2), kept_at: None });
        Heavy::evolve(&mut q, queued(3, "check", me, 40, None));
        Heavy::evolve(&mut q, HeavyEvent::HeavyStarted { id: Uuid::from_u128(3), at: 41 });
        Heavy::evolve(&mut q, HeavyEvent::HeavyEnded { id: Uuid::from_u128(3), kept_at: Some(42) });
        assert!(q.left.is_empty(), "{:?}", q.left);
    }

    #[test]
    fn a_kept_place_older_than_a_day_goes_once_another_leaves() {
        let (me, other) = (Some(Uuid::from_u128(7)), Some(Uuid::from_u128(8)));
        let mut q = Queue::default();
        let day = 86_400_000;
        Heavy::evolve(&mut q, queued(1, "check", me, 10, None));
        Heavy::evolve(&mut q, HeavyEvent::HeavyEnded { id: Uuid::from_u128(1), kept_at: Some(day) });
        Heavy::evolve(&mut q, queued(2, "gate", other, 2 * day - 1, None));
        Heavy::evolve(&mut q, HeavyEvent::HeavyEnded { id: Uuid::from_u128(2), kept_at: Some(2 * day) });
        assert_eq!(q.left.len(), 2, "a day old, it is kept");
        Heavy::evolve(&mut q, queued(3, "test", other, 2 * day, None));
        Heavy::evolve(&mut q, HeavyEvent::HeavyEnded { id: Uuid::from_u128(3), kept_at: Some(2 * day + 1) });
        assert_eq!(q.left.iter().map(|l| l.what.as_str()).collect::<Vec<_>>(), ["gate", "test"], "past a day, it went");
    }
}
