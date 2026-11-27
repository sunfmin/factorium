//! A live worker's health. Each tick the daemon makes one observation of a
//! worker — what its session shows, what its processes hold — and one
//! command, [`Observe`], records it and answers the verdict: wait for the
//! watch and why, end it on a used-up account, or nothing. The rules (up,
//! hot, hung, silent, quota, limit) and their fixed bounds are ADR 0023's,
//! off the path; their clocks live here. Over and quiet are neutral:
//! [`crate::neutral::hot`].

use factum_core::{Event, Uuid};
use serde::{Deserialize, Serialize};

use crate::neutral::hot::Shares;
use crate::settings::{Setting, Settings, Time};
use crate::worker::{Limit, Status, Worker, WorkerEvent, Workers};
use crate::{command, no};

pub use crate::neutral::hot::{Load, quiet};

/// How long a session's harness has to come up before its worker goes to
/// the watch.
const UP_BOUND_MS: i64 = 5 * 60 * 1000;

/// How long a used-up account is left alone: its screen gives no time.
const QUOTA_REST_MS: i64 = 60 * 60 * 1000;

/// One worker's share of the host, from the settings.
fn shares(s: &Settings) -> Shares {
    Shares {
        cpu: s.get(Setting::WorkerCpuShare),
        memory_gib: s.get(Setting::WorkerMemoryGib),
        procs_max: s.get(Setting::WorkerProcsMax),
    }
}

/// What a session is doing, seen from outside.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Sight {
    /// Its terminal is gone.
    Gone,
    /// Its account is used up: the line that says so.
    Quota(String),
    /// Its engine is at its limit until then (ms): the words that say so,
    /// and the window of its account it spent, as stablehand names it
    /// (`None`: the whole account).
    Limit { said: String, until: i64, window: Option<String> },
    /// Its harness stopped: the line that says so.
    Stopped(String),
    /// Its harness is not up in its terminal (not yet, or no longer): why.
    Down(String),
    /// A turn is open: the hash of its screen.
    Open(u64),
    /// Its turn closed at this time, in ms; `None`: no session file says.
    Closed(Option<i64>),
}

/// One look at one live worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub worker: Uuid,
    pub sight: Sight,
    pub load: Load,
    /// Its processes sit next to idle.
    pub quiet: bool,
    /// A heavy entry of its own waits in the queue, or runs alive within
    /// the hold bound.
    #[serde(default)]
    pub heavy: bool,
    /// Its issue waits for something unmet: an open issue, a cut, a time.
    #[serde(default)]
    pub blocked: bool,
    /// Where the evidence of its processes goes, should it be hot.
    pub evidence: String,
    pub settings: Settings,
}

// Records one observation of a worker and decides on it: its clocks
// (over its share, quiet in an open turn) start or stop, and a rule
// past its bound sends it to the watch or ends it.
command!(Observe, "worker.observe", Workers, Observation, |p| p.worker, |s, p, now| {
    if s.started.is_none() {
        return no("no_worker", "no such worker");
    }
    Ok(match s.status {
        Status::Ended => Vec::new(),
        Status::Waiting if s.parked => Vec::new(),
        Status::Waiting => spent(s, &p.sight, now.0).unwrap_or_default(),
        _ => judge(s, p, now.0),
    })
});

/// What `sight` makes of worker `w` at `now` if it says its engine is
/// spent: its account used up, or at its limit (landing, it is left to its
/// landing); `None` if it says neither.
fn spent(w: &Worker, sight: &Sight, now: i64) -> Option<Vec<WorkerEvent>> {
    Some(match sight {
        Sight::Quota(said) => vec![WorkerEvent::UsedUp { said: said.clone(), until: now + QUOTA_REST_MS }],
        Sight::Limit { .. } if matches!(w.status, Status::Landing | Status::Finishing) => Vec::new(),
        Sight::Limit { said, until, .. } => {
            vec![WorkerEvent::WorkerLimited(Limit { said: said.clone(), until: *until, at: now })]
        }
        _ => return None,
    })
}

/// What `o` makes of worker `w` at `now`.
fn judge(w: &Worker, o: &Observation, now: i64) -> Vec<WorkerEvent> {
    if let Some(events) = spent(w, &o.sight, now) {
        return events;
    }
    let s = &o.settings;
    let wait = |why: String| vec![WorkerEvent::WorkerWaits { why, at: now }];
    let started = w.started.as_ref().map_or(now, |st| st.at);
    let screen = match &o.sight {
        Sight::Gone => return wait("its session's terminal is gone".into()),
        Sight::Quota(_) | Sight::Limit { .. } => return Vec::new(),
        Sight::Stopped(said) => return wait(format!("its harness stopped: {said}")),
        Sight::Down(why) if now - started > UP_BOUND_MS => {
            let after = (now - started) / 60_000;
            return wait(format!("its harness is not up {after} min after it started: {why}"));
        }
        Sight::Down(_) => return Vec::new(),
        Sight::Open(h) => Some(*h),
        Sight::Closed(_) => None,
    };
    let mut events = Vec::new();
    // Its own heavy entry in flight, or its issue blocked: the silence clock waits for it.
    let held = o.heavy || o.blocked;
    let freed_at = match (held, w.heavy) {
        (true, false) => {
            events.push(WorkerEvent::HeavyHeld);
            w.freed_at
        }
        (false, true) => {
            events.push(WorkerEvent::HeavyFreed { at: now });
            Some(now)
        }
        _ => w.freed_at,
    };
    // Hot: over its share for `hot_for`.
    let over = o.load.over(&shares(s));
    let over_since = match (&over, w.over_since) {
        (Some(_), None) => {
            events.push(WorkerEvent::WentOver { at: now });
            Some(now)
        }
        (None, Some(_)) => {
            events.push(WorkerEvent::CameUnder);
            None
        }
        (_, since) => since,
    };
    if let (Some(what), Some(t)) = (&over, over_since)
        && now - t >= s.ms(Time::HotFor)
    {
        events.push(WorkerEvent::WentHot { what: what.clone(), evidence: o.evidence.clone() });
        events.extend(wait(format!("hot: {what}; evidence in {}", o.evidence)));
        return events;
    }
    // Hung: quiet in an open turn on one screen past the silent bound.
    let screen = screen.filter(|_| o.quiet && w.status == Status::Working);
    let quiet_since = match (screen, w.quiet) {
        (Some(h), Some((was, at))) if h == was => Some(at),
        (Some(h), _) => {
            events.push(WorkerEvent::WentQuiet { screen: h, at: now });
            Some(now)
        }
        (None, Some(_)) => {
            events.push(WorkerEvent::Stirred);
            None
        }
        (None, None) => None,
    };
    if let Some(t) = quiet_since.filter(|t| now - t > s.ms(Time::SilentBound)) {
        let mins = (now - t) / 60_000;
        events.extend(wait(format!(
            "hung for {mins} min: its turn is open, its processes use next to no CPU and its screen has not changed"
        )));
        return events;
    }
    // Silent: its turn closed, no done and nothing holding it, past the bound.
    if let Sight::Closed(at) = o.sight
        && !held
    {
        let idle_since = at.map_or(w.since, |t| t.max(w.since));
        let idle_since = freed_at.map_or(idle_since, |t| t.max(idle_since));
        let declared = w.declared.as_ref();
        if w.status == Status::Working
            && now - idle_since > s.ms(Time::SilentBound)
            && declared.is_none_or(|d| now > d.until)
        {
            let mins = (now - idle_since) / 60_000;
            let past = declared.map_or(String::new(), |d| {
                format!(", past the {} min wait it declared for: {}", (d.until - d.at) / 60_000, d.why)
            });
            events.extend(wait(format!("silent for {mins} min without `factorium worker done`{past}")));
        }
    }
    events
}

/// What an observation decided, for the app to carry out.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Nothing to do.
    Fine,
    /// It waits for the watch, and why; hot, its processes' evidence is
    /// written where the observation said.
    Waits { why: String, hot: Option<String> },
    /// Its account is used up, as its screen said: the account rests until
    /// then (ms), and the worker ends.
    UsedUp { said: String, until: i64 },
    /// Its engine is at its limit, as its screen said, until then (ms): it
    /// waits, parked until the reset, and its account or harness starts no
    /// worker before it.
    Limited { said: String, until: i64 },
}

impl Verdict {
    /// The verdict the events `Observe` recorded carry.
    pub fn of(events: &[Event]) -> Verdict {
        let mut hot = None;
        for e in events {
            match crate::tracker::decode(e) {
                Ok(WorkerEvent::WentHot { evidence, .. }) => hot = Some(evidence),
                Ok(WorkerEvent::UsedUp { said, until }) => return Verdict::UsedUp { said, until },
                Ok(WorkerEvent::WorkerLimited(l)) => return Verdict::Limited { said: l.said, until: l.until },
                Ok(WorkerEvent::WorkerWaits { why, .. }) => return Verdict::Waits { why, hot },
                _ => {}
            }
        }
        Verdict::Fine
    }
}
