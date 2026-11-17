//! Hot workers: a worker whose processes take more than their share — of
//! the CPU, of memory, of process count — for longer than `hot_for`.

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// What one worker's processes held at one reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Load {
    /// Percent of the whole CPU (all cores).
    pub cpu: f64,
    pub rss_kib: u64,
    pub procs: u64,
}

/// What the load is over, if anything.
pub fn over(load: &Load, s: &Settings) -> Option<String> {
    if load.cpu > s.get("worker_cpu_share") as f64 {
        return Some(format!("cpu {:.0}% over its share of {}%", load.cpu, s.get("worker_cpu_share")));
    }
    if load.rss_kib > s.get("worker_memory_gib") * (1 << 20) {
        return Some(format!("memory {} GiB over {} GiB", load.rss_kib >> 20, s.get("worker_memory_gib")));
    }
    if load.procs > s.get("worker_procs_max") {
        return Some(format!("{} processes, over {}", load.procs, s.get("worker_procs_max")));
    }
    None
}

/// Whether a worker over since `since` (ms) is hot at `now`.
pub fn hot(since: Option<i64>, now: i64, s: &Settings) -> bool {
    since.is_some_and(|t| now - t >= s.ms("hot_for"))
}
