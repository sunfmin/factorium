//! Admission: whether one more thing may start now, from the host's reading
//! alone. No concurrency number: the reading says whether there is room.

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// What the host read, as the rules see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub cpu_busy: u64,
    pub memory_free: u64,
    pub disk_free: u64,
    /// The share of the CPU the workers' processes hold, in percent.
    pub workers_cpu: u64,
}

const GIB: u64 = 1 << 30;

/// Whether the host has room for one more start; else why not.
pub fn room(r: &Reading, s: &Settings) -> Result<(), String> {
    if r.disk_free < s.get("disk_floor_gib") * GIB {
        return Err(format!(
            "disk: {} GiB free, under the floor of {} GiB",
            r.disk_free / GIB,
            s.get("disk_floor_gib")
        ));
    }
    if r.cpu_busy > s.get("cpu_busy_max") {
        return Err(format!("cpu: {}% busy, over {}%", r.cpu_busy, s.get("cpu_busy_max")));
    }
    if r.memory_free < s.get("memory_free_min") {
        return Err(format!("memory: {}% free, under {}%", r.memory_free, s.get("memory_free_min")));
    }
    Ok(())
}

/// Whether one more worker may start: room, and the workers under their
/// share of the CPU.
pub fn admit(r: &Reading, s: &Settings) -> Result<(), String> {
    room(r, s)?;
    if r.workers_cpu > s.get("workers_cpu_share") {
        return Err(format!("workers: {}% of the CPU, over their {}%", r.workers_cpu, s.get("workers_cpu_share")));
    }
    Ok(())
}

/// Whether the disk is under its floor: the time to reclaim `target/`.
pub fn disk_short(r: &Reading, s: &Settings) -> bool {
    r.disk_free < s.get("disk_floor_gib") * GIB
}
