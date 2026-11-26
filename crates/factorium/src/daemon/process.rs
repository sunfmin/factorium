//! The daemon as a process: one per database, holding a lock on its pid
//! file for its life and through each exec of a newer binary in its place
//! (the descriptor handed on in its environment); started once, then
//! ticking ([`App::tick`]) until stopped; alive while its ticked file is
//! fresh. The tick knows none of this: it answers what it did and what to
//! run next ([`App::after`]).

use factorium_domain::daemon::{self, Start, StartDaemon};
use factorium_domain::doctor;
use factorium_domain::settings::{Settings, Time};
use serde_json::json;

use crate::lock::{self, Taken};
use crate::{Answer, App, Refusal};

/// The descriptor a daemon hands the binary it runs in its place, open on
/// its pid file, whose lock it holds.
const LOCK_FD: &str = "FACTORIUM_DAEMON_LOCK_FD";

/// How soon a stop between ticks is seen, ms on the app's clock.
const STOP_POLL: u64 = 100;

impl App {
    /// The daemon starts: the kit's commands move to stablehand, once; the
    /// programs it needs (git, orca, and those of the commands stablehand
    /// keeps, else the fallback's) are looked for on its PATH once, and
    /// what is missing is recorded for doctor, not met tick by tick; a
    /// start it died in the middle of is undone.
    pub async fn daemon_start(&self) -> Answer {
        let moved = self.move_commands().await?;
        let mut needed = vec!["git".to_string(), "orca".to_string()];
        let kept = self.kept_commands().map(|k| k.into_values().collect::<Vec<_>>());
        for command in kept.unwrap_or_else(|_| vec![factorium_domain::kit::FALLBACK_COMMAND.into()]) {
            if let Some(program) = command.split_whitespace().next().map(str::to_string)
                && !needed.contains(&program)
            {
                needed.push(program);
            }
        }
        let missing: Vec<String> = needed.into_iter().filter(|p| !self.host.programs().found(p)).collect();
        let path = std::env::var("PATH").unwrap_or_default();
        self.submit::<StartDaemon>(Start { path: path.clone(), missing: missing.clone() }).await?;
        // Waiters of older binaries no longer start once this one runs.
        self.note_rules().await?;
        self.adopt_queued().await?;
        // The landing gates its stacks waited for ran in the process this one replaces.
        self.clear_stacks().await?;
        let freed = self.free_unstarted().await?;
        self.retire_clones()?;
        Ok(json!({ "path": path, "missing": missing, "freed": freed, "moved": moved }))
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
        self.typing_ended().await;
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
