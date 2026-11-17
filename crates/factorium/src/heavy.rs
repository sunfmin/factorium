//! The heavy-command queue as the app uses it: a CLI waits its turn and
//! runs; the daemon asks once per tick and goes on if it must wait.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use factorium_domain::admission::{self, Reading};
use factorium_domain::heavy::{Begin, End, EndHeavy, Enqueue, EnqueueHeavy, Heavy, StartHeavy, queue_id};
use factorium_host::{Ran, run};
use factum::Uuid;

use crate::{Answer, App};

impl App {
    /// The host's reading as the rules see it: the workers' share of the CPU
    /// added from their processes.
    pub(crate) fn reading(&self) -> Reading {
        let r = self.host.reading();
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        let workers: f64 = self.host.processes().iter().filter(|p| p.worker.is_some()).map(|p| p.cpu).sum();
        Reading {
            cpu_busy: r.cpu_busy,
            memory_free: r.memory_free,
            disk_free: r.disk_free,
            workers_cpu: (workers / cores).round() as u64,
        }
    }

    /// Asks once to start `id` (queuing it first): whether it started, or
    /// why it waits. Entries of processes that died are forgotten first.
    pub(crate) async fn heavy_try(&self, id: Uuid, what: &str, worker: Option<Uuid>) -> Answer<Result<(), String>> {
        let q = self.state::<Heavy>(queue_id()).await?;
        for e in q.entries.iter().filter(|e| e.id != id && e.pid != std::process::id() && !self.host.alive(e.pid)) {
            self.submit::<EndHeavy>(End { id: e.id }).await?;
        }
        if !q.entries.iter().any(|e| e.id == id) {
            let pid = std::process::id();
            self.submit::<EnqueueHeavy>(Enqueue { id, what: what.into(), pid, worker }).await?;
        }
        if q.entries.iter().any(|e| e.id == id && e.started_at.is_some()) {
            return Ok(Ok(()));
        }
        let settings = self.settings().await?;
        let read_at = self.now();
        let room = admission::room(&self.reading(), &settings);
        match self.submit::<StartHeavy>(Begin { id, room, read_at }).await {
            Ok(()) => Ok(Ok(())),
            Err(r) if r.code == "wait" => Ok(Err(r.message)),
            Err(r) => Err(r),
        }
    }

    pub(crate) async fn heavy_end(&self, id: Uuid) -> Answer<()> {
        self.submit::<EndHeavy>(End { id }).await
    }

    /// Runs `line` in `dir` as a heavy command: waits its turn, runs it under
    /// `limit`, and ends it early if it holds others up past the hold bound.
    pub async fn heavy(
        &self,
        what: &str,
        worker: Option<Uuid>,
        dir: &Path,
        line: &str,
        env: &[(String, String)],
        limit: Duration,
    ) -> Answer<Ran> {
        let id = Uuid::now_v7();
        loop {
            match self.heavy_try(id, what, worker).await? {
                Ok(()) => break,
                Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (dir, line, env, flag) = (dir.to_path_buf(), line.to_string(), env.to_vec(), stop.clone());
        let mut job =
            tokio::task::spawn_blocking(move || run(&dir, &line, &env, limit, &|| flag.load(Ordering::Relaxed)));
        let bound = self.settings().await?.ms("hold_bound");
        let ran = loop {
            tokio::select! {
                ran = &mut job => break ran.map_err(|e| crate::Refusal::new("internal", e.to_string()))?,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    let q = self.state::<Heavy>(queue_id()).await?;
                    if q.overheld(id, self.now(), bound) {
                        stop.store(true, Ordering::Relaxed);
                    }
                }
            }
        };
        self.heavy_end(id).await?;
        Ok(ran)
    }
}
