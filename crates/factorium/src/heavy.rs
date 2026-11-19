//! The heavy-command queue as the app uses it, and the one way to run a
//! heavy command: queue it, wait for its turn (the CLI) or ask once (the
//! daemon, which asks again next tick), run its steps, end it if it holds
//! others up past the hold bound, and take it off the queue.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use factorium_domain::admission::{self, Reading};
use factorium_domain::heavy::{
    Begin, End, EndHeavy, Enqueue, EnqueueHeavy, Entry, Heavy, NoteRules, NoteRun, Rules, Runs, StartHeavy, older,
    queue_id,
};
use factorium_domain::settings::{Settings, Time};
use factorium_host::{PLACE, Priority, Proc, Ran, run};
use factum::Uuid;

use crate::{Answer, App, Refusal};

/// How often a waiting command asks for its turn, and a running one whether
/// it holds others up past the bound.
/// On the app's clock: a scenario moves it.
const POLL: u64 = 500;

/// A heavy command: its steps run in `dir` one after another, each under
/// `limit`, until one fails.
pub(crate) struct Job<'a> {
    pub what: String,
    pub worker: Option<Uuid>,
    /// When the line move it is for passes its bound, ms: past then, it
    /// goes ahead of workers' waiting entries.
    pub bound_at: Option<i64>,
    pub dir: &'a Path,
    pub steps: &'a [String],
    pub env: &'a [(String, String)],
    pub limit: Duration,
    /// Normal for whatever measures time a gate compares (a gate, a
    /// worker's `worker exclusive`), background for the rest (SPEC §1.9).
    pub priority: Priority,
}

/// How a job takes its turn.
pub(crate) enum Turn {
    /// Waits for it: a CLI command.
    Wait,
    /// Asks once, keeping its place under `key` until a tick's
    /// [`App::asking_once`] does not ask again: the daemon, whose tick never
    /// waits.
    Once(String),
}

impl App {
    /// The host's reading as the rules see it: the workers' share of the CPU
    /// added from their processes.
    pub(crate) fn reading(&self) -> Reading {
        self.reading_outside(&self.host.processes(), &BTreeSet::new())
    }

    /// [`App::reading`] with the CPU of the processes in `granted` left out:
    /// load outside the heavy slots, which the queue does not bound.
    pub(crate) fn reading_outside(&self, procs: &[Proc], granted: &BTreeSet<u32>) -> Reading {
        let r = self.host.reading();
        let cores = self.cores() as f64;
        let outside = || procs.iter().filter(|p| !granted.contains(&p.pid));
        let workers: f64 = outside().filter(|p| p.worker.is_some()).map(|p| p.cpu).sum();
        let slots: f64 = procs.iter().filter(|p| granted.contains(&p.pid)).map(|p| p.cpu).sum();
        let pct = |cpu: f64| (cpu / cores).round() as u64;
        let (memory_free, disk_free) = (r.memory_free, r.disk_free);
        Reading { cpu_busy: r.cpu_busy.saturating_sub(pct(slots)), memory_free, disk_free, workers_cpu: pct(workers) }
    }

    /// How many cores the host has: a process's CPU is a percent of one.
    pub(crate) fn cores(&self) -> usize {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }

    /// Runs `job` when its turn comes: how each step ran, up to the first
    /// that failed. `None`: [`Turn::Once`] and not its turn yet.
    pub(crate) async fn heavy(&self, job: Job<'_>, turn: Turn) -> Answer<Option<Vec<Ran>>> {
        let id = match &turn {
            // A waiter that execs the installed binary keeps its place.
            Turn::Wait => std::env::var(PLACE).ok().and_then(|p| p.parse().ok()).unwrap_or_else(Uuid::now_v7),
            Turn::Once(key) => {
                let id = factorium_domain::id(&format!("heavy:{key}"));
                self.memory.lock().unwrap().asked.insert(id);
                id
            }
        };
        loop {
            let took = match self.take_turn(id, &job).await {
                Err(r) if r.code == "stale" => match &turn {
                    Turn::Wait => {
                        let r = self.as_installed(id, r);
                        self.submit::<EndHeavy>(End { id }).await?;
                        return Err(r);
                    }
                    // The daemon execs the installed binary after this tick.
                    Turn::Once(_) => false,
                },
                took => took?,
            };
            match (took, &turn) {
                (true, _) => break,
                (false, Turn::Wait) => self.clock.sleep(POLL).await,
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

    /// One tick's asks of the queue: `asks` asks with [`Turn::Once`], and
    /// then the daemon's waiting jobs it did not ask for again leave the
    /// queue, as what they were for is no longer due. A failed `asks` keeps
    /// its asks for the next tick's scope.
    pub(crate) async fn asking_once<T>(&self, asks: impl Future<Output = Answer<T>>) -> Answer<T> {
        let done = asks.await?;
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
        Ok(done)
    }

    /// The daemon starts, or execs a new binary as the same pid: this pid's
    /// waiting entries were queued by the process it replaces, whose memory
    /// went with it. Taken as its own, the first tick's
    /// [`App::asking_once`] ends those it no longer asks for.
    pub(crate) async fn adopt_queued(&self) -> Answer<()> {
        let pid = std::process::id();
        let q = self.state::<Heavy>(queue_id()).await?;
        let mine = q.entries.iter().filter(|e| e.pid == pid && e.started_at.is_none()).map(|e| e.id);
        self.memory.lock().unwrap().queued.extend(mine);
        Ok(())
    }

    /// A waiter whose rules the queue no longer takes (refused as `stale`)
    /// execs the binary Homebrew links now, as the same pid, keeping its
    /// entry `id`; answers why not if it cannot.
    fn as_installed(&self, id: Uuid, stale: Refusal) -> Refusal {
        use std::os::unix::process::CommandExt;
        self.runs_as_this();
        let Some(link) = self.swapped() else { return stale };
        let e = std::process::Command::new(&link).args(std::env::args_os().skip(1)).env(PLACE, id.to_string()).exec();
        Refusal::new("stale", format!("{}; and exec {}: {e}", stale.message, link.display()))
    }

    /// Queues `id` as `what`, held by no worker, and asks once to start it,
    /// with room on the host as its caller read it. Refused, it is taken off
    /// the queue again, so it never holds others up: `false` when it must
    /// wait, the refusal otherwise (`stale`: a newer binary's rules replaced
    /// this one's). The daemon's idle work starts so.
    pub async fn start_once(&self, id: Uuid, what: String, settings: &Settings) -> Answer<bool> {
        let pid = std::process::id();
        self.submit::<EnqueueHeavy>(Enqueue { id, what, pid, worker: None, bound_at: None }).await?;
        let settle = settings.ms(Time::HeavySettle);
        let begin = Begin { id, room: Ok(()), read_at: self.now(), settle, rules: self.rules.clone() };
        match self.submit::<StartHeavy>(begin).await {
            Ok(()) => Ok(true),
            Err(r) => {
                self.submit::<EndHeavy>(End { id }).await?;
                if r.code == "wait" { Ok(false) } else { Err(r) }
            }
        }
    }

    /// Records that the rules this app asks under are the newest, if they are.
    pub(crate) async fn note_rules(&self) -> Answer<()> {
        self.submit::<NoteRules>(Rules { version: self.rules.clone() }).await
    }

    /// Queues `id` if it is not, and asks once to start it. Entries of
    /// processes that died are forgotten first.
    async fn take_turn(&self, id: Uuid, job: &Job<'_>) -> Answer<bool> {
        let q = self.state::<Heavy>(queue_id()).await?;
        if older(&q.rules, &self.rules) {
            self.note_rules().await?;
        }
        let pid = std::process::id();
        for e in q.entries.iter().filter(|e| e.id != id && e.pid != pid && !self.host.alive(e.pid)) {
            self.submit::<EndHeavy>(End { id: e.id }).await?;
        }
        if !q.holds(id) {
            let (what, worker, bound_at) = (job.what.clone(), job.worker, job.bound_at);
            self.submit::<EnqueueHeavy>(Enqueue { id, what, pid, worker, bound_at }).await?;
        }
        let settings = self.settings().await?;
        // Stamped before the host is read: a start after it is not in it.
        let read_at = self.now();
        let room = admission::room(&self.reading(), &settings);
        let settle = settings.ms(Time::HeavySettle);
        match self.submit::<StartHeavy>(Begin { id, room, read_at, settle, rules: self.rules.clone() }).await {
            Ok(()) => Ok(true),
            Err(r) if r.code == "wait" => Ok(false),
            Err(r) => Err(r),
        }
    }

    /// Runs the started `id`'s steps, noting the session each runs in, and
    /// ending the one running once it holds others up past the hold bound.
    /// Taken off the queue (an unqueue, which ends that session), it ends
    /// too, refused as `unqueued`; ended still queued, it was held.
    async fn run_held(&self, id: Uuid, job: &Job<'_>) -> Answer<Vec<Ran>> {
        let bound = self.settings().await?.ms(Time::HoldBound);
        let ended = Arc::new(AtomicBool::new(false));
        let mut rans = Vec::new();
        for step in job.steps {
            let (dir, line, env, limit) = (job.dir.to_path_buf(), step.clone(), job.env.to_vec(), job.limit);
            let (stop, priority) = (ended.clone(), job.priority);
            let (session, mut runs_in) = tokio::sync::mpsc::unbounded_channel();
            let (started, stop) = (move |s: u32| session.send(s).unwrap_or(()), move || stop.load(Ordering::Relaxed));
            let mut run = tokio::task::spawn_blocking(move || run(&dir, &line, &env, limit, priority, &started, &stop));
            let mut ran = loop {
                tokio::select! {
                    ran = &mut run => break ran.map_err(|e| Refusal::new("internal", e.to_string()))?,
                    Some(session) = runs_in.recv() => self.submit::<NoteRun>(Runs { id, session }).await?,
                    _ = self.clock.sleep(POLL) => {
                        let q = self.state::<Heavy>(queue_id()).await?;
                        ended.fetch_or(!q.holds(id) || q.overheld(id, self.now(), bound), Ordering::Relaxed);
                    }
                }
            };
            if !self.state::<Heavy>(queue_id()).await?.holds(id) {
                let why = format!("`{}` was unqueued: it and every process it started were ended", job.what);
                return Err(Refusal::new("unqueued", format!("{why}\n{}", ran.tail)));
            }
            if ended.load(Ordering::Relaxed) {
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

/// The processes the heavy queue granted: the pid of each started entry of
/// `entries`, and every process under it.
pub(crate) fn granted<'a>(procs: &[Proc], entries: impl IntoIterator<Item = &'a Entry>) -> BTreeSet<u32> {
    let mut roots: Vec<u32> = entries.into_iter().filter(|e| e.started_at.is_some()).map(|e| e.pid).collect();
    let mut all = BTreeSet::new();
    while let Some(pid) = roots.pop() {
        if all.insert(pid) {
            roots.extend(procs.iter().filter(|p| p.ppid == pid).map(|p| p.pid));
        }
    }
    all
}
