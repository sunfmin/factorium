//! The heavy-command queue as the app uses it, and the one way to run a
//! heavy command: queue it, wait for its turn (the CLI) or ask once (the
//! daemon, which asks again next tick), run its steps, end it if it holds
//! others up past the hold bound, and take it off the queue.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use factorium_domain::admission::{self, Reading};
use factorium_domain::heavy::{Begin, End, EndHeavy, Enqueue, EnqueueHeavy, Heavy, StartHeavy, queue_id};
use factorium_host::{Ran, run};
use factum::Uuid;

use crate::{Answer, App, Refusal};

/// How often a waiting command asks for its turn, and a running one whether
/// it holds others up past the bound.
const POLL: Duration = Duration::from_millis(500);

/// A heavy command: its steps run in `dir` one after another, each under
/// `limit`, until one fails.
pub(crate) struct Job<'a> {
    pub what: String,
    pub worker: Option<Uuid>,
    pub dir: &'a Path,
    pub steps: &'a [String],
    pub env: &'a [(String, String)],
    pub limit: Duration,
}

/// How a job takes its turn.
pub(crate) enum Turn {
    /// Waits for it: a CLI command.
    Wait,
    /// Asks once, keeping its place under `key` until the next tick asks
    /// again: the daemon, whose tick never waits.
    Once(String),
}

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

    /// Runs `job` when its turn comes: how each step ran, up to the first
    /// that failed. `None`: [`Turn::Once`] and not its turn yet.
    pub(crate) async fn heavy(&self, job: Job<'_>, turn: Turn) -> Answer<Option<Vec<Ran>>> {
        let id = match &turn {
            Turn::Wait => Uuid::now_v7(),
            Turn::Once(key) => {
                let id = factorium_domain::id(&format!("heavy:{key}"));
                self.memory.lock().unwrap().asked.insert(id);
                id
            }
        };
        loop {
            match (self.take_turn(id, &job).await?, &turn) {
                (true, _) => break,
                (false, Turn::Wait) => tokio::time::sleep(POLL).await,
                (false, Turn::Once(_)) => {
                    self.memory.lock().unwrap().queued.insert(id);
                    return Ok(None);
                }
            }
        }
        let ran = self.run_held(id, &job).await;
        self.memory.lock().unwrap().queued.remove(&id);
        self.submit::<EndHeavy>(End { id }).await?;
        ran.map(Some)
    }

    /// Takes the daemon's waiting jobs that this tick did not ask for again
    /// off the queue: what they were for is no longer due.
    pub(crate) async fn heavy_forget_unasked(&self) -> Answer<()> {
        let gone: Vec<Uuid> = {
            let mut m = self.memory.lock().unwrap();
            let asked = std::mem::take(&mut m.asked);
            let gone = m.queued.difference(&asked).copied().collect();
            m.queued.retain(|id| asked.contains(id));
            gone
        };
        for id in gone {
            self.submit::<EndHeavy>(End { id }).await?;
        }
        Ok(())
    }

    /// Queues `id` if it is not, and asks once to start it. Entries of
    /// processes that died are forgotten first.
    async fn take_turn(&self, id: Uuid, job: &Job<'_>) -> Answer<bool> {
        let q = self.state::<Heavy>(queue_id()).await?;
        let pid = std::process::id();
        for e in q.entries.iter().filter(|e| e.id != id && e.pid != pid && !self.host.alive(e.pid)) {
            self.submit::<EndHeavy>(End { id: e.id }).await?;
        }
        if !q.entries.iter().any(|e| e.id == id) {
            self.submit::<EnqueueHeavy>(Enqueue { id, what: job.what.clone(), pid, worker: job.worker }).await?;
        }
        let room = admission::room(&self.reading(), &self.settings().await?);
        match self.submit::<StartHeavy>(Begin { id, room, read_at: self.now() }).await {
            Ok(()) => Ok(true),
            Err(r) if r.code == "wait" => Ok(false),
            Err(r) => Err(r),
        }
    }

    /// Runs the started `id`'s steps, ending the one running once it holds
    /// others up past the hold bound.
    async fn run_held(&self, id: Uuid, job: &Job<'_>) -> Answer<Vec<Ran>> {
        let bound = self.settings().await?.ms("hold_bound");
        let held = Arc::new(AtomicBool::new(false));
        let mut rans = Vec::new();
        for step in job.steps {
            let (dir, line, env, limit) = (job.dir.to_path_buf(), step.clone(), job.env.to_vec(), job.limit);
            let flag = held.clone();
            let mut run =
                tokio::task::spawn_blocking(move || run(&dir, &line, &env, limit, &|| flag.load(Ordering::Relaxed)));
            let mut ran = loop {
                tokio::select! {
                    ran = &mut run => break ran.map_err(|e| Refusal::new("internal", e.to_string()))?,
                    _ = tokio::time::sleep(POLL) => {
                        if self.state::<Heavy>(queue_id()).await?.overheld(id, self.now(), bound) {
                            held.store(true, Ordering::Relaxed);
                        }
                    }
                }
            };
            if held.load(Ordering::Relaxed) {
                let s = bound / 1000;
                ran.tail = format!("{}\nended: it held others up past the hold bound of {s}s", ran.tail);
            }
            let ok = ran.ok;
            rans.push(ran);
            if !ok {
                break;
            }
        }
        Ok(rans)
    }
}
