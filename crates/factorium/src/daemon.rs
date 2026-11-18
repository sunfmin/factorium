//! The daemon's tick: one pass over everything that moves without a
//! command — workers watched, landings, promotions, cuts, one start if
//! admission allows, and the watch's look.

use factorium_domain::admission;
use factorium_domain::checkout::Checkout;
use factorium_domain::daemon::{NoteTick, Start, StartDaemon, Ticked};
use factorium_domain::health;
use factorium_domain::heavy::{Heavy, queue_id};
use factorium_domain::issue::Status as IssueStatus;
use factorium_domain::kit::{Kits, kit_id};
use factorium_domain::line::Move;
use factorium_domain::settings::{Settings, Time};
use factorium_domain::tracker::{self, WorkerRow};
use factorium_domain::watch::{Held, NoteAdmission};
use factum::Value;
use serde::Serialize;
use serde_json::json;

use crate::{Answer, App, LineClone, Refusal};

/// What one tick did.
#[derive(Debug, Default, Serialize)]
pub struct Tick {
    pub seen: Vec<Value>,
    pub landed: Vec<Value>,
    pub lines: Vec<Value>,
    pub started: Option<Value>,
    /// Why nothing started, when something was ready.
    pub held: Option<String>,
    pub watch: Option<Value>,
    /// The binary to run in place of this one once the tick ends: Homebrew
    /// links another version of the daemon's own formula, whoever cut it.
    pub restart: Option<String>,
}

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
        let done = self.pass().await;
        let error = done.as_ref().err().map(Refusal::to_string);
        self.submit::<NoteTick>(Ticked { error }).await?;
        if done.is_err() {
            let settings = self.settings().await.unwrap_or_default();
            if let Err(e) = self.look_if_due(&settings).await {
                eprintln!("look: {e}");
            }
        }
        done
    }

    async fn pass(&self) -> Answer<Tick> {
        let mut t = Tick::default();
        self.engine.settle().await?;
        let settings = self.settings().await?;
        // Workers: their sessions, and their share of the host.
        let procs = self.host.processes();
        let queue = self.state::<Heavy>(queue_id()).await?;
        for w in self.live_workers().await? {
            let id = w.worker;
            // What runs in its heavy slot is the queue's to bound, not its share's.
            let slot = queue.entries.iter().filter(|e| e.worker == Some(id) && e.started_at.is_some());
            let held = descendants(&procs, slot.map(|e| e.pid).collect());
            let mine: Vec<_> = procs
                .iter()
                .filter(|p| p.worker.as_deref() == Some(&id.to_string()) && !held.contains(&p.pid))
                .collect();
            let core_percent = mine.iter().map(|p| p.cpu).sum::<f64>();
            let rss_kib = mine.iter().map(|p| p.rss_kib).sum();
            let load = health::Load::of(core_percent, rss_kib, mine.len() as u64, self.cores());
            if let Some(seen) = self.worker_health(id, load, health::quiet(core_percent), &settings).await? {
                t.seen.push(seen);
            }
        }
        // The lines: each checkout's landing, then its promotion and cut,
        // as its line says each is due.
        let checkouts = self.listed().await?;
        (t.landed, t.lines) = self
            .asking_once(async {
                let (mut landed, mut lines) = (Vec::new(), Vec::new());
                for c in &checkouts {
                    if let Ok(did) = self.carry(c, Move::Land, false).await? {
                        landed.push(did);
                    }
                }
                for c in &checkouts {
                    for mv in [Move::Promote, Move::Cut] {
                        if let Ok(did) = self.carry(c, mv, false).await? {
                            lines.push(did);
                        }
                    }
                    // The local base follows origin's as last fetched, whatever left it behind.
                    let git = self.host.git(&c.path);
                    lines.extend(crate::line::follow_base(c, &git, &format!("refs/remotes/origin/{}", c.base)));
                }
                Ok((landed, lines))
            })
            .await?;
        // One start, if admission allows.
        if let Some((checkout, number)) = self.next_issue().await? {
            let reading = self.reading();
            let held = admission::admit(&reading, &settings);
            let held = match held {
                Ok(()) => match self.start_worker(&checkout, number).await {
                    Ok(started) => {
                        t.started = Some(started);
                        None
                    }
                    Err(r) if r.code == "no_harness" => Some(format!("kit: {}", r.message)),
                    Err(r) => return Err(r),
                },
                Err(why) => Some(why),
            };
            self.submit::<NoteAdmission>(Held { why: held.clone() }).await?;
            t.held = held;
        } else {
            self.submit::<NoteAdmission>(Held { why: None }).await?;
        }
        if admission::disk_short(&self.reading(), &settings) {
            self.reclaim();
        }
        t.watch = self.look_if_due(&settings).await?;
        t.restart = self.swapped().map(|b| b.to_string_lossy().into_owned());
        Ok(t)
    }

    /// The daemon runs the binary `me` (canonical): if Homebrew installed it,
    /// each tick asks whether its link now names another version.
    pub fn runs_as(&self, me: &std::path::Path) {
        let link = self.host.brew().link_of(me);
        self.memory.lock().unwrap().running = link.map(|l| (me.to_path_buf(), l));
    }

    /// The link to exec in place of this binary: Homebrew links a version
    /// other than the one running, by a cut from the daemon, the CLI or
    /// by hand. Mid-upgrade, when the link resolves to nothing, not yet.
    fn swapped(&self) -> Option<std::path::PathBuf> {
        let (me, link) = self.memory.lock().unwrap().running.clone()?;
        let now = link.canonicalize().ok()?;
        (now != me).then_some(link)
    }

    /// The oldest ready issue no worker holds and nothing open blocks.
    async fn next_issue(&self) -> Answer<Option<(String, u64)>> {
        let mut checked = std::collections::BTreeSet::new();
        for r in self.rows(tracker::untaken()).await? {
            let (checkout, number) = (r.checkout, r.number);
            if !checked.contains(&checkout) && self.checkout(&checkout).await.is_err() {
                continue;
            }
            checked.insert(checkout.clone());
            let issue = self.issue(&checkout, number).await?;
            let mut open = false;
            for b in &issue.blocked_by {
                open |= self.issue(&checkout, *b).await.is_ok_and(|i| i.status != IssueStatus::Closed);
            }
            if !open {
                return Ok(Some((checkout, number)));
            }
        }
        Ok(None)
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
    /// once, and what is missing is recorded for doctor, not met tick by tick.
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
        Ok(json!({ "path": path, "missing": missing }))
    }

    /// `factorium run`: ticks until stopped. One daemon runs: another that
    /// is alive is refused.
    pub async fn run(&self, stop: &std::sync::atomic::AtomicBool) -> Answer {
        let pid = self.data().pid();
        if let Some(n) = self.daemon_pid()
            && n != std::process::id()
        {
            return Err(Refusal::new("running", format!("a daemon runs already, as pid {n}")));
        }
        std::fs::write(&pid, std::process::id().to_string()).map_err(|e| Refusal::new("io", e.to_string()))?;
        if let Ok(me) = std::env::current_exe().and_then(|e| e.canonicalize()) {
            self.runs_as(&me);
        }
        eprintln!("started: {}", self.daemon_start().await?);
        let mut ticks = 0u64;
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            match self.tick().await {
                Ok(t) => {
                    if t.started.is_some() || !t.landed.is_empty() || !t.lines.is_empty() || !t.seen.is_empty() {
                        println!("{}", serde_json::to_string(&t).unwrap_or_default());
                    }
                    if let Some(binary) = &t.restart {
                        // Between ticks, as the same pid: the pid file stays right.
                        use std::os::unix::process::CommandExt;
                        let e = std::process::Command::new(binary).args(std::env::args_os().skip(1)).exec();
                        eprintln!("restart as {binary}: {e}");
                    }
                }
                Err(e) => eprintln!("tick: {e}"),
            }
            ticks += 1;
            let every = self
                .settings()
                .await
                .map(|s| s.ms(Time::TickEvery))
                .unwrap_or_else(|_| Settings::default().ms(Time::TickEvery));
            tokio::time::sleep(std::time::Duration::from_millis(every as u64)).await;
        }
        let _ = std::fs::remove_file(&pid);
        Ok(json!({ "stopped": true, "ticks": ticks }))
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
            let _ = std::fs::remove_file(self.data().pid());
            json!({ "stopped": false, "why": "no daemon runs" })
        };
        answer["watch_closed"] = json!(self.close_watch_session().await?);
        Ok(answer)
    }
}

/// `roots` and every process under them.
fn descendants(procs: &[factorium_host::Proc], mut roots: Vec<u32>) -> std::collections::BTreeSet<u32> {
    let mut all = std::collections::BTreeSet::new();
    while let Some(pid) = roots.pop() {
        if all.insert(pid) {
            roots.extend(procs.iter().filter(|p| p.ppid == pid).map(|p| p.pid));
        }
    }
    all
}
