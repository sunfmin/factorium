//! The host's load, read once: one snapshot of the host's reading, its
//! processes and the heavy queue, and the one rule that splits them — what
//! the heavy queue granted is the queue's to bound; everything else counts
//! against the workers' shares and admission. Admission, the queue, each
//! worker's share and doctor ask the same snapshot.

use std::collections::BTreeSet;

use factorium_domain::admission::Reading;
use factorium_domain::health;
use factorium_domain::heavy::{Heavy, Queue, queue_id};
use factorium_host::{Host, Proc};
use factum::Uuid;

use crate::{Answer, App};

/// One reading of the host and the heavy queue.
pub(crate) struct Load {
    host: factorium_host::Reading,
    procs: Vec<Proc>,
    queue: Queue,
    /// The pids of the queue's entries alive when it was read.
    alive: BTreeSet<u32>,
    cores: usize,
}

/// A worker's part of a [`Load`].
pub(crate) struct Share {
    /// What its processes outside its heavy slot hold.
    pub load: health::Load,
    /// Whether they are quiet.
    pub quiet: bool,
    /// Whether its own heavy entry waits, or runs within `hold` of its
    /// start: then it is not silent.
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

    /// The queue as read.
    pub(crate) fn queue(&self) -> &Queue {
        &self.queue
    }

    /// The queue's reading: the host as read, the workers' share of the CPU
    /// added from their processes. Nothing is left out: the queue bounds
    /// its own slots.
    pub(crate) fn room(&self) -> Reading {
        self.outside(&BTreeSet::new())
    }

    /// Admission's reading: [`Load::room`] with the CPU of every process the
    /// queue granted left out, which the queue bounds, not admission.
    pub(crate) fn admission(&self) -> Reading {
        self.outside(&self.granted(|_| true))
    }

    /// `worker`'s part, at `now`, with started heavy entries held past `hold`
    /// ms no longer keeping it from being silent.
    pub(crate) fn worker(&self, worker: Uuid, now: i64, hold: i64) -> Share {
        let mine = |w: Option<Uuid>| w == Some(worker);
        // What runs in its heavy slot is the queue's to bound, not its share's.
        let held = self.granted(|e| mine(e.worker));
        let tag = worker.to_string();
        let procs: Vec<_> =
            self.procs.iter().filter(|p| p.worker.as_deref() == Some(&tag) && !held.contains(&p.pid)).collect();
        let core_percent = procs.iter().map(|p| p.cpu).sum::<f64>();
        let rss_kib = procs.iter().map(|p| p.rss_kib).sum();
        let load = health::Load::of(core_percent, rss_kib, procs.len() as u64, self.cores);
        let heavy = self
            .queue
            .entries
            .iter()
            .any(|e| mine(e.worker) && self.alive.contains(&e.pid) && e.started_at.is_none_or(|t| now - t <= hold));
        Share { load, quiet: health::quiet(core_percent), heavy }
    }

    /// The host's reading with the CPU of the processes in `granted` left out.
    fn outside(&self, granted: &BTreeSet<u32>) -> Reading {
        let cores = self.cores as f64;
        let (inside, outside): (Vec<&Proc>, Vec<&Proc>) = self.procs.iter().partition(|p| granted.contains(&p.pid));
        let workers: f64 = outside.iter().filter(|p| p.worker.is_some()).map(|p| p.cpu).sum();
        let slots: f64 = inside.iter().map(|p| p.cpu).sum();
        let pct = |cpu: f64| (cpu / cores).round() as u64;
        let r = &self.host;
        let (memory_free, disk_free) = (r.memory_free, r.disk_free);
        Reading { cpu_busy: r.cpu_busy.saturating_sub(pct(slots)), memory_free, disk_free, workers_cpu: pct(workers) }
    }

    /// The processes the queue granted to its entries `of` picks: the pid of
    /// each started one, and every process under it.
    fn granted(&self, of: impl Fn(&factorium_domain::heavy::Entry) -> bool) -> BTreeSet<u32> {
        let started = self.queue.entries.iter().filter(|e| e.started_at.is_some() && of(e));
        let mut roots: Vec<u32> = started.map(|e| e.pid).collect();
        let mut all = BTreeSet::new();
        while let Some(pid) = roots.pop() {
            if all.insert(pid) {
                roots.extend(self.procs.iter().filter(|p| p.ppid == pid).map(|p| p.pid));
            }
        }
        all
    }
}
