//! Where an attempt's time went (factorium#308), read from what factorium
//! keeps of it and nothing written twice: its worker's events (each with
//! the time the log took it), the heavy runs' rows of its own commands and
//! of its landings' gates ([`crate::tracker::RunRow`]), and its session's
//! outbox. Each step has its start and end, in ms; one still open, none.

use factum_core::Uuid;
use serde::Serialize;

use crate::heavy::Outcome;
use crate::outbox::{OutboxEvent, Sender};
use crate::tracker::RunRow;
use crate::worker::{RedKind, Status, WorkerEvent};

/// An attempt's steps.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Steps {
    /// Each heavy command it ran (`worker gate`, `worker exclusive`, a
    /// hold), as asked: waiting in the queue, then running.
    pub heavy: Vec<Run>,
    /// Each `done`, from the ask to how it ended.
    pub landings: Vec<Landing>,
    /// Each wait it declared (`worker waiting`).
    pub waits: Vec<Wait>,
    /// Each time it was parked: behind what its issue waits for, or on its
    /// engine's limit.
    pub parks: Vec<Park>,
    /// Each message passed to its session: who from, when given, and when
    /// it reached it. Their text is the outbox's (`factorium outbox --worker`).
    pub messages: Vec<Told>,
}

/// One heavy command: asked, granted, ended, and how it ran.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Run {
    pub entry: Uuid,
    pub what: String,
    pub hold: bool,
    pub asked_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    /// From its ask to its grant (to now, or its end, if never granted).
    pub queued_ms: i64,
    /// From its grant to its end (to now, if it runs); none if never granted.
    pub ran_ms: Option<i64>,
    pub outcome: Option<Outcome>,
}

impl Run {
    fn of(r: &RunRow, now: i64) -> Run {
        let granted_or = r.started_at.or(r.ended_at).unwrap_or(now);
        Run {
            entry: r.entry,
            what: r.what.clone(),
            hold: r.hold,
            asked_at: r.asked_at,
            started_at: r.started_at,
            ended_at: r.ended_at,
            queued_ms: (granted_or - r.asked_at).max(0),
            ran_ms: r.started_at.map(|s| (r.ended_at.unwrap_or(now) - s).max(0)),
            outcome: r.outcome,
        }
    }
}

/// One landing: `done` asked, each check that its branch merges while it
/// waited its turn, each fast gate on its merge (asked at its turn in the
/// line), and how it ended.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Landing {
    pub asked_at: i64,
    pub checked: Vec<Checked>,
    pub gates: Vec<Run>,
    pub ended_at: Option<i64>,
    /// `landed`, `red`, or the status it left landing for otherwise
    /// (`waiting`, `ended`, `working`); none while it lands.
    pub outcome: Option<String>,
    pub commit: Option<String>,
    pub red: Option<Red>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Checked {
    pub at: i64,
    pub tip: String,
}

/// A red landing: its kind (none before factorium#308), the files it named,
/// the proving tip it did not merge onto, and the message that told it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Red {
    pub kind: Option<RedKind>,
    pub files: Vec<String>,
    pub tip: Option<String>,
    pub moved: bool,
    pub told: Option<Told>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Wait {
    pub why: String,
    pub at: i64,
    /// Until when it declared it.
    pub until: i64,
    pub ended_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Park {
    pub why: String,
    pub at: i64,
    /// Its engine's reset, parked on its limit.
    pub until: Option<i64>,
    pub ended_at: Option<i64>,
}

/// A message to its session, by its number in the outbox.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Told {
    pub n: u64,
    pub from: Sender,
    pub at: i64,
    /// How its last try went (the outbox's word); none: not tried yet.
    pub delivery: Option<&'static str>,
    /// When it first reached the session (typed, submitted, queued, or in
    /// its brief); none: it has not.
    pub reached_at: Option<i64>,
}

/// The steps of an attempt from `events`, its worker's events each with the
/// time it was taken; `runs`, its heavy runs' rows; and `outbox`, its
/// session's outbox events each with its time. `now` ends nothing: a step
/// still open has no end.
pub fn steps(events: &[(i64, WorkerEvent)], runs: &[RunRow], outbox: &[(i64, OutboxEvent)], now: i64) -> Steps {
    let messages = told(outbox);
    let mut s = Steps {
        heavy: runs.iter().filter(|r| !r.lands && shown(r)).map(|r| Run::of(r, now)).collect(),
        messages: messages.clone(),
        ..Steps::default()
    };
    for (at, e) in events {
        let at = *at;
        if let Some(st) = e.standing() {
            if let Some(l) = s.landings.last_mut().filter(|l| l.ended_at.is_none() && st.status != Status::Landing) {
                l.ended_at = Some(at);
                l.outcome = Some(word(st.status, e).into());
            }
            close(s.waits.iter_mut().map(|w| &mut w.ended_at), at);
        }
        match e {
            WorkerEvent::LandingAsked { at: asked } => s.landings.push(Landing {
                asked_at: *asked,
                checked: Vec::new(),
                gates: Vec::new(),
                ended_at: None,
                outcome: None,
                commit: None,
                red: None,
            }),
            WorkerEvent::MergeChecked { tip, at } => {
                if let Some(l) = s.landings.last_mut() {
                    l.checked.push(Checked { at: *at, tip: tip.clone() });
                }
            }
            WorkerEvent::LandingRed { at: red_at, tip, moved, kind, files, .. } => {
                let told = messages.iter().find(|m| m.from == Sender::Factorium && m.at >= *red_at).cloned();
                if let Some(l) = s.landings.last_mut() {
                    let red = Red { kind: *kind, files: files.clone(), tip: tip.clone(), moved: *moved, told };
                    l.red = Some(red);
                }
            }
            WorkerEvent::WorkerLanded { commit, .. } => {
                if let Some(l) = s.landings.last_mut() {
                    l.commit = Some(commit.clone());
                }
            }
            WorkerEvent::WaitDeclared(d) => {
                s.waits.push(Wait { why: d.why.clone(), at: d.at, until: d.until, ended_at: None })
            }
            WorkerEvent::WaitEnded => close(s.waits.iter_mut().map(|w| &mut w.ended_at), at),
            WorkerEvent::WorkerParked => s.parks.push(park("(no reason given)".into(), at, None)),
            WorkerEvent::ParkedBehind { why } => s.parks.push(park(why.clone(), at, None)),
            WorkerEvent::WorkerLimited(l) => {
                s.parks.push(park(format!("its engine is at its limit: {}", l.said), l.at, Some(l.until)))
            }
            _ => {}
        }
        // What ends a park: unparked, sent back to work, or its worker's end.
        if matches!(
            e,
            WorkerEvent::WorkerUnparked
                | WorkerEvent::WorkerResumed { .. }
                | WorkerEvent::WorkerLanded { .. }
                | WorkerEvent::WorkerEnded { .. }
        ) {
            close(s.parks.iter_mut().map(|p| &mut p.ended_at), at);
        }
    }
    // Each landing's gates: those asked from its `done` to its end.
    for l in &mut s.landings {
        let within = |r: &&RunRow| r.lands && r.asked_at >= l.asked_at && l.ended_at.is_none_or(|e| r.asked_at <= e);
        l.gates = runs.iter().filter(within).filter(|r| shown(r)).map(|r| Run::of(r, now)).collect();
    }
    s
}

/// Whether a run is shown: not one that left the queue unstarted, whose
/// wait the run that took its place counts.
fn shown(r: &RunRow) -> bool {
    r.started_at.is_some() || r.ended_at.is_none()
}

fn park(why: String, at: i64, until: Option<i64>) -> Park {
    Park { why, at, until, ended_at: None }
}

/// Ends each of `open` still open, at `at`.
fn close<'a>(open: impl Iterator<Item = &'a mut Option<i64>>, at: i64) {
    open.filter(|e| e.is_none()).for_each(|e| *e = Some(at));
}

/// How a landing ended, by the event that ended it.
fn word(status: Status, e: &WorkerEvent) -> &'static str {
    match (e, status) {
        (WorkerEvent::LandingRed { .. }, _) => "red",
        (WorkerEvent::WorkerLanded { .. }, _) => "landed",
        (_, Status::Waiting) => "waiting",
        (_, Status::Ended) => "ended",
        (_, Status::Finishing) => "finishing",
        (_, Status::Working | Status::Landing) => "working",
    }
}

/// Each message given, with its last outcome and when it first arrived.
fn told(outbox: &[(i64, OutboxEvent)]) -> Vec<Told> {
    let mut all: Vec<Told> = Vec::new();
    for (at, e) in outbox {
        match e {
            OutboxEvent::Given { n, from, at: given, .. } => {
                all.push(Told { n: *n, from: from.clone(), at: *given, delivery: None, reached_at: None })
            }
            OutboxEvent::Tried { n, outcome } => {
                if let Some(m) = all.iter_mut().find(|m| m.n == *n) {
                    m.delivery = Some(outcome.word());
                    if outcome.arrived() && m.reached_at.is_none() {
                        m.reached_at = Some(*at);
                    }
                }
            }
        }
    }
    all
}
