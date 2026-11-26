//! Where an attempt's time went (factorium#308), read from what factorium
//! keeps of it and nothing written twice: its worker's events (each with
//! the time the log took it), the heavy runs' rows of its own commands and
//! of its landings' gates ([`crate::tracker::RunRow`]), and its session's
//! outbox. Each step has its start and end, in ms; one still open, none.

use serde::Serialize;

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
    /// Each wait it declared (`worker waiting`), until when it declared it.
    pub waits: Vec<Span>,
    /// Each time it was parked: behind what its issue waits for, or on its
    /// engine's limit, until its reset.
    pub parks: Vec<Span>,
    /// Each message passed to its session: who from, when given, and when
    /// it reached it. Their text is the outbox's (`outbox --worker`).
    pub messages: Vec<Told>,
}

/// One heavy command as its row keeps it, with its wait and its run apart.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Run {
    #[serde(flatten)]
    pub run: RunRow,
    /// From its ask to its grant (to now, or its end, if never granted).
    pub queued_ms: i64,
    /// From its grant to its end (to now, if it runs); none if never granted.
    pub ran_ms: Option<i64>,
}

impl Run {
    fn of(r: &RunRow, now: i64) -> Run {
        let granted_or = r.started_at.or(r.ended_at).unwrap_or(now);
        let ran_ms = r.started_at.map(|s| (r.ended_at.unwrap_or(now) - s).max(0));
        Run { run: r.clone(), queued_ms: (granted_or - r.asked_at).max(0), ran_ms }
    }
}

/// One landing: `done` asked, each check that its branch merges while it
/// waited its turn, each fast gate on its merge (asked at its turn in the
/// line), and how it ended.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
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

/// A wait or a park: why, from when, until when it was declared or its
/// engine resets (none: until what it waits for is met), and when it ended.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Span {
    pub why: String,
    pub at: i64,
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
    let shown =
        |lands: bool| runs.iter().filter(move |r| r.lands == lands && (r.started_at.is_some() || r.ended_at.is_none()));
    let mut s =
        Steps { heavy: shown(false).map(|r| Run::of(r, now)).collect(), messages: told(outbox), ..Steps::default() };
    let span = |why: String, at: i64, until: Option<i64>| Span { why, at, until, ended_at: None };
    for (at, e) in events {
        let at = *at;
        if let Some(st) = e.standing() {
            if let Some(l) = s.landings.last_mut().filter(|l| l.ended_at.is_none() && st.status != Status::Landing) {
                l.ended_at = Some(at);
                l.outcome = Some(word(st.status, e));
            }
            close(&mut s.waits, at);
        }
        let landing = s.landings.last_mut();
        match e {
            WorkerEvent::LandingAsked { at } => s.landings.push(Landing { asked_at: *at, ..Landing::default() }),
            WorkerEvent::MergeChecked { tip } => {
                landing.into_iter().for_each(|l| l.checked.push(Checked { at, tip: tip.clone() }))
            }
            WorkerEvent::LandingRed { at: red_at, tip, moved, kind, files, .. } => {
                let told = s.messages.iter().find(|m| m.from == Sender::Factorium && m.at >= *red_at).cloned();
                if let Some(l) = landing {
                    l.red = Some(Red { kind: *kind, files: files.clone(), tip: tip.clone(), moved: *moved, told });
                }
            }
            WorkerEvent::WorkerLanded { commit, .. } => {
                landing.into_iter().for_each(|l| l.commit = Some(commit.clone()))
            }
            WorkerEvent::WaitDeclared(d) => s.waits.push(span(d.why.clone(), d.at, Some(d.until))),
            WorkerEvent::WaitEnded => close(&mut s.waits, at),
            WorkerEvent::WorkerParked => s.parks.push(span("(no reason given)".into(), at, None)),
            WorkerEvent::ParkedBehind { why } => s.parks.push(span(why.clone(), at, None)),
            WorkerEvent::WorkerLimited(l) => {
                s.parks.push(span(format!("its engine is at its limit: {}", l.said), l.at, Some(l.until)))
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
            close(&mut s.parks, at);
        }
    }
    // Each landing's gates: those asked from its `done` to its end.
    for l in &mut s.landings {
        let within = |r: &&RunRow| r.asked_at >= l.asked_at && l.ended_at.is_none_or(|e| r.asked_at <= e);
        l.gates = shown(true).filter(within).map(|r| Run::of(r, now)).collect();
    }
    s
}

/// Ends each of `spans` still open, at `at`.
fn close(spans: &mut [Span], at: i64) {
    spans.iter_mut().filter(|s| s.ended_at.is_none()).for_each(|s| s.ended_at = Some(at));
}

/// How a landing ended, by the event that ended it.
fn word(status: Status, e: &WorkerEvent) -> String {
    match e {
        WorkerEvent::LandingRed { .. } => "red".into(),
        WorkerEvent::WorkerLanded { .. } => "landed".into(),
        _ => crate::named(status),
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
