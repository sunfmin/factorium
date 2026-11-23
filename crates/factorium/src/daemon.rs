//! The daemon's tick: one pass over everything that moves without a
//! command — landings, promotions, cuts, issues to bump a pin of another
//! checkout, workers watched, one start if admission allows, idle work, and
//! the watch's look. The host and the heavy queue are read once a pass
//! ([`crate::load`]), after the lines' gates. No step waits for a heavy run:
//! those run beside the ticks. Each tick is timed by its steps, and one past
//! half the hang bound is kept for doctor, naming the step it spent most in.

use factorium_domain::admission;
use factorium_domain::bump;
use factorium_domain::checkout::Checkout;
use factorium_domain::daemon::{self, NoteTick, Start, StartDaemon, Step, Steps, Ticked};
use factorium_domain::doctor;
use factorium_domain::issue::Author;
use factorium_domain::kit::{Kits, kit_id};
use factorium_domain::settings::{Settings, Time};
use factorium_domain::tracker::{self, WorkerRow};
use factorium_domain::watch::{Held, NoteAdmission};
use factorium_domain::worker::{Of, Status as WorkerStatus, Unpark, Workers};
use factum::Value;
use serde::Serialize;
use serde_json::json;

use crate::lock::{self, Taken};
use crate::{Answer, App, LineClone, Refusal};

/// The descriptor a daemon hands the binary it runs in its place, open on
/// its pid file, whose lock it holds.
const LOCK_FD: &str = "FACTORIUM_DAEMON_LOCK_FD";

/// What one tick did.
#[derive(Debug, Default, Serialize)]
pub struct Tick {
    pub seen: Vec<Value>,
    pub landed: Vec<Value>,
    /// Issues closed whose worker landed: the close a dead daemon left undone.
    pub closed: Vec<Value>,
    pub lines: Vec<Value>,
    /// Idle work that ended, yielded or started.
    pub idle: Vec<Value>,
    pub started: Option<Value>,
    /// Parked workers sent back to work: what their issues waited for is met.
    pub resumed: Vec<Value>,
    /// Workers told their role changed under them.
    pub roles: Vec<Value>,
    /// Issues filed to bump a pin of another checkout behind its base tip.
    pub bumped: Vec<Value>,
    /// Why nothing started, when something was ready.
    pub held: Option<String>,
    pub watch: Option<Value>,
    /// The binary to run in place of this one once the tick ends: Homebrew
    /// links another version of the daemon's own formula, whoever cut it.
    pub restart: Option<String>,
    /// The upgrade of the daemon's own formula from the tap, started or ended.
    pub upgrade: Option<Value>,
    /// How long the tick took, ms on the app's clock.
    pub took_ms: i64,
    /// The step it spent most in.
    pub longest: Step,
    /// Past half the hang bound.
    pub slow: bool,
}

impl Tick {
    /// Its line in daemon.log, if daemon.log has it: it was slow, or did something (landed, moved
    /// a line, saw, started, resumed or told a worker its role changed, or moved idle work).
    pub fn line(&self) -> Option<String> {
        self.logged().then(|| serde_json::to_string(self).unwrap_or_default())
    }

    fn logged(&self) -> bool {
        self.slow
            || self.started.is_some()
            || !self.landed.is_empty()
            || !self.lines.is_empty()
            || !self.seen.is_empty()
            || !self.idle.is_empty()
            || !self.resumed.is_empty()
            || !self.roles.is_empty()
            || self.upgrade.is_some()
    }
}

/// How soon a stop between ticks is seen, ms on the app's clock.
const STOP_POLL: u64 = 100;

impl App {
    async fn listed(&self) -> Answer<Vec<Checkout>> {
        self.rows(tracker::listed()).await
    }

    pub(crate) async fn live_workers(&self) -> Answer<Vec<WorkerRow>> {
        self.rows(tracker::workers(None, false)).await
    }

    /// One pass of the daemon, recorded: a failing one adds to the run of
    /// failures doctor reads, and the watch still looks.
    pub async fn tick(&self) -> Answer<Tick> {
        let start = self.now();
        let mut steps = Steps::at(start);
        let mut done = self.pass(&mut steps).await;
        let error = done.as_ref().err().map(Refusal::to_string);
        self.proving(error.as_deref()).await;
        steps.ended("proving the binary", self.now());
        let took_ms = self.now() - start;
        let every = self.settings().await.unwrap_or_default().ms(Time::TickEvery);
        let slow = daemon::slow(took_ms, every).then(|| (took_ms, steps.longest.clone()));
        if let Ok(t) = &mut done {
            (t.took_ms, t.longest, t.slow) = (took_ms, steps.longest, slow.is_some());
        }
        let noted = self.submit::<NoteTick>(Ticked { error, slow }).await;
        if done.is_err() || noted.is_err() {
            let settings = self.settings().await.unwrap_or_default();
            if let Err(e) = self.look_if_due(&settings).await {
                eprintln!("look: {e}");
                self.look_failed(&e);
            }
        }
        noted.and(done)
    }

    async fn pass(&self, steps: &mut Steps) -> Answer<Tick> {
        let mut t = Tick::default();
        self.engine.settle().await?;
        let settings = self.settings().await?;
        steps.ended("settling the store", self.now());
        // Idle work that ended, or yields: before the lines, so it never runs ahead of them.
        t.idle.extend(self.idle_check().await?);
        t.closed = self.close_landed().await?;
        steps.ended("idle work's check and closing landed issues", self.now());
        // The lines, as each says what is due.
        let checkouts = self.listed().await?;
        (t.landed, t.lines) = self.asking_once(self.lines(&checkouts)).await?;
        steps.ended("the lines' moves", self.now());
        t.bumped = self.bumps(&checkouts).await?;
        steps.ended("bumps", self.now());
        // The host and the heavy queue, read once, after the lines' gates
        // ran: the workers' shares, admission and idle work all ask it.
        let load = self.load().await?;
        steps.ended("reading the host", self.now());
        // Workers: their sessions, and their share of the host.
        let hold = settings.ms(Time::HoldBound);
        for w in self.live_workers().await? {
            if let Some(seen) = self.worker_health(w.worker, load.worker(w.worker, self.now(), hold), &settings).await?
            {
                t.seen.push(seen);
            }
        }
        steps.ended("watching workers", self.now());
        t.roles = self.tell_role_changes().await?;
        steps.ended("telling workers their role changed", self.now());
        // One start, if admission allows.
        if let Some((checkout, number)) = self.next_issue().await? {
            let held = admission::admit(&load.admission(), &load.queue, self.now(), &settings);
            let held = match held {
                Ok(()) => match self.start_worker(&checkout, number).await {
                    Ok(started) => {
                        t.started = Some(started);
                        None
                    }
                    Err(r) if r.code == "no_harness" => Some(format!("kit: {}", r.message)),
                    // Claimed since it was picked: it starts on a later tick.
                    Err(r) if r.code == "claimed" => Some(format!("{checkout}#{number}: {}", r.message)),
                    Err(r) => return Err(r),
                },
                Err(why) => Some(why),
            };
            self.submit::<NoteAdmission>(Held { why: held.clone() }).await?;
            t.held = held;
        } else {
            self.submit::<NoteAdmission>(Held { why: None }).await?;
        }
        steps.ended("starting a worker", self.now());
        // Idle work, once the lines and the workers have what they need.
        t.idle.extend(self.idle_start(&checkouts, &load, &settings).await?);
        if admission::disk_short(&load.room(), &settings) {
            self.reclaim();
        }
        steps.ended("idle work's start", self.now());
        t.resumed = self.resume_parked().await?;
        steps.ended("resuming parked workers", self.now());
        t.watch = self.look_if_due(&settings).await?;
        steps.ended("the watch's look", self.now());
        t.upgrade = self.upgrade_from_tap(&checkouts, &settings).await?;
        steps.ended("upgrading from the tap", self.now());
        t.restart = self.swap_due().map(|b| b.to_string_lossy().into_owned());
        self.note_refusal().await?;
        Ok(t)
    }

    /// The linked binary to exec once proven; not while a line move runs
    /// beside the ticks, which a restart would end.
    fn swap_due(&self) -> Option<std::path::PathBuf> {
        (!self.moves_run()).then(|| self.swapped(false)).flatten()
    }

    /// The binary to run in place of this one after a tick: a good tick's
    /// restart. After a failed one, the linked binary once proven, since an
    /// older daemon whose store cannot read an event a newer binary wrote
    /// is mended by the swap alone (factorium#252); else the kept one a
    /// rollback goes back to.
    pub fn after(&self, ticked: &Answer<Tick>) -> Option<std::path::PathBuf> {
        match ticked {
            Ok(t) => t.restart.as_ref().map(Into::into),
            Err(_) => self.swap_due().or_else(|| self.fallback()),
        }
    }

    /// One issue to bump, in each checkout whose proving tip pins another
    /// checkout by git rev behind that one's base tip, while none is open
    /// (factorium#127). Read from refs as last fetched: no fetch of its own.
    async fn bumps(&self, checkouts: &[Checkout]) -> Answer<Vec<Value>> {
        let mut filed = Vec::new();
        for c in checkouts {
            let proving = c.fetched_proving();
            let pins = self.host.git(&c.path).shown(&proving, "Cargo.toml").map(|t| bump::pins(&t)).unwrap_or_default();
            if pins.is_empty() {
                continue;
            }
            let open: Vec<String> =
                self.rows(tracker::issues(Some(&c.name), false)).await?.into_iter().map(|r| r.title).collect();
            for d in checkouts.iter().filter(|d| d.name != c.name && !bump::open(&d.name, &open)) {
                let git = self.host.git(&d.path);
                let Some(pin) = git.origin().ok().and_then(|u| pins.iter().find(|p| bump::same_repo(&p.url, &u)))
                else {
                    continue;
                };
                let Ok(tip) = git.sha(&d.fetched_base()) else { continue };
                let landings = git.landings(&pin.rev, &tip).unwrap_or_default();
                if landings.is_empty() || !git.is_ancestor(&pin.rev, &tip) {
                    continue;
                }
                let (title, body) = bump::issue(&d.name, &pin.rev, &tip, &landings);
                filed.push(self.issue_add(&c.name, &title, &body, false, &[]).await?);
            }
        }
        Ok(filed)
    }

    /// The oldest ready issue no worker holds and nothing open blocks,
    /// whose account, if it names one, can start now, and whose role has
    /// what it needs of this host (each role checked once a tick).
    async fn next_issue(&self) -> Answer<Option<(String, u64)>> {
        let mut checked = std::collections::BTreeSet::new();
        let mut roles = crate::needs::Checked::new();
        let mut held = None;
        for r in self.rows(tracker::untaken()).await? {
            let (checkout, number) = (r.checkout, r.number);
            if !checked.contains(&checkout) && self.checkout(&checkout).await.is_err() {
                continue;
            }
            checked.insert(checkout.clone());
            let issue = self.issue(&checkout, number).await?;
            // Claimed, a command acts on its worktrees: it starts once that
            // is done. A claim whose process is gone is cleared.
            self.clear_dead_claim(&issue).await?;
            if self.issue(&checkout, number).await?.claim.is_some() {
                continue;
            }
            if !self.blockers(&issue).await?.iter().all(|(_, met)| met.is_some()) {
                continue;
            }
            // One that cannot start now (its account used up, or what its
            // checkout is pinned to at its limit) waits behind those that
            // can; with none, its start answers why it is held.
            if self.pick(&checkout, issue.on.account.as_deref()).await.is_err() {
                held.get_or_insert((checkout, number));
                continue;
            }
            // Its role lacking what it needs of this host, it waits; doctor
            // hands the watch what it lacks.
            let role = self.checkout(&checkout).await?.role_for(issue.on.role.as_deref()).map(|(r, _)| r.to_string());
            if let Some(role) = role
                && !self.role_ready(&role, &mut roles).await?
            {
                continue;
            }
            return Ok(Some((checkout, number)));
        }
        Ok(held)
    }

    /// Each worker the watch parked behind what its issue waits for, idle
    /// (waiting, or in a wait it declared), sent back to work once all of
    /// it is met, told what was met and how, and to merge the proving tip;
    /// and each parked on its engine's limit, once it reset. Unparked
    /// first, so a restart that does not reach it leaves it to the watch.
    async fn resume_parked(&self) -> Answer<Vec<Value>> {
        let mut resumed = Vec::new();
        for w in self.rows(tracker::workers(None, false)).await? {
            let wk = self.state::<Workers>(w.worker).await?;
            let idle = match wk.status {
                WorkerStatus::Waiting => true,
                WorkerStatus::Working => wk.declared.is_some(),
                _ => false,
            };
            if !idle {
                continue;
            }
            let text = match wk.waits_on_limit() {
                // Its engine's limit reset: what was held for it is typed first.
                Some(limit) if limit.until <= self.now() => {
                    "Your engine's limit reset. Pick up your issue where you left it.".to_string()
                }
                Some(_) => continue,
                None if !wk.parked => continue,
                None => {
                    let met: Option<Vec<String>> = (self.blockers(&self.issue(&w.checkout, w.number).await?).await?)
                        .into_iter()
                        .map(|(name, met)| Some(format!("{name} ({})", met?)))
                        .collect();
                    let Some(met) = met.filter(|m| !m.is_empty()) else { continue };
                    let p = self.checkout(&w.checkout).await?.proving;
                    format!(
                        "What your issue waited for is met: {}. Merge the proving tip into your branch \
                         (`git fetch origin {p} && git merge origin/{p}`) before you gate again.",
                        met.join(", ")
                    )
                }
            };
            self.submit::<Unpark>(Of { worker: w.worker }).await?;
            let did = self.restart(w.worker, Author::Factorium, &text, false).await;
            resumed.push(did.unwrap_or_else(|r| json!({ "worker": w.worker, "refused": r.to_string() })));
        }
        Ok(resumed)
    }

    /// Frees disk: the build outputs of the line's worktrees, rebuilt when
    /// next needed.
    fn reclaim(&self) {
        for kind in LineClone::ALL {
            let Ok(dirs) = std::fs::read_dir(self.data().clones(kind)) else { continue };
            for d in dirs.flatten() {
                let _ = std::fs::remove_dir_all(d.path().join("target"));
            }
        }
    }

    /// The daemon starts: the programs it needs are looked for on its PATH
    /// once, and what is missing is recorded for doctor, not met tick by
    /// tick; a start it died in the middle of is undone.
    pub async fn daemon_start(&self) -> Answer {
        let kit = self.state::<Kits>(kit_id()).await?;
        let mut needed = vec!["git".to_string(), "orca".to_string()];
        for e in &kit.entries {
            let program = e.command.split_whitespace().next().unwrap_or_default().to_string();
            if !program.is_empty() && !needed.contains(&program) {
                needed.push(program);
            }
        }
        let missing: Vec<String> = needed.into_iter().filter(|p| !self.host.programs().found(p)).collect();
        let path = std::env::var("PATH").unwrap_or_default();
        self.submit::<StartDaemon>(Start { path: path.clone(), missing: missing.clone() }).await?;
        // Waiters of older binaries no longer start once this one runs.
        self.note_rules().await?;
        self.adopt_queued().await?;
        let freed = self.free_unstarted().await?;
        Ok(json!({ "path": path, "missing": missing, "freed": freed }))
    }

    /// `factorium run`: ticks until `stop` is set (on SIGTERM or SIGINT, as
    /// `stop` and launchd send), then ends in order: the tick in progress
    /// finishes, idle work ends, the pid file goes. One daemon runs per
    /// database: it holds a lock on the pid file for its life, and another
    /// is refused while it does, whatever the file says. `handed` is the
    /// descriptor the daemon this process replaced handed it (`handed_lock`).
    pub async fn run(&self, stop: &std::sync::atomic::AtomicBool, handed: Option<i32>) -> Answer {
        let pid = self.data().pid();
        let me = std::process::id().to_string();
        // Held already if this process ran a newly installed binary as the
        // same pid: the lock came through the exec.
        let inherited = handed.and_then(|fd| crate::host::procs::inherited(fd, &pid)).filter(|f| f.try_lock().is_ok());
        let held = match inherited.map_or_else(|| lock::take(&pid, &me), Ok) {
            Ok(held) => held,
            Err(Taken::Held(said)) => {
                let who = said.trim().parse::<u32>().ok().filter(|n| *n != 0 && self.host.alive(*n));
                let why = who.map_or_else(|| format!("a daemon holds {}", pid.display()), |n| format!("as pid {n}"));
                return Err(Refusal::new("running", format!("a daemon runs already, {why}")));
            }
            Err(Taken::Fails(e)) => return Err(Refusal::new("io", e)),
        };
        let _ = self.linked();
        // A database this binary cannot use fails its ticks, which may go
        // back to the kept binary; their looks reach the watch outside the store.
        match self.daemon_start().await {
            Ok(started) => eprintln!("started: {started}"),
            Err(e) => eprintln!("start: {e}"),
        }
        let mut ticks = 0u64;
        self.note_ticked();
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            let ticked = self.tick().await;
            self.note_ticked();
            match &ticked {
                Ok(t) => t.line().into_iter().for_each(|l| println!("{l}")),
                Err(e) => eprintln!("tick: {e}"),
            }
            if let Some(binary) = self.after(&ticked) {
                self.exec(&binary, &held).await;
            }
            ticks += 1;
            let every = self
                .settings()
                .await
                .map(|s| s.ms(Time::TickEvery))
                .unwrap_or_else(|_| Settings::default().ms(Time::TickEvery));
            // On the app's clock, in slices, so a stop between ticks ends the wait.
            let until = self.now() + every;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) && self.now() < until {
                self.clock.sleep(STOP_POLL).await;
            }
        }
        let _ = self.beside(|_| true, false, true).await;
        let _ = std::fs::remove_file(self.data().ticked());
        let _ = std::fs::remove_file(&pid);
        Ok(json!({ "stopped": true, "ticks": ticks }))
    }

    /// The daemon is alive and ticking, now: what [`Self::daemon_down`]
    /// reads. A file, not an event: a tick that does nothing records nothing.
    fn note_ticked(&self) {
        let _ = std::fs::write(self.data().ticked(), self.now().to_string());
    }

    /// Why no daemon runs on this database, if none does: its pid file names
    /// no live process holding its lock, or that one has not ticked lately.
    pub async fn daemon_down(&self) -> Option<String> {
        let pid = self.daemon_pid();
        // The lock is tried only on a live pid: a daemon starting takes it
        // before it writes its own, and is never refused for this look.
        let locked = pid.is_some() && lock::held(&self.data().pid());
        let ticked = std::fs::read_to_string(self.data().ticked()).ok().and_then(|t| t.trim().parse().ok());
        let every = self.settings().await.unwrap_or_default().ms(Time::TickEvery);
        daemon::down(&daemon::Probe { pid, locked, ticked }, every, self.now())
    }

    /// `factorium doctor` as the owner runs it: [`Self::doctor`], with no
    /// daemon running as its first finding, unless the store cannot be read
    /// (that is its one finding). The watch's look leaves it out: the daemon runs it.
    pub async fn doctor_here(&self) -> Answer {
        let mut answer = self.doctor().await?;
        if let (None, Some(why)) = (answer.get("unreadable"), self.daemon_down().await) {
            let log = self.data().log().display().to_string();
            let found = doctor::no_daemon(&why, self.service_loaded().as_deref(), &log);
            if let Some(f) = answer["findings"].as_array_mut() {
                f.insert(0, json!(found));
            }
        }
        Ok(answer)
    }

    /// Runs `binary` in place of this one, between ticks, as the same pid:
    /// the pid file stays right, and its lock stays held. Answers only if it cannot.
    async fn exec(&self, binary: &std::path::Path, held: &std::fs::File) {
        use std::os::unix::process::CommandExt;
        let fd = match crate::host::procs::kept_across_exec(held, true) {
            Ok(fd) => fd,
            Err(e) => return eprintln!("restart as {}: its lock would not stay held: {e}", binary.display()),
        };
        let _ = self.beside(|_| true, false, true).await;
        let e =
            std::process::Command::new(binary).args(std::env::args_os().skip(1)).env(LOCK_FD, fd.to_string()).exec();
        let _ = crate::host::procs::kept_across_exec(held, false);
        eprintln!("restart as {}: {e}", binary.display());
    }

    /// The pid of the daemon running now, if one is.
    pub(crate) fn daemon_pid(&self) -> Option<u32> {
        let said = std::fs::read_to_string(self.data().pid()).ok()?;
        said.trim().parse().ok().filter(|n| *n != 0 && self.host.alive(*n))
    }

    /// `factorium stop`: ends the running daemon (run by the service, the
    /// service is unloaded, so that launchd does not start it again), then
    /// closes the watch's terminal and records it has no session, so the next
    /// daemon starts a fresh one. Workers' sessions go on; the next daemon
    /// watches them.
    pub async fn stop(&self) -> Answer {
        let mut answer = if let Some(service) = self.service_loaded() {
            self.service_stop(&service)?
        } else if let Some(n) = self.daemon_pid() {
            let _ = std::process::Command::new("kill").arg(n.to_string()).status();
            json!({ "stopped": true, "pid": n })
        } else {
            // Only once its lock is ours: a daemon that has it and has not
            // yet written its pid keeps its file.
            if let Ok(_held) = lock::take(&self.data().pid(), "") {
                let _ = std::fs::remove_file(self.data().pid());
            }
            json!({ "stopped": false, "why": "no daemon runs" })
        };
        answer["watch_closed"] = json!(self.close_watch_session().await?);
        Ok(answer)
    }
}

/// The descriptor of the pid file whose lock the daemon this process
/// replaced (`exec`) handed it, as its environment names it. Read by the
/// process edge alone: every process a daemon starts inherits the variable,
/// a test run among them, and in one the descriptor is a file of its own.
pub fn handed_lock() -> Option<i32> {
    std::env::var(LOCK_FD).ok()?.parse().ok()
}
