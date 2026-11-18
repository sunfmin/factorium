//! Idle work: what a checkout declares under `[idle]` to run only while the
//! host is idle, at most once per `every` — a check too slow for any gate.
//! It runs as a heavy command on the base's tip, never ahead of anything
//! else the queue holds, and yields to whatever queues behind it. Each run
//! that ends is recorded; a red one is a finding, as a red promotion is.

use factum_core::{Aggregate, Uuid};
use serde::{Deserialize, Serialize};

use crate::heavy::Queue;
use crate::worker::Status;
use crate::{command, id};

/// The id of the stream of `checkout`'s idle work.
pub fn idle_id(checkout: &str) -> Uuid {
    id(&format!("idle:{checkout}"))
}

/// What the queue calls the idle work of `checkout`.
pub fn job(checkout: &str) -> String {
    format!("idle work of {checkout}")
}

/// One run that ended: when it started, on which base tip, and how it went.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub tip: String,
    pub at: i64,
    pub green: bool,
    pub took_ms: u64,
    /// The last lines it printed, and why it ended.
    pub tail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Idle {
    /// The last run that ended green or red; one that yielded is not one.
    pub last: Option<Run>,
}

impl Idle {
    /// Whether a run is due at `now`, `every` ms after the last one started.
    pub fn due(&self, every_ms: i64, now: i64) -> bool {
        self.last.as_ref().is_none_or(|r| now - r.at >= every_ms)
    }

    /// The last run, if it was red.
    pub fn red(&self) -> Option<&Run> {
        self.last.as_ref().filter(|r| !r.green)
    }
}

/// Whether the host is idle, or why not: no live worker works (each waits
/// for the watch), the heavy queue holds nothing, and the host has room.
pub fn host_idle(workers: &[Status], queue: &Queue, room: Result<(), String>) -> Result<(), String> {
    if workers.iter().any(|s| *s != Status::Waiting && *s != Status::Ended) {
        return Err("a worker works".into());
    }
    if let Some(e) = queue.entries.first() {
        return Err(format!("the heavy queue holds `{}`", e.what));
    }
    room
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IdleEvent {
    IdleRan { run: Run },
}

pub struct Idles;

impl Aggregate for Idles {
    const NAME: &'static str = "idle";
    type State = Idle;
    type Event = IdleEvent;

    fn evolve(s: &mut Idle, IdleEvent::IdleRan { run }: IdleEvent) {
        s.last = Some(run);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ran {
    pub checkout: String,
    pub run: Run,
}

command!(RecordIdle, "idle.ran", Idles, Ran, |p| idle_id(&p.checkout), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![IdleEvent::IdleRan { run: p.run.clone() }])
});
