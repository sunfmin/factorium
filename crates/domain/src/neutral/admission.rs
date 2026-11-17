//! Admission: whether the host has room for one more start, from its
//! reading alone. No concurrency number: the reading says whether there is
//! room.

/// What the host read, as the rule sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    /// Percent of the CPU busy.
    pub cpu_busy: u64,
    /// Percent of memory free.
    pub memory_free: u64,
    /// Bytes of disk free.
    pub disk_free: u64,
}

/// Where the host stops having room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub cpu_busy_max: u64,
    pub memory_free_min: u64,
    pub disk_floor_gib: u64,
}

const GIB: u64 = 1 << 30;

/// Whether the host has room for one more start; else why not.
pub fn room(r: &Reading, l: &Limits) -> Result<(), String> {
    if disk_short(r, l) {
        return Err(format!("disk: {} GiB free, under the floor of {} GiB", r.disk_free / GIB, l.disk_floor_gib));
    }
    if r.cpu_busy > l.cpu_busy_max {
        return Err(format!("cpu: {}% busy, over {}%", r.cpu_busy, l.cpu_busy_max));
    }
    if r.memory_free < l.memory_free_min {
        return Err(format!("memory: {}% free, under {}%", r.memory_free, l.memory_free_min));
    }
    Ok(())
}

/// Whether the disk is under its floor: the time to reclaim build output.
pub fn disk_short(r: &Reading, l: &Limits) -> bool {
    r.disk_free < l.disk_floor_gib * GIB
}
