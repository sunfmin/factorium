//! Admission as factorium asks it: the host's room (neutral:
//! [`crate::neutral::admission`]), and for a worker, the workers under
//! their share of the CPU.

use serde::{Deserialize, Serialize};

use crate::neutral::admission::{self as neutral, Limits};
use crate::settings::{Setting, Settings};

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

/// Whether one more worker may start: room, and the workers under their
/// share of the CPU.
pub fn admit(r: &Reading, s: &Settings) -> Result<(), String> {
    room(r, s)?;
    if r.workers_cpu > s.get(Setting::WorkersCpuShare) {
        return Err(format!("workers: {}% of the CPU, over their {}%", r.workers_cpu, s.get(Setting::WorkersCpuShare)));
    }
    Ok(())
}

/// Whether the disk is under its floor: the time to reclaim `target/`.
pub fn disk_short(r: &Reading, s: &Settings) -> bool {
    neutral::disk_short(&r.host(), &limits(s))
}
