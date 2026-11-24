//! The host's load, read once: the host's reading, its processes and the
//! heavy queue, split by one rule — what the queue granted is the queue's to
//! bound; the rest counts against the workers' shares and admission.

use std::collections::BTreeSet;

use factorium_domain::admission::Reading;
use factorium_domain::health;
use factorium_domain::heavy::{Entry, Heavy, Queue, queue_id};
use factorium_host::{Host, Proc};
use factum::Uuid;

use crate::{Answer, App};

/// One reading of the host and the heavy queue.
pub(crate) struct Load {
    host: factorium_host::Reading,
    procs: Vec<Proc>,
    /// The queue as read.
    pub queue: Queue,
    /// The pids of the queue's entries alive when it was read.
    alive: BTreeSet<u32>,
    cores: usize,
}

/// A worker's part of a [`Load`]: what its processes outside its heavy slot
/// hold, whether they are quiet, and whether its own heavy entry keeps it
/// from being silent.
pub(crate) struct Share {
    pub load: health::Load,
    pub quiet: bool,
    pub heavy: bool,
}

impl App {
    /// The host and the heavy queue, read now.
    pub(crate) async fn load(&self) -> Answer<Load> {
        Ok(Load::read(&*self.host, self.state::<Heavy>(queue_id()).await?))
    }
}

impl Load {
    /// The host read now, beside `queue` as its caller read it.
    pub(crate) fn read(host: &dyn Host, queue: Queue) -> Load {
        let alive = queue.entries.iter().map(|e| e.pid).filter(|&pid| host.alive(pid)).collect();
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        Load { host: host.reading(), procs: host.processes(), queue, alive, cores }
    }

    /// Whether `pid`, one of the queue's entries' waiters, was alive as read.
    pub(crate) fn alive(&self, pid: u32) -> bool {
        self.alive.contains(&pid)
    }

    /// The queue's reading: nothing left out, as it bounds its own slots.
    pub(crate) fn room(&self) -> Reading {
        whole(&self.host)
    }

    /// The queue's reading as a waiter asks for its turn: the host's alone.
    /// It leaves no process out, so it lists none: each waiter asked twice a
    /// second, and the listing (`ps -E` of every process) and a `kill` per
    /// entry cost each 0.13 of a core (factorium#194).
    pub(crate) fn queue(host: &dyn Host) -> Reading {
        whole(&host.reading())
    }

    /// Admission's reading: every process the queue granted left out.
    pub(crate) fn admission(&self) -> Reading {
        // Only what holds the host is the queue's to bound.
        self.outside(&self.granted(|e| e.asks_for(factorium_domain::heavy::HOST)))
    }

    /// `worker`'s part at `now`: an entry of its own waiting, or started
    /// within `hold` ms and alive, keeps it from being silent: one waiting
    /// for a resource is waiting, not silent.
    pub(crate) fn worker(&self, worker: Uuid, now: i64, hold: i64) -> Share {
        let mine = |e: &Entry| e.worker == Some(worker);
        let (held, tag) = (self.granted(mine), Some(worker.to_string()));
        let procs: Vec<_> = self.procs.iter().filter(|p| p.worker == tag && !held.contains(&p.pid)).collect();
        let cpu = procs.iter().map(|p| p.cpu).sum::<f64>();
        let load = health::Load::of(cpu, procs.iter().map(|p| p.rss_kib).sum(), procs.len() as u64, self.cores);
        let live = |e: &Entry| self.alive.contains(&e.pid) && e.started_at.is_none_or(|t| now - t <= hold);
        Share { load, quiet: health::quiet(cpu), heavy: self.queue.entries.iter().any(|e| mine(e) && live(e)) }
    }

    /// The host's reading with the CPU of the processes in `granted` left out.
    fn outside(&self, granted: &BTreeSet<u32>) -> Reading {
        let (inside, outside): (Vec<&Proc>, Vec<_>) = self.procs.iter().partition(|p| granted.contains(&p.pid));
        let pct = |cpu: f64| (cpu / self.cores as f64).round() as u64;
        let workers = pct(outside.iter().filter(|p| p.worker.is_some()).map(|p| p.cpu).sum());
        let (r, slots) = (&self.host, pct(inside.iter().map(|p| p.cpu).sum()));
        let (memory_free, disk_free) = (r.memory_free, r.disk_free);
        Reading { cpu_busy: r.cpu_busy.saturating_sub(slots), memory_free, disk_free, workers_cpu: workers }
    }

    /// The pid of each started entry `of` picks, and every process under it.
    fn granted(&self, of: impl Fn(&Entry) -> bool) -> BTreeSet<u32> {
        let mut roots: Vec<u32> =
            self.queue.entries.iter().filter(|e| e.started_at.is_some() && of(e)).map(|e| e.pid).collect();
        let mut all = BTreeSet::new();
        while let Some(pid) = roots.pop() {
            if all.insert(pid) {
                roots.extend(self.procs.iter().filter(|p| p.ppid == pid).map(|p| p.pid));
            }
        }
        all
    }
}

/// `r` with nothing left out: the queue's reading.
fn whole(r: &factorium_host::Reading) -> Reading {
    Reading { cpu_busy: r.cpu_busy, memory_free: r.memory_free, disk_free: r.disk_free, workers_cpu: 0 }
}
