//! The daemon's tick: one pass over everything that moves without a
//! command — workers watched, landings, promotions, cuts, one start if
//! admission allows, and the watch's look.

use factorium_domain::admission;
use factorium_domain::checkout::Checkout;
use factorium_domain::hot;
use factorium_domain::issue::Status as IssueStatus;
use factorium_domain::settings::{Settings, Time};
use factorium_domain::tracker::{self, WorkerRow};
use factorium_domain::watch::{Held, NoteAdmission};
use factorium_domain::worker::Status as WorkerStatus;
use factum::Value;
use serde::Serialize;
use serde_json::json;

use crate::{Answer, App};

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
    /// The binary to run in place of this one once the tick ends: a cut
    /// installed the daemon's own formula.
    pub restart: Option<String>,
}

impl App {
    async fn listed(&self) -> Answer<Vec<Checkout>> {
        self.rows(tracker::listed()).await
    }

    async fn live_workers(&self) -> Answer<Vec<WorkerRow>> {
        self.rows(tracker::workers(None, false)).await
    }

    /// One pass of the daemon.
    pub async fn tick(&self) -> Answer<Tick> {
        let mut t = Tick::default();
        self.engine.settle().await?;
        let settings = self.settings().await?;
        // Workers: their sessions, and their share of the host.
        let procs = self.host.processes();
        for w in self.live_workers().await? {
            let id = w.worker;
            let mine: Vec<_> = procs.iter().filter(|p| p.worker.as_deref() == Some(&id.to_string())).collect();
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
            let load = hot::Load {
                cpu: mine.iter().map(|p| p.cpu).sum::<f64>() / cores,
                rss_kib: mine.iter().map(|p| p.rss_kib).sum(),
                procs: mine.len() as u64,
            };
            if let Some(seen) = self.observe(id, &load, &settings).await? {
                t.seen.push(seen);
            }
        }
        // Landings, one per checkout, oldest first.
        let workers = self.live_workers().await?;
        let checkouts = self.listed().await?;
        for c in &checkouts {
            let landing = workers.iter().find(|w| w.checkout == c.name && w.state.status == WorkerStatus::Landing);
            if let Some(w) = landing
                && let Some(did) = self.land(w.worker).await?
            {
                t.landed.push(did);
            }
        }
        // Promotions and cuts, when the checkout is idle or a bound passed.
        let workers = self.live_workers().await?;
        for c in &checkouts {
            let idle = !workers.iter().any(|w| w.checkout == c.name && w.state.status != WorkerStatus::Waiting);
            if let Some(did) = self.promote_if_due(c, idle, false).await? {
                t.lines.push(did);
            }
            if let Some(did) = self.cut_if_due(c, idle, false).await? {
                t.lines.push(did);
            }
        }
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
        t.restart = t.lines.iter().find_map(|l| l["restart"].as_str().map(String::from));
        Ok(t)
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
        for kind in ["landing", "promotion", "cut"] {
            let Ok(dirs) = std::fs::read_dir(self.dir(&[kind])) else { continue };
            for d in dirs.flatten() {
                let _ = std::fs::remove_dir_all(d.path().join("target"));
            }
        }
    }

    /// `factorium run`: ticks until stopped.
    pub async fn run(&self, stop: &std::sync::atomic::AtomicBool) -> Answer {
        let pid = self.dir(&["daemon.pid"]);
        std::fs::write(&pid, std::process::id().to_string()).map_err(|e| crate::Refusal::new("io", e.to_string()))?;
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

    /// `factorium stop`: ends the running daemon.
    pub fn stop(&self) -> Answer {
        let pid = self.dir(&["daemon.pid"]);
        let Ok(said) = std::fs::read_to_string(&pid) else {
            return Ok(json!({ "stopped": false, "why": "no daemon runs" }));
        };
        let n: u32 = said.trim().parse().unwrap_or(0);
        if n == 0 || !self.host.alive(n) {
            let _ = std::fs::remove_file(&pid);
            return Ok(json!({ "stopped": false, "why": "no daemon runs" }));
        }
        let _ = std::process::Command::new("kill").arg(n.to_string()).status();
        Ok(json!({ "stopped": true, "pid": n }))
    }
}
