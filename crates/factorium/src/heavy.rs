//! The heavy-command queue as the app uses it, and the one way to run a
//! heavy command: queue it, wait for its turn (the CLI), ask once (the
//! daemon, which asks again next tick) or ask once and run beside the
//! daemon's ticks (idle work), run its steps noting each one's session, end
//! it if it holds others up past the hold bound, and take it off the queue.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use factorium_domain::admission::{self, Reading};
use factorium_domain::heavy::{
    Begin, End, EndHeavy, Enqueue, EnqueueHeavy, Entry, Heavy, NoteRules, NoteRun, Rules, Runs, StartHeavy, older,
    queue_id,
};
use factorium_domain::settings::Time;
use factorium_host::{PLACE, Priority, Proc, Ran, run};
use factum::Uuid;

use crate::{Answer, App, Refusal};

/// How often a waiting command asks for its turn, and a running one whether
/// it holds others up past the bound.
/// On the app's clock: a scenario moves it.
const POLL: u64 = 500;

/// A heavy command: its steps run in `dir` one after another, each under
/// `limit`, until one fails. Beside the ticks, `limit` is the whole run's,
/// on the app's clock.
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
    /// Asks once and, let start, runs beside the daemon's ticks, which
    /// [`App::beside`] ends it on: the daemon's idle work. Refused, it
    /// leaves the queue at once, holding nothing up. One that `yields` gives
    /// way to anything else queued.
    Beside { yields: bool },
}

/// The steps of a [`Job`], owned to run on a thread of their own.
struct Steps {
    dir: PathBuf,
    steps: Vec<String>,
    env: Vec<(String, String)>,
    limit: Duration,
    priority: Priority,
}

impl Steps {
    fn of(job: &Job<'_>) -> Steps {
        let (dir, steps, env) = (job.dir.to_path_buf(), job.steps.to_vec(), job.env.to_vec());
        Steps { dir, steps, env, limit: job.limit, priority: job.priority }
    }

    /// Runs them one after another until one fails, telling `started` the
    /// session each runs in, and ending the one running once `stop` says so.
    fn run(self, started: &dyn Fn(u32), stop: &dyn Fn() -> bool) -> Vec<Ran> {
        let mut rans = Vec::new();
        for step in &self.steps {
            let ran = run(&self.dir, step, &self.env, self.limit, self.priority, started, stop);
            let ok = ran.ok;
            rans.push(ran);
            if !ok {
                break;
            }
        }
        rans
    }
}

/// The job running beside the daemon's ticks ([`Turn::Beside`]): the host
/// runs one at a time.
pub(crate) struct Beside {
    /// Its queue entry.
    id: Uuid,
    yields: bool,
    /// When its limit passes, on the app's clock, ms.
    deadline: i64,
    /// Set, its running step ends, and so its run.
    stop: Arc<AtomicBool>,
    sessions: Sessions,
    run: std::thread::JoinHandle<Vec<Ran>>,
}

/// The sessions a job's steps run in, as each starts.
type Sessions = tokio::sync::mpsc::UnboundedReceiver<u32>;

/// How the job beside the ticks went, as a tick found it.
pub(crate) enum Besides {
    /// It runs on.
    Runs,
    /// Its steps ran, up to the first that failed: past its limit, that one
    /// `timed_out`; held others up past the hold bound, its tail says so.
    Ran(Vec<Ran>),
    /// It gave way to what else the queue holds, or its caller's `busy`.
    GaveWay,
    /// It was unqueued, which ended it.
    Unqueued,
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
    /// that failed. `None`: [`Turn::Once`] or [`Turn::Beside`], and not its
    /// turn yet. [`Turn::Beside`] answers no runs once it started: each
    /// tick's [`App::beside`] answers how it ran.
    pub(crate) async fn heavy(&self, job: Job<'_>, turn: Turn) -> Answer<Option<Vec<Ran>>> {
        let id = match &turn {
            Turn::Beside { yields } => return self.start_beside(&job, *yields).await,
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
                    Turn::Once(_) | Turn::Beside { .. } => false,
                },
                took => took?,
            };
            match (took, &turn) {
                (true, _) => break,
                (false, Turn::Wait) => self.clock.sleep(POLL).await,
                (false, Turn::Once(_) | Turn::Beside { .. }) => {
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

    /// [`Turn::Beside`]: queues `job` and asks once to start it. Started, its
    /// steps run on a thread of their own, the first one's session noted
    /// before this answers, so an unqueue from then on ends it; refused, it
    /// leaves the queue again (`stale`: a newer binary's rules replaced this
    /// one's, which the daemon execs after this tick).
    async fn start_beside(&self, job: &Job<'_>, yields: bool) -> Answer<Option<Vec<Ran>>> {
        let id = Uuid::now_v7();
        let took = self.take_turn(id, job).await;
        if !matches!(took, Ok(true)) {
            self.submit::<EndHeavy>(End { id }).await?;
            return took.map(|_| None);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (session, mut sessions) = tokio::sync::mpsc::unbounded_channel();
        let deadline = self.now().saturating_add(job.limit.as_millis().try_into().unwrap_or(i64::MAX));
        let (steps, ended, clock) = (Steps { limit: Duration::MAX, ..Steps::of(job) }, stop.clone(), self.clock.clone());
        let run = std::thread::spawn(move || {
            let started = |s: u32| session.send(s).unwrap_or(());
            steps.run(&started, &|| ended.load(Ordering::Relaxed) || clock.now() >= deadline)
        });
        if let Some(session) = sessions.recv().await {
            self.submit::<NoteRun>(Runs { id, session }).await?;
        }
        self.memory.lock().unwrap().beside = Some(Beside { id, yields, deadline, stop, sessions, run });
        Ok(Some(Vec::new()))
    }

    /// The job beside the ticks, as this tick finds it, its sessions noted:
    /// ended, and off the queue, once it ran, was unqueued, held others up
    /// past the hold bound, or — one that yields — anything else is queued
    /// or its caller is `busy`. `None`: none runs.
    pub(crate) async fn beside(&self, busy: bool) -> Answer<Option<Besides>> {
        let Some(mut b) = self.memory.lock().unwrap().beside.take() else { return Ok(None) };
        while let Ok(session) = b.sessions.try_recv() {
            self.submit::<NoteRun>(Runs { id: b.id, session }).await?;
        }
        let q = self.state::<Heavy>(queue_id()).await?;
        let bound = self.settings().await?.ms(Time::HoldBound);
        let (held, overheld) = (q.holds(b.id), q.overheld(b.id, self.now(), bound));
        let gives_way = b.yields && (busy || q.entries.iter().any(|e| e.id != b.id));
        if !b.run.is_finished() && held && !overheld && !gives_way {
            self.memory.lock().unwrap().beside = Some(b);
            return Ok(Some(Besides::Runs));
        }
        let deadline = b.deadline;
        let mut rans = self.end_beside(b).await;
        if !held {
            return Ok(Some(Besides::Unqueued));
        }
        let Some(last) = rans.last_mut().filter(|r| !r.ok) else { return Ok(Some(Besides::Ran(rans))) };
        if gives_way {
            return Ok(Some(Besides::GaveWay));
        }
        if overheld {
            held_past(last, bound);
        } else if self.now() >= deadline {
            last.timed_out = true;
        }
        Ok(Some(Besides::Ran(rans)))
    }

    /// Ends the job beside the ticks, if one runs: the daemon is about to
    /// stop or be replaced, and what it started must not outlive it.
    pub(crate) async fn beside_end(&self) {
        let Some(b) = self.memory.lock().unwrap().beside.take() else { return };
        self.end_beside(b).await;
    }

    /// Ends `b`'s run, if it runs, and takes it off the queue: how its steps ran.
    async fn end_beside(&self, b: Beside) -> Vec<Ran> {
        b.stop.store(true, Ordering::Relaxed);
        let joined = tokio::task::spawn_blocking(move || b.run.join()).await;
        let _ = self.submit::<EndHeavy>(End { id: b.id }).await;
        joined.ok().and_then(Result::ok).unwrap_or_else(|| {
            vec![Ran { ok: false, code: None, tail: "its run panicked".into(), took_ms: 0, timed_out: false }]
        })
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
        let (steps, stop) = (Steps::of(job), ended.clone());
        let (session, mut runs_in) = tokio::sync::mpsc::unbounded_channel();
        let (started, stop) = (move |s: u32| session.send(s).unwrap_or(()), move || stop.load(Ordering::Relaxed));
        let mut run = tokio::task::spawn_blocking(move || steps.run(&started, &stop));
        let mut rans = loop {
            tokio::select! {
                rans = &mut run => break rans.map_err(|e| Refusal::new("internal", e.to_string()))?,
                Some(session) = runs_in.recv() => self.submit::<NoteRun>(Runs { id, session }).await?,
                _ = self.clock.sleep(POLL) => {
                    let q = self.state::<Heavy>(queue_id()).await?;
                    ended.fetch_or(!q.holds(id) || q.overheld(id, self.now(), bound), Ordering::Relaxed);
                }
            }
        };
        if !self.state::<Heavy>(queue_id()).await?.holds(id) {
            let why = format!("`{}` was unqueued: it and every process it started were ended", job.what);
            let tail = rans.last().map_or("", |r| r.tail.as_str());
            return Err(Refusal::new("unqueued", format!("{why}\n{tail}")));
        }
        if let Some(last) = rans.last_mut().filter(|_| ended.load(Ordering::Relaxed)) {
            held_past(last, bound);
        }
        Ok(rans)
    }
}

/// Says in `ran`'s tail that it was ended for holding others up past `bound`, ms.
fn held_past(ran: &mut Ran, bound: i64) {
    ran.tail = format!("{}\nended: it held others up past the hold bound of {}s", ran.tail, bound / 1000);
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
