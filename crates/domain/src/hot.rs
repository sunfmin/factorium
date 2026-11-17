//! Hot workers: a worker whose processes take more than their share — of
//! the CPU, of memory, of process count — for longer than `hot_for`; and
//! the opposite, processes that sit next to idle.

use serde::{Deserialize, Serialize};

use crate::settings::{Setting, Settings, Time};

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
    if load.cpu > s.get(Setting::WorkerCpuShare) as f64 {
        return Some(format!("cpu {:.0}% over its share of {}%", load.cpu, s.get(Setting::WorkerCpuShare)));
    }
    if load.rss_kib > s.get(Setting::WorkerMemoryGib) * (1 << 20) {
        return Some(format!("memory {} GiB over {} GiB", load.rss_kib >> 20, s.get(Setting::WorkerMemoryGib)));
    }
    if load.procs > s.get(Setting::WorkerProcsMax) {
        return Some(format!("{} processes, over {}", load.procs, s.get(Setting::WorkerProcsMax)));
    }
    None
}

/// Whether a worker over since `since` (ms) is hot at `now`.
pub fn hot(since: Option<i64>, now: i64, s: &Settings) -> bool {
    since.is_some_and(|t| now - t >= s.ms(Time::HotFor))
}

/// Below this share of one core, a worker's processes together use next to
/// no CPU: a command blocked, not working.
const QUIET_CPU: f64 = 2.0;

/// Whether processes using `core_percent` of one core between them sit idle.
pub fn quiet(core_percent: f64) -> bool {
    core_percent < QUIET_CPU
}
