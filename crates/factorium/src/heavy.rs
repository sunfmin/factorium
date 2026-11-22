//! The heavy-command queue as the app uses it, and the one way to run a
//! heavy command: queue it, wait for its turn (the CLI), ask once (the
//! daemon, which asks again next tick) or ask and run beside the daemon's
//! ticks (idle work, and a line's promotion gate and cut), run its steps noting each one's session, end
//! it if it holds others up past the hold bound, and take it off the queue.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use factorium_domain::admission;
use factorium_domain::heavy::{
    Begin, End, EndHeavy, Ends, Enqueue, EnqueueHeavy, Heavy, NoteRules, NoteRun, Rules, Runs, StartHeavy, queue_id,
};
use factorium_domain::settings::Time;
use factorium_host::{PLACE, Priority, Ran, run};
use factum::Uuid;

use crate::{Answer, App, Refusal};

/// How often a waiting command asks for its turn, and a running one whether
/// it holds others up past the bound.
/// On the app's clock: a scenario moves it.
const POLL: u64 = 500;

/// A heavy command: its steps run in `dir` one after another, each under `limit`, until one fails.
/// Idle work's `limit` is the whole run's, on the app's clock. Its kind's constructor picks
/// its priority (SPEC §1.9), limit and env.
pub(crate) struct Job<'a> {
    what: String,
    worker: Option<Uuid>,
    /// Asked by a program of the owner's that is not a worker.
    program: bool,
    /// When the line move it is for passes its bound, ms: past then, it
    /// goes ahead of workers' waiting entries.
    bound_at: Option<i64>,
    dir: &'a Path,
    steps: &'a [String],
    env: Vec<(String, String)>,
    limit: Duration,
    /// `limit` is the whole run's, on the app's clock: idle work's.
    whole: bool,
    priority: Priority,
}

impl<'a> Job<'a> {
    /// A gate's `steps` in `dir`, each within `limit` s: what a gate compares, at normal priority.
    pub(crate) fn gate(what: String, dir: &'a Path, steps: &'a [String], limit: u64) -> Self {
        Job::new(what, dir, steps, Duration::from_secs(limit), Priority::Normal)
    }
    /// A cut's `steps` of `version` in its clone `dir`, each within `limit` s: in the background.
    pub(crate) fn cut(what: String, dir: &'a Path, steps: &'a [String], version: &str, limit: u64) -> Self {
        let env = vec![("FACTORIUM_VERSION".to_string(), version.to_string())];
        Job { env, ..Job::new(what, dir, steps, Duration::from_secs(limit), Priority::Background) }
    }
    /// A command run by hand (`worker exclusive`), within four hold bounds of `hold_ms`, a minute
    /// at least: at a gate's priority, or `background` for work whose time no gate compares.
    pub(crate) fn exclusive(what: String, dir: &'a Path, steps: &'a [String], hold_ms: i64, background: bool) -> Self {
        let limit = 4 * Duration::from_millis(hold_ms.max(60_000) as u64);
        Job::new(what, dir, steps, limit, if background { Priority::Background } else { Priority::Normal })
    }
    /// Idle work's `steps` in its clone `dir`, all within `limit` s on the app's clock: in the background.
    pub(crate) fn idle(what: String, dir: &'a Path, steps: &'a [String], limit: u64) -> Self {
        Job { whole: true, ..Job::new(what, dir, steps, Duration::from_secs(limit), Priority::Background) }
    }
    /// The job of a program of the owner's that is not a worker: it yields as a worker's does.
    pub(crate) fn of_program(self) -> Self {
        Job { program: true, ..self }
    }
    /// The job for `worker`, or for a line move due by `bound_at`.
    pub(crate) fn of(self, worker: Option<Uuid>, bound_at: Option<i64>) -> Self {
        Job { worker, bound_at, ..self }
    }
    fn new(what: String, dir: &'a Path, steps: &'a [String], limit: Duration, priority: Priority) -> Self {
        let env = Vec::new();
        Job { what, worker: None, program: false, bound_at: None, dir, steps, env, limit, whole: false, priority }
    }
}

/// Who runs a command by hand: a worker (`worker exclusive`; `None` outside a worker's session),
/// or a program of the owner's that is not a worker (`heavy run`), whose entry yields as a worker's.
pub(crate) enum Asker {
    Worker(Option<Uuid>),
    Program,
}

/// How a job takes its turn.
pub(crate) enum Turn {
    /// Waits for it: a CLI command.
    Wait,
    /// Asks once and, let start, runs beside the daemon's ticks ([`App::beside`]), kept with what
    /// it is `for` as one: the daemon's one way, so no tick waits for a run (factorium#231). With
    /// a `key`, it keeps its place under it until a tick's [`App::asking_once`] does not ask
    /// again: a line's move. Without one, refused, it leaves the queue: idle work. One that
    /// `yields` gives way.
    Beside { key: Option<String>, yields: bool, of: For },
}

/// What a run beside the ticks is for: what is left to do once its steps end.
pub(crate) enum For {
    Idle(Box<crate::idle::Running>),
    Line(Box<crate::line::Moving>),
}

impl For {
    /// Whether it is a move of `checkout`'s line.
    pub(crate) fn moves(&self, checkout: &str) -> bool {
        matches!(self, For::Line(m) if m.checkout == checkout)
    }
}

/// A run beside the ticks and what it is for, kept as one.
pub(crate) struct Beside(Running, For);

/// A job's steps running on a thread of their own, as entry `id`: beside
/// the ticks, within `deadline` on the app's clock; ended once it must, `why`.
pub(crate) struct Running {
    id: Uuid,
    yields: bool,
    deadline: Option<i64>,
    hold_ms: i64,
    why: Option<Ends>,
    stop: Arc<AtomicBool>,
    sessions: tokio::sync::mpsc::UnboundedReceiver<u32>,
    run: tokio::task::JoinHandle<Vec<Ran>>,
    /// Closed once its steps ended: [`App::beside_ended`] waits on it.
    ended: tokio::sync::watch::Receiver<()>,
}

impl App {
    /// Runs a program of the owner's command `cmd` in `dir` through the queue, as a worker's
    /// `worker exclusive` runs: at a gate's priority, or `background`'s.
    pub async fn heavy_run(&self, dir: &Path, cmd: &str, background: bool) -> Answer {
        self.exclusive(Asker::Program, dir, cmd, background).await
    }

    /// The queue's depth: how many entries run and how many wait, each entry, oldest first, and
    /// the places kept for entries that left unstarted.
    pub async fn heavy_depth(&self) -> Answer {
        let q = self.state::<Heavy>(queue_id()).await?;
        let (running, waiting) = (q.running().count(), q.waiting().count());
        Ok(serde_json::json!({ "running": running, "waiting": waiting, "entries": q.entries, "kept": q.left }))
    }

    /// Runs `cmd` in `dir` by hand for `asker`, waiting for its turn: within four hold bounds, at
    /// a gate's priority or `background`'s. Answers how it ran.
    pub(crate) async fn exclusive(&self, asker: Asker, dir: &Path, cmd: &str, background: bool) -> Answer {
        let (hold_ms, steps) = (self.settings().await?.ms(Time::HoldBound), [cmd.to_string()]);
        let job = Job::exclusive(cmd.into(), dir, &steps, hold_ms, background);
        let job = match asker {
            Asker::Worker(worker) => job.of(worker, None),
            Asker::Program => job.of_program(),
        };
        let ran = self.heavy(job, Turn::Wait).await?.and_then(|mut r| r.pop()).expect("a waited turn");
        Ok(serde_json::json!({ "ok": ran.ok, "code": ran.code, "took_ms": ran.took_ms, "tail": ran.tail }))
    }

    /// Runs `job` when its turn comes: how each step ran, up to the first that failed. `None`:
    /// [`Turn::Beside`], and not its turn yet. [`Turn::Beside`] answers no runs
    /// once it started: a tick's [`App::beside`] answers how it ran, with what it is for.
    pub(crate) async fn heavy(&self, job: Job<'_>, turn: Turn) -> Answer<Option<Vec<Ran>>> {
        // A waiter's own binary, noted before it waits: refused as stale, it finds what to exec (factorium#193).
        let _ = matches!(turn, Turn::Wait).then(|| self.exe());
        // `queued`: its entry was queued before this ask, so one the queue
        // no longer holds was unqueued.
        let (id, mut queued) = match &turn {
            Turn::Beside { key: None, .. } => (Uuid::now_v7(), false),
            // A waiter that execs the installed binary keeps its place.
            Turn::Wait => match std::env::var(PLACE).ok().and_then(|p| p.parse().ok()) {
                Some(place) => (place, true),
                None => (Uuid::now_v7(), false),
            },
            Turn::Beside { key: Some(key), .. } => {
                let id = factorium_domain::id(&format!("heavy:{key}"));
                self.memory.lock().unwrap().asked.insert(id);
                (id, false)
            }
        };
        loop {
            let took = match self.take_turn(id, &job, std::mem::replace(&mut queued, true)).await {
                Err(r) if r.code == "stale" => match &turn {
                    Turn::Wait => {
                        let r = self.as_installed(id, r);
                        self.submit::<EndHeavy>(End { id, keeps: true }).await?;
                        return Err(r);
                    }
                    // The daemon execs the installed binary after this tick.
                    Turn::Beside { .. } => false,
                },
                took => took?,
            };
            match (took, &turn) {
                (true, _) => break,
                (false, Turn::Wait) => self.clock.sleep(POLL).await,
                (false, Turn::Beside { key: Some(_), .. }) => {
                    self.memory.lock().unwrap().queued.insert(id);
                    return Ok(None);
                }
                (false, Turn::Beside { key: None, .. }) => {
                    return self.submit::<EndHeavy>(End { id, keeps: false }).await.map(|()| None);
                }
            }
        }
        if let Turn::Beside { yields, of, .. } = turn {
            let mut run = self.run_steps(id, &job, yields).await?;
            // Started, it is the tick's [`App::beside`]'s: no tick's asks end it.
            self.memory.lock().unwrap().queued.remove(&id);
            // Its first session noted before this answers: an unqueue from then on ends it.
            if let Some(session) = run.sessions.recv().await {
                self.submit::<NoteRun>(Runs { id, session }).await?;
            }
            self.memory.lock().unwrap().beside.push(Beside(run, of));
            return Ok(Some(Vec::new()));
        }
        let ran = self.run_held(id, &job).await;
        self.memory.lock().unwrap().queued.remove(&id);
        self.submit::<EndHeavy>(End { id, keeps: false }).await?;
        ran.map(Some)
    }

    /// One tick's asks of the queue: `asks` asks with a keyed [`Turn::Beside`], and
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
            self.submit::<EndHeavy>(End { id, keeps: false }).await?;
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
        let _ = self.linked();
        let Some(link) = self.swapped(true) else { return stale };
        let e = std::process::Command::new(&link).args(std::env::args_os().skip(1)).env(PLACE, id.to_string()).exec();
        Refusal::new("stale", format!("{}; and exec {}: {e}", stale.message, link.display()))
    }

    /// The jobs beside the ticks that `pick` picks, once each ended, off the queue: what each is
    /// for, and how its steps ran or why it was ended before. Those that run on are kept. `busy`:
    /// one that yields gives way. `end`: the daemon stops or is replaced, and none may outlive it.
    pub(crate) async fn beside(
        &self,
        pick: impl Fn(&For) -> bool,
        busy: bool,
        end: bool,
    ) -> Answer<Vec<(For, Result<Vec<Ran>, Ends>)>> {
        let picked: Vec<Beside> = {
            let mut m = self.memory.lock().unwrap();
            let (picked, kept) = std::mem::take(&mut m.beside).into_iter().partition(|b| pick(&b.1));
            m.beside = kept;
            picked
        };
        let (mut ended, mut failed, mut picked) = (Vec::new(), None, picked.into_iter());
        while let Some(mut b) = picked.next() {
            let looked = self.look_at(&mut b.0, busy || end).await;
            b.0.stop.fetch_or(end, Ordering::Relaxed);
            if !end && (looked.is_err() || (b.0.why.is_none() && !b.0.run.is_finished())) {
                self.memory.lock().unwrap().beside.push(b);
                failed = failed.or(looked.err());
                continue;
            }
            let Beside(r, of) = b;
            let (id, why, rans) = (r.id, r.why, self.ran(r).await);
            // Failing, those not yet looked at are kept for the next tick.
            let rans = match (self.submit::<EndHeavy>(End { id, keeps: false }).await, rans) {
                (Ok(()), Ok(rans)) => rans,
                (Err(e), _) | (_, Err(e)) => {
                    self.memory.lock().unwrap().beside.extend(picked);
                    return Err(e);
                }
            };
            ended.push((
                of,
                match why {
                    Some(w @ Ends::Unqueued) => Err(w),
                    Some(w @ Ends::GaveWay) if rans.iter().any(|r| !r.ok) => Err(w),
                    _ => Ok(rans),
                },
            ));
        }
        failed.map_or(Ok(ended), Err)
    }

    /// Whether a job that `pick` picks runs beside the ticks.
    pub(crate) fn beside_runs(&self, pick: impl Fn(&For) -> bool) -> bool {
        self.memory.lock().unwrap().beside.iter().any(|b| pick(&b.1))
    }

    /// Until each job beside the ticks that runs now has ended its steps: the next tick's
    /// [`App::beside`] finds it ended. A scenario waits on this, not on ticks in a loop.
    pub async fn beside_ended(&self) {
        self.ended_beside(|_| true).await
    }

    /// Until each line move beside the ticks that runs now has ended its steps.
    pub async fn moves_ended(&self) {
        self.ended_beside(|f| matches!(f, For::Line(_))).await
    }

    /// Whether a line move runs beside the ticks.
    pub fn moves_run(&self) -> bool {
        self.beside_runs(|f| matches!(f, For::Line(_)))
    }

    async fn ended_beside(&self, pick: impl Fn(&For) -> bool) {
        let ended: Vec<_> =
            self.memory.lock().unwrap().beside.iter().filter(|b| pick(&b.1)).map(|b| b.0.ended.clone()).collect();
        for mut ended in ended {
            // Nothing is sent on it: it answers once its sender is dropped.
            let _ = ended.changed().await;
        }
    }

    /// Records that the rules this app asks under are the queue's: the daemon's, as it starts.
    pub(crate) async fn note_rules(&self) -> Answer<()> {
        self.submit::<NoteRules>(Rules { version: self.rules.clone() }).await
    }

    /// Queues `id` if it is not, and asks once to start it: in the place its
    /// asker's entry for the same command had, if that left unstarted within
    /// the keep bound. Entries of processes that died are forgotten first,
    /// their places kept for their askers. One `queued` before that
    /// the queue no longer holds was unqueued: refused as `unqueued`, not
    /// queued again.
    async fn take_turn(&self, id: Uuid, job: &Job<'_>, queued: bool) -> Answer<bool> {
        let q = self.state::<Heavy>(queue_id()).await?;
        let pid = std::process::id();
        for e in q.entries.iter().filter(|e| e.id != id && e.pid != pid && !self.host.alive(e.pid)) {
            self.submit::<EndHeavy>(End { id: e.id, keeps: true }).await?;
        }
        if !q.holds(id) && queued {
            return Err(Refusal::new("unqueued", format!("`{}` was unqueued while it waited: it never ran", job.what)));
        }
        let settings = self.settings().await?;
        // Queued, or asked again under these rules: a binary that execed in its place keeps it.
        if q.entries.iter().all(|e| e.id != id || e.rules != self.rules) {
            let (what, worker, program, bound_at) = (job.what.clone(), job.worker, job.program, job.bound_at);
            let (keep, rules) = (settings.ms(Time::HeavyKeep), self.rules.clone());
            self.submit::<EnqueueHeavy>(Enqueue { id, what, pid, worker, program, bound_at, keep, rules }).await?;
        }
        // Stamped before the host is read: a start after it is not in it.
        let read_at = self.now();
        let room = admission::room(&crate::load::Load::queue(&*self.host), &settings);
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
        let mut r = self.run_steps(id, job, false).await?;
        loop {
            tokio::select! {
                session = r.sessions.recv() => match session {
                    Some(session) => self.submit::<NoteRun>(Runs { id, session }).await?,
                    None => break,
                },
                _ = self.clock.sleep(POLL) => self.look_at(&mut r, false).await?,
            }
        }
        let rans = self.ran(r).await?;
        if !self.state::<Heavy>(queue_id()).await?.holds(id) {
            let why = format!("`{}` was unqueued: it and every process it started were ended", job.what);
            let tail = rans.last().map_or("", |r| r.tail.as_str());
            return Err(Refusal::new("unqueued", format!("{why}\n{tail}")));
        }
        Ok(rans)
    }

    /// Starts `job`'s steps as the started entry `id`, one after another until one fails, each
    /// within the job's limit or, idle work's, all within it on the app's clock. One that `yields` gives way.
    async fn run_steps(&self, id: Uuid, job: &Job<'_>, yields: bool) -> Answer<Running> {
        let hold_ms = self.settings().await?.ms(Time::HoldBound);
        let deadline = job.whole.then(|| self.now().saturating_add(job.limit.as_millis() as i64));
        let (stop, (session, sessions)) = (Arc::new(AtomicBool::new(false)), tokio::sync::mpsc::unbounded_channel());
        let (dir, steps, env, priority) = (job.dir.to_path_buf(), job.steps.to_vec(), job.env.clone(), job.priority);
        let (ended, clock, limit) = (stop.clone(), self.clock.clone(), deadline.map_or(job.limit, |_| Duration::MAX));
        let (done, ended_rx) = tokio::sync::watch::channel(());
        let run = tokio::task::spawn_blocking(move || {
            let _done = done;
            let stop = || ended.load(Ordering::Relaxed) || deadline.is_some_and(|d| clock.now() >= d);
            let (started, mut rans) = (|s| session.send(s).unwrap_or(()), Vec::<Ran>::new());
            while rans.len() < steps.len() && rans.last().is_none_or(|r| r.ok) {
                rans.push(run(&dir, &steps[rans.len()], &env, limit, priority, &started, &stop));
            }
            rans
        });
        Ok(Running { id, yields, deadline, hold_ms, why: None, stop, sessions, run, ended: ended_rx })
    }

    /// Notes the sessions `r`'s steps started in, and ends the one running
    /// once `r` must end: unqueued, held past the hold bound, or giving way.
    async fn look_at(&self, r: &mut Running, busy: bool) -> Answer<()> {
        while let Ok(session) = r.sessions.try_recv() {
            self.submit::<NoteRun>(Runs { id: r.id, session }).await?;
        }
        let q = self.state::<Heavy>(queue_id()).await?;
        let ends = q.ends(r.id, self.now(), r.hold_ms, r.yields).or((r.yields && busy).then_some(Ends::GaveWay));
        r.why = r.why.or(ends);
        r.stop.store(r.why.is_some(), Ordering::Relaxed);
        Ok(())
    }

    /// How `r`'s steps ran, once they ended: ended held, the last one's tail
    /// says so; past its deadline, it `timed_out`.
    async fn ran(&self, r: Running) -> Answer<Vec<Ran>> {
        let mut rans = r.run.await.map_err(|e| Refusal::new("internal", e.to_string()))?;
        if let Some(last) = rans.last_mut().filter(|l| !l.ok) {
            if r.why == Some(Ends::Held) {
                last.tail = format!("{}\nended: {} of {}s", last.tail, Ends::Held, r.hold_ms / 1000);
            } else if r.deadline.is_some_and(|d| self.now() >= d) {
                last.timed_out = true;
            }
        }
        Ok(rans)
    }
}
