//! The hot rule: whether one session's processes hold more of the host than
//! their share, and whether they sit idle.

use serde::{Deserialize, Serialize};

/// What one session's processes held at one reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Load {
    /// Percent of the whole CPU (all cores).
    pub cpu: f64,
    pub rss_kib: u64,
    pub procs: u64,
}

/// One session's share of the host: past any of them, it is over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shares {
    /// Percent of the whole CPU.
    pub cpu: u64,
    pub memory_gib: u64,
    pub procs_max: u64,
}

/// Below this share of one core, a session's processes together use next
/// to no CPU: a command blocked, not working.
const QUIET_CPU: f64 = 2.0;

impl Load {
    /// What processes holding `core_percent` of one core, `rss_kib` and
    /// `procs` hold on a host of `cores`.
    pub fn of(core_percent: f64, rss_kib: u64, procs: u64, cores: usize) -> Load {
        Load { cpu: core_percent / cores as f64, rss_kib, procs }
    }

    /// What the load is over, if anything.
    pub fn over(&self, s: &Shares) -> Option<String> {
        if self.cpu > s.cpu as f64 {
            return Some(format!("cpu {:.0}% over its share of {}%", self.cpu, s.cpu));
        }
        if self.rss_kib > s.memory_gib * (1 << 20) {
            return Some(format!("memory {} GiB over {} GiB", self.rss_kib >> 20, s.memory_gib));
        }
        if self.procs > s.procs_max {
            return Some(format!("{} processes, over {}", self.procs, s.procs_max));
        }
        None
    }
}

/// Whether processes using `core_percent` of one core between them sit idle.
pub fn quiet(core_percent: f64) -> bool {
    core_percent < QUIET_CPU
}
