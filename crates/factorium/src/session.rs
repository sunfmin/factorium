//! Watching a worker's session from outside: its terminal's screen, its
//! session files (the session file is the turn), and its processes' load.
//! What goes wrong goes to the watch; a used-up account is noted and the
//! issue is started again on another.

use std::path::PathBuf;

use factorium_domain::hot;
use factorium_domain::kit::{AccountNote, LimitAccount};
use factorium_domain::settings::{Settings, Time};
use factorium_domain::worker::{self as w, Status, Workers};
use factorium_harness::{Harness, Stop, Turn};
use factum::{Uuid, Value};
use serde_json::json;

use crate::{Answer, App};

/// How long a used-up account is left alone when its screen gave no time.
const QUOTA_REST_MS: i64 = 60 * 60 * 1000;

impl App {
    /// The turn a harness's session files record for the worktree `cwd` of
    /// the session handed `brief`. `None`: no session says.
    pub(crate) fn turn_of(&self, harness: Harness, cwd: &str, brief: &str) -> Option<Turn> {
        harness.turn(&self.host.home(), &self.dir(&["kept", harness.key()]), cwd, brief)
    }

    /// Looks at one live worker; answers what it found, if anything.
    pub(crate) async fn observe(&self, worker: Uuid, load: &hot::Load, settings: &Settings) -> Answer<Option<Value>> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.clone() else { return Ok(None) };
        if matches!(wk.status, Status::Ended | Status::Waiting) {
            return Ok(None);
        }
        let harness = Harness::read(&st.harness).unwrap_or(Harness::ClaudeCode);
        let now = self.now();
        let wait = |why: String| async move {
            self.submit::<w::Wait>(w::Because { worker, why: why.clone() }).await?;
            Ok::<_, crate::Refusal>(Some(json!({ "worker": worker, "waits": why })))
        };
        let Some(screen) = self.host.orca().screen(&st.terminal) else {
            return wait("its session's terminal is gone".into()).await;
        };
        let stopped = harness.stopped(&screen);
        if let Some(Stop::Quota(quota)) = &stopped {
            if let Some(account) = &st.account {
                let note = AccountNote { name: account.clone(), windows: Vec::new(), until: now + QUOTA_REST_MS };
                self.submit::<LimitAccount>(note).await?;
            }
            self.end_worker(worker, &format!("its account is used up: {quota}"), false).await?;
            return Ok(Some(json!({ "worker": worker, "ended": "quota", "said": quota })));
        }
        if let Some(Stop::Refused(said)) = stopped {
            return wait(format!("its harness stopped: {said}")).await;
        }
        // Hot: over its share for `hot_for`.
        let over = hot::over(load, settings);
        let since = {
            let mut m = self.memory.lock().unwrap_or_else(|p| p.into_inner());
            match &over {
                Some(_) => Some(*m.over.entry(worker).or_insert(now)),
                None => m.over.remove(&worker).and(None),
            }
        };
        if let (Some(what), true) = (&over, hot::hot(since, now, settings)) {
            let evidence = self.evidence(worker);
            self.memory.lock().unwrap_or_else(|p| p.into_inner()).over.remove(&worker);
            return wait(format!("hot: {what}; evidence in {}", evidence.display())).await;
        }
        // Silent: its turn ended, and no done, past the bound.
        if wk.status == Status::Working {
            let idle_since = match self.turn_of(harness, &st.worktree, &st.brief) {
                Some(Turn::Closed { since, .. }) => Some((since as i64 * 1000).max(wk.since)),
                Some(Turn::Open { .. }) => None,
                None => Some(wk.since),
            };
            if let Some(t) = idle_since.filter(|t| now - t > settings.ms(Time::SilentBound)) {
                return wait(format!("silent for {} min without `factorium worker done`", (now - t) / 60_000)).await;
            }
        }
        Ok(None)
    }

    /// Writes what the worker's processes are doing now; answers where.
    fn evidence(&self, worker: Uuid) -> PathBuf {
        let path = self.dir(&["evidence", &format!("{worker}-{}.txt", self.now())]);
        let procs: Vec<String> = self
            .host
            .processes()
            .into_iter()
            .filter(|p| p.worker.as_deref() == Some(&worker.to_string()))
            .map(|p| format!("{:>7} {:>7} {:>6.1}% {:>9}KiB {}", p.pid, p.ppid, p.cpu, p.rss_kib, p.command))
            .collect();
        let _ = std::fs::create_dir_all(path.parent().expect("a parent"));
        let _ = std::fs::write(&path, format!("    pid    ppid    cpu       rss command\n{}\n", procs.join("\n")));
        path
    }
}
