//! The host's reading: CPU busy share over the last minute, free memory and
//! free disk. Admission and the heavy-command queue decide on nothing else.

use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    /// Percent of the CPU busy, over up to the last minute.
    pub cpu_busy: u64,
    /// Percent of memory free.
    pub memory_free: u64,
    /// Bytes free on the disk factorium keeps its data on.
    pub disk_free: u64,
    /// Bytes on that disk.
    pub disk_total: u64,
}

/// The CPU's tick samples of the last minute.
#[derive(Default)]
pub struct Cpu {
    samples: std::collections::VecDeque<(Instant, u64, u64)>,
}

pub fn read(cpu: &mut Cpu, data: &Path) -> Reading {
    let (disk_free, disk_total) = disk(data).unwrap_or((0, 0));
    Reading { cpu_busy: cpu.busy(), memory_free: memory_free().unwrap_or(100), disk_free, disk_total }
}

impl Cpu {
    fn busy(&mut self) -> u64 {
        let now = Instant::now();
        let Some((busy, total)) = ticks() else { return load_share() };
        self.samples.push_back((now, busy, total));
        while self.samples.len() > 2 && now.duration_since(self.samples[1].0).as_secs() >= 60 {
            self.samples.pop_front();
        }
        if self.samples.len() < 2 || now.duration_since(self.samples[0].0).as_millis() < 250 {
            std::thread::sleep(std::time::Duration::from_millis(300));
            return self.busy();
        }
        let (_, b0, t0) = self.samples[0];
        let (db, dt) = (busy.saturating_sub(b0), total.saturating_sub(t0));
        (db.min(dt) * 100 + dt / 2).checked_div(dt).unwrap_or(0)
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn ticks() -> Option<(u64, u64)> {
    unsafe extern "C" {
        fn mach_host_self() -> libc::mach_port_t;
    }
    let mut info = libc::host_cpu_load_info { cpu_ticks: [0; libc::CPU_STATE_MAX as usize] };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: host_statistics writes at most `count` integers into `info`,
    // which is a host_cpu_load_info of that size.
    let answered = unsafe {
        libc::host_statistics(mach_host_self(), libc::HOST_CPU_LOAD_INFO, (&raw mut info).cast(), &mut count)
    };
    if answered != libc::KERN_SUCCESS {
        return None;
    }
    let tick = |s: libc::c_int| u64::from(info.cpu_ticks[s as usize]);
    let busy = tick(libc::CPU_STATE_USER) + tick(libc::CPU_STATE_SYSTEM) + tick(libc::CPU_STATE_NICE);
    Some((busy, busy + tick(libc::CPU_STATE_IDLE)))
}

#[cfg(not(target_os = "macos"))]
fn ticks() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let f: Vec<u64> = line.split_whitespace().skip(1).filter_map(|w| w.parse().ok()).collect();
    let idle = f.get(3)? + f.get(4).copied().unwrap_or(0);
    let total: u64 = f.iter().take(8).sum();
    Some((total.saturating_sub(idle), total))
}

/// The one-minute load over the cores, as a percent: when ticks are not read.
fn load_share() -> u64 {
    let said = crate::shell::output("sysctl", &["-n", "vm.loadavg"]).unwrap_or_default();
    let load: f64 = said.split_whitespace().find(|w| *w != "{").and_then(|w| w.parse().ok()).unwrap_or(0.0);
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    ((load / cores) * 100.0).round().clamp(0.0, 100.0) as u64
}

fn memory_free() -> Option<u64> {
    if let Ok(info) = std::fs::read_to_string("/proc/meminfo") {
        let field = |name: &str| -> Option<u64> {
            info.lines().find(|l| l.starts_with(name))?.split_whitespace().nth(1)?.parse().ok()
        };
        let (available, total) = (field("MemAvailable:")?, field("MemTotal:")?);
        return (total > 0).then(|| available * 100 / total);
    }
    crate::shell::output("sysctl", &["-n", "kern.memorystatus_level"]).ok()?.trim().parse().ok()
}

#[allow(unsafe_code)]
fn disk(path: &Path) -> Option<(u64, u64)> {
    let mut at = path;
    while !at.exists() {
        at = at.parent()?;
    }
    let c = std::ffi::CString::new(at.as_os_str().as_encoded_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a NUL-terminated path and `s` a statvfs to fill.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_frsize as u64;
    Some((s.f_bavail as u64 * block, s.f_blocks as u64 * block))
}
