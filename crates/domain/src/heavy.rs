//! The heavy-command queue of resources, whose rules SPEC.md §1.9 holds:
//! its stream, its entries and what each asks, each resource's line and
//! why one waits, and the line jobs' names. The start rule is neutral:
//! [`crate::neutral::queue`].

use std::collections::BTreeMap;

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

    /// Whether `what` names a gate, a line's or a worker's (`fast gate of web#1`, its
    /// `of` the issue).
    pub fn is_gate(what: &str) -> bool {
        [Kind::Fast, Kind::Slow].iter().any(|k| what.starts_with(&LineJob::Gate(*k).of("")))
    }
}

/// An entry: what its asker asked, and where it stands in the line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub ask: Ask,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    /// The session the running step runs in, once its waiter noted it:
    /// every process the step started, wherever it moved, so ending the
    /// entry from another process ends them all.
    #[serde(default)]
    pub session: Option<u32>,
}

/// A daily window, in the host's local time as it was asked: the minutes
/// of the day it opens and closes at (it may pass midnight), and the host's
/// offset from UTC then, minutes east.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub from: u32,
    pub to: u32,
    pub offset: i32,
}

const DAY_MIN: i64 = 24 * 60;

impl Window {
    /// How long it has been open at `now`, ms; none: it is closed.
    pub fn open_for(&self, now: i64) -> Option<i64> {
        let day = DAY_MIN * 60_000;
        let since = (now + i64::from(self.offset - self.from as i32) * 60_000).rem_euclid(day);
        let len = (i64::from(self.to) - i64::from(self.from)).rem_euclid(DAY_MIN) * 60_000;
        (since < len).then_some(since)
    }

    /// As the owner names it: `22:00-07:00`.
    pub fn shown(&self) -> String {
        let hm = |m: u32| format!("{:02}:{:02}", m / 60, m % 60);
        format!("{}-{}", hm(self.from), hm(self.to))
    }
}

impl Entry {
    /// Whether it is a worker's hold across commands.
    pub fn is_hold(&self) -> bool {
        self.ask.hold_for.is_some()
    }

    /// The resources it asks for: [`HOST`] when it names none.
    pub fn asks(&self) -> Vec<&str> {
        if self.ask.resources.is_empty() { vec![HOST] } else { self.ask.resources.iter().map(String::as_str).collect() }
    }

    /// Whether it asks for resource `name`.
    pub fn asks_for(&self, name: &str) -> bool {
        self.asks().contains(&name)
    }

    /// Whether it waits outside the window it named at `now`.
    pub fn outside(&self, now: i64) -> bool {
        self.started_at.is_none() && self.ask.window.is_some_and(|w| w.open_for(now).is_none())
    }

    /// Until when it declared it holds, ms, once started.
    pub fn until(&self) -> Option<i64> {
        Some(self.started_at?.saturating_add(self.ask.hold_for?))
    }
}

/// The resource every heavy command asked for before resources were named:
/// the host's load, by the `room` rule.
pub const HOST: &str = "host";

/// A resource's capacity rule: what varies from one kind of resource to
/// another, and nothing else does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// As many as the host's readings allow, one start per settled reading.
    Room,
    /// At most this many holders.
    Slots(u64),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Queue {
    pub entries: Vec<Entry>,
    /// When the last one asking for [`HOST`] started: one start per reading.
    pub last_start: i64,
    /// The rules of the daemon that runs: older ones may not start.
    #[serde(default)]
    pub rules: String,
    /// The places of entries that left unstarted, kept for their askers.
    #[serde(default)]
    pub left: Vec<Left>,
    /// The slots of each named resource, as the host's settings declared it
    /// when an entry last asked for it.
    #[serde(default)]
    pub slots: BTreeMap<String, u64>,
    /// The named resources given back because their holder died or its worker ended.
    #[serde(default)]
    pub given_back: Vec<GivenBack>,
}

/// A named resource given back by factorium, not by its holder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GivenBack {
    pub resource: String,
    pub entry: Uuid,
    pub what: String,
    pub worker: Option<Uuid>,
    /// Why: its waiter died, or its worker ended.
    pub why: String,
    /// When, ms.
    pub at: i64,
}

/// A resource as the queue holds it: its rule, its holders and those
/// waiting for it, oldest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    pub name: String,
    pub rule: Rule,
    pub holders: Vec<Holding>,
    pub waiting: Vec<Holding>,
}

/// An entry holding a resource, or waiting for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Holding {
    pub entry: Uuid,
    pub what: String,
    pub worker: Option<Uuid>,
    /// A worker's hold across commands.
    pub hold: bool,
    /// Since when it holds (its start) or waits (its place), ms.
    pub since: i64,
    /// Until when it declared it holds, ms.
    pub until: Option<i64>,
    /// How long it declared it holds, ms: none, a command, read as `hold_default`.
    #[serde(default)]
    pub declared: Option<i64>,
    /// The daily window it starts in, `22:00-07:00`.
    #[serde(default)]
    pub window: Option<String>,
    /// Waiting, why it is not next: none, it is.
    #[serde(default)]
    pub why: Option<String>,
}

impl From<&Entry> for Holding {
    fn from(e: &Entry) -> Holding {
        let (entry, what, worker, hold, until) = (e.ask.id, e.ask.what.clone(), e.ask.worker, e.is_hold(), e.until());
        let (declared, window, why) = (e.ask.hold_for, e.ask.window.map(|w| w.shown()), None);
        Holding { entry, what, worker, hold, since: e.started_at.unwrap_or(e.queued_at), until, declared, window, why }
    }
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
        self.ask.id
    }

    fn what(&self) -> &str {
        &self.ask.what
    }

    fn queued_at(&self) -> i64 {
        self.queued_at
    }

    fn started_at(&self) -> Option<i64> {
        self.started_at
    }

    // A line move past its bound goes ahead of workers' and programs' entries.
    fn yields(&self) -> bool {
        self.ask.worker.is_some() || self.ask.program
    }

    fn bound_at(&self) -> Option<i64> {
        self.ask.bound_at
    }

    fn declared(&self) -> Option<i64> {
        self.ask.hold_for
    }

    // One with a window joins the line as its window opens.
    fn joined(&self, now: i64) -> i64 {
        let opened = self.ask.window.and_then(|w| w.open_for(now)).map(|open| now - open);
        opened.map_or(self.queued_at, |o| o.max(self.queued_at))
    }
}

impl Queue {
    pub fn running(&self) -> impl Iterator<Item = &Entry> {
        queue::running(&self.entries)
    }

    pub fn waiting(&self) -> impl Iterator<Item = &Entry> {
        queue::waiting(&self.entries)
    }

    /// The capacity rule of resource `name`: [`HOST`]'s is room, a named
    /// one's its slots (one, if no ask declared them).
    pub fn rule(&self, name: &str) -> Rule {
        if name == HOST { Rule::Room } else { Rule::Slots(self.slots.get(name).copied().unwrap_or(1)) }
    }

    /// Whether `e` asks for `name` inside its worker's own hold of it: the
    /// hold holds it, so the command takes no slot of its own.
    fn covered(&self, e: &Entry, name: &str) -> bool {
        !e.is_hold()
            && e.ask.worker.is_some()
            && self.running().any(|h| h.is_hold() && h.ask.worker == e.ask.worker && h.asks_for(name))
    }

    /// The running entries holding `name`, but commands inside their worker's hold of it.
    pub fn holders(&self, name: &str) -> impl Iterator<Item = &Entry> {
        self.running().filter(move |e| e.asks_for(name) && !self.covered(e, name))
    }

    /// The resource the waiting `e` asks for whose slots are all held, if
    /// one is: it waits for that one, and is passed over in every queue, so
    /// it never holds one resource while it waits for another.
    pub fn full_for<'e>(&self, e: &'e Entry) -> Option<&'e str> {
        let full = |r: &&str| match self.rule(r) {
            Rule::Slots(n) => !self.covered(e, r) && self.holders(r).count() as u64 >= n,
            Rule::Room => false,
        };
        e.started_at.is_none().then(|| e.asks().into_iter().find(full)).flatten()
    }

    /// Why the waiting `e` is passed over in every queue at `now`, if it
    /// is: it waits under replaced rules (factorium#241), for a full
    /// resource, or for its window.
    fn passed_over(&self, e: &Entry, now: i64) -> Option<String> {
        if self.stale(e) {
            return Some(format!("it asks under the rules of {}, which {} replaced", e.ask.rules, self.rules));
        }
        if let Some(name) = self.full_for(e) {
            let by: Vec<String> =
                self.holders(name).map(|h| format!("`{}` (entry {})", h.ask.what, h.ask.id)).collect();
            return Some(format!("waits for {name}, held by {}", by.join(", ")));
        }
        let window = e.ask.window.filter(|_| e.outside(now));
        window.map(|w| format!("waits for its window from {}", &w.shown()[..5]))
    }

    /// Resource `name`'s queue at `now`: the entries asking for it but
    /// those passed over, and commands inside their worker's hold of it.
    fn queue_of(&self, name: &str, now: i64) -> Vec<Entry> {
        let of = |e: &&Entry| {
            e.asks_for(name) && !self.covered(e, name) && (e.started_at.is_some() || self.passed_over(e, now).is_none())
        };
        self.entries.iter().filter(of).cloned().collect()
    }

    /// The waiting entry queued first in [`HOST`]'s queue at `now`: those
    /// waiting for a full resource wait for it alone, and hold up nothing else.
    pub fn oldest(&self, now: i64) -> Option<&Entry> {
        let first = queue::oldest(&self.queue_of(HOST, now)).map(|e| e.ask.id);
        first.and_then(|id| self.entries.iter().find(|e| e.ask.id == id))
    }

    /// Each resource declared to it (`slots`), [`HOST`], and each other an
    /// entry asks for: its rule, its holders and those waiting for it at
    /// `now`, in the order they start by `order`, each with why it is not
    /// next; then those passed over, oldest first, and why.
    pub fn resources(&self, now: i64, order: queue::Order) -> Vec<Resource> {
        let mut names: Vec<&str> = self.entries.iter().flat_map(Entry::asks).collect();
        names.extend(self.slots.keys().map(String::as_str).chain([HOST]));
        names.sort();
        names.dedup();
        let resource = |name: &str| {
            // The line as it stands once a slot is free: waiting for this one's holders is waiting in it.
            let waits = |e: &Entry| self.full_for(e).is_some_and(|r| r == name) || self.passed_over(e, now).is_none();
            let (inline, mut over): (Vec<Entry>, Vec<Entry>) =
                self.waiting().filter(|e| e.asks_for(name) && !self.covered(e, name)).cloned().partition(waits);
            let line = queue::line(&inline, now, order);
            let mut waiting: Vec<Holding> = Vec::new();
            for (i, e) in line.iter().enumerate() {
                let why = match i {
                    0 => self.full_for(e).map(|_| self.passed_over(e, now).unwrap_or_default()),
                    _ => Some(behind(line[0], e, now, order)),
                };
                waiting.push(Holding { why, ..Holding::from(*e) });
            }
            over.sort_by_key(|e| (e.queued_at, e.ask.id));
            waiting.extend(over.iter().map(|e| Holding { why: self.passed_over(e, now), ..Holding::from(e) }));
            let holders = self.holders(name).map(Holding::from).collect();
            Resource { name: name.into(), rule: self.rule(name), holders, waiting }
        };
        names.into_iter().map(resource).collect()
    }

    /// When the newest of `checkout`'s line jobs that run now started, if one runs.
    pub fn started(&self, checkout: &str) -> Option<i64> {
        let jobs = [LineJob::Gate(Kind::Fast), LineJob::Gate(Kind::Slow), LineJob::Cut].map(|j| j.of(checkout));
        self.running().filter(|e| jobs.contains(&e.ask.what)).filter_map(|e| e.started_at).max()
    }

    /// Whether the line job `job` of `checkout` runs now.
    pub fn runs(&self, checkout: &str, job: LineJob) -> bool {
        let what = job.of(checkout);
        self.running().any(|e| e.ask.what == what)
    }

    /// Whether `id` may start at `now`: next in each of its resources'
    /// queues by `order`, all at once, by each one's rule — for [`HOST`],
    /// whether the host has room, when the reading was taken, and how long a
    /// start takes to show; for slots, one free. Else why not.
    pub fn may_start(&self, id: Uuid, room: Result<(), String>, at: Reading, now: i64) -> Result<(), String> {
        let Some(e) = self.waiting().find(|e| e.ask.id == id) else { return Err("not waiting".into()) };
        if let Some(why) = self.passed_over(e, now) {
            return Err(why);
        }
        for name in e.asks().into_iter().filter(|r| !self.covered(e, r)) {
            let q = self.queue_of(name, now);
            match self.rule(name) {
                Rule::Room => {
                    let starts = queue::Starts { last: self.last_start, settle: at.settle };
                    queue::may_start(&q, starts, id, room.clone(), at.read_at, now, at.order)?
                }
                Rule::Slots(_) => match queue::next(&q, now, at.order) {
                    Some(n) if n.ask.id == id => {}
                    Some(n) => return Err(format!("waits for {name} {}", behind(n, e, now, at.order))),
                    None => return Err("not waiting".into()),
                },
            }
        }
        Ok(())
    }

    /// Whether `e` waits under rules older than the queue's.
    pub fn stale(&self, e: &Entry) -> bool {
        e.started_at.is_none() && older(&e.ask.rules, &self.rules)
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
        self.entries.iter().any(|e| e.ask.id == id)
    }

    /// Whether the running `id` holds others in [`HOST`]'s queue up past
    /// `bound` and must end. Only a command holding the host is: a named
    /// resource is never cut short.
    pub fn overheld(&self, id: Uuid, now: i64, bound: i64) -> bool {
        self.entries.iter().any(|e| e.ask.id == id && e.asks_for(HOST))
            && queue::overheld(&self.queue_of(HOST, now), id, now, bound)
    }

    /// Why the running `id` must end at `now`, if it must: unqueued, held
    /// past `bound`, or one that `yields` with anything else asking for the host.
    pub fn ends(&self, id: Uuid, now: i64, bound: i64, yields: bool) -> Option<Ends> {
        if !self.holds(id) {
            Some(Ends::Unqueued)
        } else if self.overheld(id, now, bound) {
            Some(Ends::Held)
        } else {
            (yields && self.entries.iter().any(|e| e.ask.id != id && e.asks_for(HOST))).then_some(Ends::GaveWay)
        }
    }
}

/// When a start's reading was taken and how the line is ordered: what
/// [`Queue::may_start`] reads besides the queue.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Reading {
    /// When the host's reading was taken, ms: before the host was read.
    pub read_at: i64,
    /// How long after the last start a reading shows its load, ms.
    pub settle: i64,
    /// How the line is ordered: none, the oldest first.
    #[serde(default)]
    pub order: queue::Order,
}

/// Why `e` waits behind `next`, the first of its line at `now`: `behind
/// `5 min check`, a 5 min hold`.
fn behind(next: &Entry, e: &Entry, now: i64, order: queue::Order) -> String {
    let why = if next.ask.bound_at.is_some_and(|b| now > b) {
        "a line move past its bound".to_string()
    } else if order.starved(next, now) {
        "waiting past the starve bound".to_string()
    } else if next.ask.hold_for.is_none() && e.ask.hold_for.is_none() {
        "queued before it".to_string()
    } else {
        let default = if next.ask.hold_for.is_none() { " (the default)" } else { "" };
        format!("a {} hold{default}", span(order.holds(next)))
    };
    format!("behind `{}`, {why}", next.ask.what)
}

/// `ms` as the owner reads a hold: `5 min`, `90 s`, `2 h 40 min`.
fn span(ms: i64) -> String {
    let min = ms / 60_000;
    match (min, min / 60, min % 60) {
        (0, _, _) => format!("{} s", ms / 1000),
        (m, 0, _) => format!("{m} min"),
        (_, h, 0) => format!("{h} h"),
        (_, h, m) => format!("{h} h {m} min"),
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
        #[serde(flatten)]
        ask: Ask,
        at: i64,
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
        /// Given back by factorium, not by its holder: when, and why. The
        /// named resources it held are kept on record.
        #[serde(default)]
        gone: Option<(i64, String)>,
        /// How its steps ran, if they ran: none for one that left
        /// unstarted, a hold, or one ended before factorium#308.
        #[serde(default)]
        outcome: Option<Outcome>,
    },
    /// A started entry's step runs in session `session`.
    HeavyRuns {
        id: Uuid,
        session: u32,
    },
    /// A daemon under other rules than the queue's started.
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
            HeavyEvent::HeavyQueued { mut ask, at, place } => {
                s.slots.extend(std::mem::take(&mut ask.slots));
                if let Some(e) = s.entries.iter_mut().find(|e| e.ask.id == ask.id) {
                    return e.ask.rules = ask.rules;
                }
                s.left.retain(|l| !l.of(&ask.what, ask.worker, ask.program));
                s.entries.push(Entry { ask, queued_at: place.unwrap_or(at), ..Entry::default() })
            }
            HeavyEvent::HeavyStarted { id, at } => {
                let mut host = false;
                for e in s.entries.iter_mut().filter(|e| e.ask.id == id) {
                    (e.started_at, host) = (Some(at), e.asks_for(HOST));
                }
                if host {
                    s.last_start = at;
                }
            }
            HeavyEvent::HeavyEnded { id, kept_at, gone, .. } => {
                if let Some(at) = kept_at {
                    s.left.retain(|l| at - l.left_at <= LEFT_KEPT_MAX);
                    let left = s.entries.iter().find(|e| e.ask.id == id && e.started_at.is_none() && e.yields());
                    s.left.extend(left.map(|e| Left {
                        what: e.ask.what.clone(),
                        worker: e.ask.worker,
                        program: e.ask.program,
                        queued_at: e.queued_at,
                        left_at: at,
                    }));
                }
                if let Some((at, why)) = gone
                    && let Some(e) = s.entries.iter().find(|e| e.ask.id == id && e.started_at.is_some())
                {
                    s.given_back.retain(|g| at - g.at <= LEFT_KEPT_MAX);
                    let named = e.asks().into_iter().filter(|r| *r != HOST).map(String::from);
                    let (entry, what, worker) = (e.ask.id, e.ask.what.clone(), e.ask.worker);
                    s.given_back.extend(named.map(|resource| GivenBack {
                        resource,
                        entry,
                        what: what.clone(),
                        worker,
                        why: why.clone(),
                        at,
                    }));
                }
                s.entries.retain(|e| e.ask.id != id)
            }
            HeavyEvent::HeavyRuns { id, session } => {
                s.entries.iter_mut().filter(|e| e.ask.id == id).for_each(|e| e.session = Some(session))
            }
            HeavyEvent::HeavyRules { version } => s.rules = version,
        }
    }
}

/// What an entry asks of the queue: as its asker queues it, and as the
/// queue records it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub id: Uuid,
    pub what: String,
    /// The process that waits for it, to forget it when that dies.
    pub pid: u32,
    pub worker: Option<Uuid>,
    /// Queued by a program of the owner's that is not a worker (`heavy
    /// run`): it yields to a line move past its bound as a worker's does.
    #[serde(default)]
    pub program: bool,
    /// When the line move it is for falls due however busy its checkout is
    /// (a hold bound before its bound), ms: past then, it goes ahead of
    /// workers' waiting entries.
    #[serde(default)]
    pub bound_at: Option<i64>,
    /// The rules it asks under: [`RULES`] of the binary that asks. Queued
    /// again under other rules (a binary that execed in its place), it
    /// keeps its place, and takes these; waiting under rules older than the
    /// queue's, it is passed over.
    #[serde(default)]
    pub rules: String,
    /// The resources it asks for, granted all at once: none named is
    /// [`HOST`] alone, as every entry before resources asked.
    #[serde(default)]
    pub resources: Vec<String>,
    /// The slots of each named one, as the host's settings declare them:
    /// the queue keeps them, not the entry.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub slots: BTreeMap<String, u64>,
    /// How long a worker's hold across commands (`worker hold`) declared
    /// it holds, ms: an entry with one is a hold, which once granted
    /// outlives its waiter until `worker release` or its worker ends. Never
    /// cut short before then; past it, a finding.
    #[serde(default)]
    pub hold_for: Option<i64>,
    /// The daily window a hold may start in: outside it, it is passed over.
    #[serde(default)]
    pub window: Option<Window>,
    /// The worker whose landing this gate is: a line's fast gate, which is
    /// no worker's entry (it holds no worker's silence clock), names the
    /// worker it lands so its record finds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lands: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Enqueue {
    #[serde(flatten)]
    pub ask: Ask,
    /// How long the place of its asker's entry that left unstarted is kept
    /// for it, ms: `heavy_keep`.
    #[serde(default)]
    pub keep: i64,
}

command!(EnqueueHeavy, "heavy.queue", Heavy, Enqueue, |_p| queue_id(), |s, p, now| {
    let a = &p.ask;
    // Queued already under these rules: nothing to do.
    let queued = s.entries.iter().any(|e| e.ask.id == a.id && e.ask.rules == a.rules);
    let place = s.place(&a.what, a.worker, a.program, now.0, p.keep);
    crate::only(!queued, HeavyEvent::HeavyQueued { ask: a.clone(), at: now.0, place })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Begin {
    pub id: Uuid,
    /// Whether the host had room, and why not.
    pub room: Result<(), String>,
    #[serde(flatten)]
    pub at: Reading,
    /// The rules it asks under: [`RULES`] of the binary that asks.
    #[serde(default)]
    pub rules: String,
}

command!(StartHeavy, "heavy.start", Heavy, Begin, |_p| queue_id(), |s, p, now| {
    if older(&p.rules, &s.rules) {
        let why = format!("asks under the rules of {}, which {} replaced", p.rules, s.rules);
        return no("stale", why);
    }
    match s.may_start(p.id, p.room.clone(), p.at, now.0) {
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
    /// Given back by factorium, not its holder, and why (its waiter died,
    /// its worker ended): the named resources it held are kept on record.
    #[serde(default)]
    pub gone: Option<String>,
    /// How its steps ran, if they ran.
    #[serde(default)]
    pub outcome: Option<Outcome>,
}

/// How a heavy command's steps ran: every one exited 0 (`green`), one did
/// not (`red`, a hold bound's end among them), or one ran past its limit
/// (`timed_out`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Green,
    Red,
    TimedOut,
}

command!(EndHeavy, "heavy.end", Heavy, End, |_p| queue_id(), |s, p, now| {
    // Whether its place is kept (unstarted, a worker's or a program's) is evolve's.
    let (kept_at, gone) = (p.keeps.then_some(now.0), p.gone.clone().map(|why| (now.0, why)));
    let ended = HeavyEvent::HeavyEnded { id: p.id, kept_at, gone, outcome: p.outcome };
    crate::only(s.holds(p.id), ended)
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Runs {
    pub id: Uuid,
    pub session: u32,
}

command!(NoteRun, "heavy.runs", Heavy, Runs, |_p| queue_id(), |s, p, _now| {
    let started = s.entries.iter().any(|e| e.ask.id == p.id && e.started_at.is_some());
    crate::only(started, HeavyEvent::HeavyRuns { id: p.id, session: p.session })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rules {
    pub version: String,
}

// The daemon's own, older ones too: a rollback's binary is refused none of its own gates.
command!(NoteRules, "heavy.rules", Heavy, Rules, |_p| queue_id(), |s, p, _now| {
    crate::only(s.rules != p.version, HeavyEvent::HeavyRules { version: p.version.clone() })
});
