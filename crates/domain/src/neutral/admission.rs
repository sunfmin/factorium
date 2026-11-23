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
    short(r, l).into_iter().next().map_or(Ok(()), Err)
}

/// Each way the host lacks room for one more start, the disk first: each
/// reading beside its limit, in the limit's unit.
pub fn short(r: &Reading, l: &Limits) -> Vec<String> {
    let mut why = Vec::new();
    if disk_short(r, l) {
        // In tenths cut, not rounded: a byte under the floor never reads as the floor.
        let tenths = (r.disk_free as u128 * 10 / GIB as u128) as u64;
        let free = format!("{}.{}", tenths / 10, tenths % 10);
        why.push(format!("disk: {free} GiB free, under the floor of {} GiB", l.disk_floor_gib));
    }
    if r.cpu_busy > l.cpu_busy_max {
        why.push(format!("cpu: {}% busy, over {}%", r.cpu_busy, l.cpu_busy_max));
    }
    if r.memory_free < l.memory_free_min {
        why.push(format!("memory: {}% free, under {}%", r.memory_free, l.memory_free_min));
    }
    why
}

/// Whether the disk is under its floor: the time to reclaim build output.
pub fn disk_short(r: &Reading, l: &Limits) -> bool {
    r.disk_free < l.disk_floor_gib * GIB
}
