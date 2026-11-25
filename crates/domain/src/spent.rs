//! Where a set of issues' time went (factorium#309), read from each
//! attempt's steps ([`crate::steps`]) and what it spent: nothing kept of its
//! own. Each attempt's time, from its start to its end (a live one's, to
//! now; one before `--since`, from then), is cut into parts that do not
//! overlap: a moment two steps share is the first's of waiting for a
//! landing, a heavy command running, one queued, a park, a declared wait;
//! one in none of them is the session's own (`rest`). The parts sum to `ms`.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::heavy::LineJob;
use crate::steps::{Run, Steps};
use crate::worker::Tokens;

/// One attempt: its time from `from` to `to`, ms, what its session spent,
/// and its steps.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub from: i64,
    pub to: i64,
    pub tokens: Option<Tokens>,
    pub steps: Steps,
}

/// One issue and the attempts of it that ran in the time asked.
#[derive(Clone, Debug)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub attempts: Vec<Attempt>,
}

/// What `factorium spent` answers.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Spent {
    pub totals: Totals,
    pub split: Split,
    pub reds: Reds,
    pub issues: Vec<Row>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Totals {
    pub issues: usize,
    pub attempts: usize,
    /// The attempts' time, summed.
    pub ms: i64,
    /// Summed over the attempts whose harness says; none: none says.
    pub tokens: Option<Tokens>,
    /// The first attempt's start, and the last landing's end: the wall
    /// clock of the round (none: nothing landed).
    pub first_start: Option<i64>,
    pub last_landing: Option<i64>,
    pub wall_ms: Option<i64>,
}

/// The time, cut by what it went to; its parts sum to the time.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Split {
    /// Heavy commands of its own waiting in the queue, by command
    /// (`gate`, `exclusive`, `hold`).
    pub heavy_queued: Parts,
    /// The same, running.
    pub heavy_ran: Parts,
    /// From `done` to the landing's end.
    pub landing: Landing,
    /// Declared waits (`worker waiting`), by why.
    pub waits: Parts,
    /// Parks, by why.
    pub parks: Parts,
    /// The session's own: the engine and its tools.
    pub rest: i64,
}

/// A part's ms, and its ms by what.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Parts {
    pub ms: i64,
    pub by: BTreeMap<String, i64>,
}

impl Parts {
    fn add(&mut self, what: &str, ms: i64) {
        self.ms += ms;
        *self.by.entry(what.into()).or_default() += ms;
    }
}

/// Waiting for a landing: one that landed (`green`), one that came back
/// red, by its kind (`unknown`: one before factorium#308), and one that
/// ended otherwise or still lands (`other`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Landing {
    pub ms: i64,
    pub green: i64,
    pub red: Parts,
    pub other: i64,
}

/// The reds: how many, by kind, and the files that conflicted, ranked by
/// how many landings they broke.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Reds {
    pub count: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub files: Vec<Broke>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Broke {
    pub file: String,
    pub landings: usize,
}

/// One issue's row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    pub number: u64,
    pub title: String,
    pub attempts: usize,
    pub ms: i64,
    pub tokens: Option<Tokens>,
    pub split: Split,
    pub reds: Reds,
}

/// What took the time from one moment: the order is the precedence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Took {
    Landing(String),
    Ran(&'static str),
    Queued(&'static str),
    Park(String),
    Wait(String),
}

/// Where `issues`' time went.
pub fn spent(issues: &[Issue]) -> Spent {
    let mut all = Spent::default();
    for i in issues {
        let mut row = Row {
            number: i.number,
            title: i.title.clone(),
            attempts: i.attempts.len(),
            ms: 0,
            tokens: None,
            split: Split::default(),
            reds: Reds::default(),
        };
        for a in &i.attempts {
            row.ms += a.to - a.from;
            row.tokens = sum(row.tokens, a.tokens);
            split(a, &mut row.split);
            for l in a.steps.landings.iter().filter(|l| l.ended_at.is_some_and(|e| a.from <= e && e <= a.to)) {
                if let Some(red) = &l.red {
                    let kind = red.kind.map(crate::named).unwrap_or("unknown".into());
                    for r in [&mut row.reds, &mut all.reds] {
                        r.count += 1;
                        *r.by_kind.entry(kind.clone()).or_default() += 1;
                        broke(&mut r.files, &red.files, red.kind == Some(crate::worker::RedKind::NoMerge));
                    }
                }
                if l.outcome.as_deref() == Some("landed") {
                    all.totals.last_landing = all.totals.last_landing.max(l.ended_at);
                }
            }
            let first = all.totals.first_start.get_or_insert(a.from);
            *first = (*first).min(a.from);
        }
        let t = &mut all.totals;
        (t.issues, t.attempts, t.ms, t.tokens) =
            (t.issues + 1, t.attempts + row.attempts, t.ms + row.ms, sum(t.tokens, row.tokens));
        all.split.merge(&row.split);
        all.issues.push(row);
    }
    let t = &mut all.totals;
    t.wall_ms = t.first_start.zip(t.last_landing).map(|(f, l)| l - f);
    for r in std::iter::once(&mut all.reds).chain(all.issues.iter_mut().map(|i| &mut i.reds)) {
        r.files.sort_by(|a, b| b.landings.cmp(&a.landings).then_with(|| a.file.cmp(&b.file)));
    }
    all
}

fn sum(a: Option<Tokens>, b: Option<Tokens>) -> Option<Tokens> {
    match (a, b) {
        (Some(a), Some(b)) => Some(Tokens { input: a.input + b.input, output: a.output + b.output }),
        (a, b) => a.or(b),
    }
}

/// Counts each of `files` once more, a conflict's.
fn broke(ranked: &mut Vec<Broke>, files: &[String], conflict: bool) {
    for f in files.iter().filter(|_| conflict) {
        match ranked.iter_mut().find(|b| &b.file == f) {
            Some(b) => b.landings += 1,
            None => ranked.push(Broke { file: f.clone(), landings: 1 }),
        }
    }
}

impl Split {
    fn merge(&mut self, o: &Split) {
        for (mine, theirs) in [
            (&mut self.heavy_queued, &o.heavy_queued),
            (&mut self.heavy_ran, &o.heavy_ran),
            (&mut self.waits, &o.waits),
            (&mut self.parks, &o.parks),
            (&mut self.landing.red, &o.landing.red),
        ] {
            theirs.by.iter().for_each(|(what, ms)| mine.add(what, *ms));
        }
        let (l, ol) = (&mut self.landing, &o.landing);
        (l.ms, l.green, l.other) = (l.ms + ol.ms, l.green + ol.green, l.other + ol.other);
        self.rest += o.rest;
    }

    fn add(&mut self, took: &Took, ms: i64) {
        match took {
            Took::Landing(outcome) => {
                self.landing.ms += ms;
                match outcome.as_str() {
                    "landed" => self.landing.green += ms,
                    "other" => self.landing.other += ms,
                    kind => self.landing.red.add(kind, ms),
                }
            }
            Took::Ran(what) => self.heavy_ran.add(what, ms),
            Took::Queued(what) => self.heavy_queued.add(what, ms),
            Took::Park(why) => self.parks.add(why, ms),
            Took::Wait(why) => self.waits.add(why, ms),
        }
    }
}

/// What a heavy command is: a hold across commands, a gate, or a command
/// run by hand.
fn command(r: &Run) -> &'static str {
    if r.run.hold {
        "hold"
    } else if LineJob::is_gate(&r.run.what) {
        "gate"
    } else {
        "exclusive"
    }
}

/// Adds attempt `a`'s time, cut into its parts, to `into`.
fn split(a: &Attempt, into: &mut Split) {
    let s = &a.steps;
    let mut spans: Vec<(i64, i64, Took)> = Vec::new();
    for l in &s.landings {
        let outcome = match (l.outcome.as_deref(), &l.red) {
            (Some("landed"), _) => "landed".to_string(),
            (Some("red"), Some(red)) => red.kind.map(crate::named).unwrap_or("unknown".into()),
            _ => "other".into(),
        };
        spans.push((l.asked_at, l.ended_at.unwrap_or(a.to), Took::Landing(outcome)));
    }
    for r in &s.heavy {
        let what = command(r);
        let granted = r.run.started_at.or(r.run.ended_at).unwrap_or(a.to);
        spans.push((r.run.asked_at, granted, Took::Queued(what)));
        if let Some(st) = r.run.started_at {
            spans.push((st, r.run.ended_at.unwrap_or(a.to), Took::Ran(what)));
        }
    }
    for p in &s.parks {
        spans.push((p.at, p.ended_at.unwrap_or(a.to), Took::Park(p.why.clone())));
    }
    for w in &s.waits {
        spans.push((w.at, w.ended_at.unwrap_or(a.to), Took::Wait(w.why.clone())));
    }
    let clip = |t: i64| t.clamp(a.from, a.to);
    let spans: Vec<_> = spans.into_iter().map(|(f, t, k)| (clip(f), clip(t), k)).filter(|(f, t, _)| f < t).collect();
    let mut cuts: Vec<i64> = spans.iter().flat_map(|(f, t, _)| [*f, *t]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut covered = 0;
    for w in cuts.windows(2) {
        let (from, to) = (w[0], w[1]);
        if let Some(took) = spans.iter().filter(|(f, t, _)| *f <= from && to <= *t).map(|(.., k)| k).min() {
            into.add(took, to - from);
            covered += to - from;
        }
    }
    into.rest += (a.to - a.from) - covered;
}
