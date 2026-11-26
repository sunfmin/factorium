//! Admission as factorium asks it: the host's room (neutral:
//! [`crate::neutral::admission`]), and for a worker, the workers under
//! their share of the CPU and the heavy queue keeping up.

use serde::{Deserialize, Serialize};

use crate::heavy::Queue;
use crate::neutral::admission::{self as neutral, Limits};
use crate::settings::{Setting, Settings, Time};

/// What the host read, as the rules see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub cpu_busy: u64,
    pub memory_free: u64,
    pub disk_free: u64,
    /// The share of the CPU the workers' processes hold, in percent.
    pub workers_cpu: u64,
}

impl Reading {
    fn host(&self) -> neutral::Reading {
        neutral::Reading { cpu_busy: self.cpu_busy, memory_free: self.memory_free, disk_free: self.disk_free }
    }
}

fn limits(s: &Settings) -> Limits {
    Limits {
        cpu_busy_max: s.get(Setting::CpuBusyMax),
        memory_free_min: s.get(Setting::MemoryFreeMin),
        disk_floor_gib: s.get(Setting::DiskFloorGib),
    }
}

/// Whether the host has room for one more start; else why not.
pub fn room(r: &Reading, s: &Settings) -> Result<(), String> {
    neutral::room(&r.host(), &limits(s))
}

/// Whether one more worker may start at `now`: room, the workers under
/// their share of the CPU, and no heavy command of `queue` waiting past the
/// queue wait bound. The reading leaves out what runs in started slots, the
/// queue's to bound, and the queue starts beside them only while the host
/// has room, so it holds the host near the busy limit; a command waiting
/// that long is the load the reading cannot show, and another worker's
/// would only wait behind it (factorium#194).
pub fn admit(r: &Reading, queue: &Queue, now: i64, s: &Settings) -> Result<(), String> {
    holds(r, queue, now, s).into_iter().next().map_or(Ok(()), Err)
}

/// Every reason [`admit`] holds a start at `now`, in the order it asks.
pub fn holds(r: &Reading, queue: &Queue, now: i64, s: &Settings) -> Vec<String> {
    let mut why = neutral::short(&r.host(), &limits(s));
    if r.workers_cpu > s.get(Setting::WorkersCpuShare) {
        why.push(format!("workers: {}% of the CPU, over their {}%", r.workers_cpu, s.get(Setting::WorkersCpuShare)));
    }
    let bound = s.ms(Time::QueueWaitBound);
    if let Some(e) = queue.oldest(now).filter(|e| now - e.queued_at > bound) {
        // A command line can run long (a ratchet's `--considered`): its head names it.
        let what: String = e.ask.what.chars().take(60).collect();
        let waited = (now - e.queued_at) / 1000;
        why.push(format!("queue: `{what}` has waited {waited}s for its turn, over {}s", bound / 1000));
    }
    why
}

/// Whether an urgent issue may start now: only the memory and disk floors
/// hold it, never the load (CPU, the workers' share, the queue's wait): one
/// more session on a busy host runs slower, one on a host out of memory or
/// disk fails.
pub fn admit_urgent(r: &Reading, s: &Settings) -> Result<(), String> {
    let floors = Limits { cpu_busy_max: u64::MAX, ..limits(s) };
    neutral::room(&r.host(), &floors)
}

/// Whether the disk is under its floor: the time to reclaim `target/`.
pub fn disk_short(r: &Reading, s: &Settings) -> bool {
    neutral::disk_short(&r.host(), &limits(s))
}
