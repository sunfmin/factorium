//! Watching a session from outside: its terminal's screen and its session
//! files (the session file is the turn). One look answers what a session
//! is doing; a worker's health and the watch's readiness are each one call
//! on it. What goes wrong with a worker goes to the watch; a used-up account
//! is noted and the issue is started again on another.

use std::path::PathBuf;

use factorium_domain::hot;
use factorium_domain::kit::{AccountNote, LimitAccount};
use factorium_domain::settings::{Settings, Time};
use factorium_domain::watch::Watch;
use factorium_domain::worker::{self as w, Status, Workers};
use factorium_harness::{Harness, Home, Stop, Turn};
use factorium_host::Term;
use factum::{Uuid, Value};
use serde_json::json;

use crate::{Answer, App};

/// How long a used-up account is left alone when its screen gave no time.
const QUOTA_REST_MS: i64 = 60 * 60 * 1000;

/// One harness session, as it was started.
struct Session<'a> {
    terminal: &'a str,
    harness: Harness,
    account: Option<&'a str>,
    /// Its working directory, and the brief it was handed.
    cwd: &'a str,
    brief: &'a str,
}

/// What a session is doing, seen from outside.
enum Seen {
    /// Its terminal is gone.
    Gone,
    /// Orca did not answer: nothing is known.
    Unknown,
    /// Its account is used up: the line that says so.
    Quota(String),
    /// Its harness stopped: the line that says so.
    Stopped(String),
    /// A turn is open.
    Open,
    /// Its turn closed at this time, in ms; `None`: no session file says.
    Closed(Option<i64>),
}

impl App {
    /// Where a session of `harness` on `account` keeps its files.
    pub(crate) fn session_home(&self, harness: Harness, account: Option<&str>) -> Home {
        harness.home(&self.host.home(), account, &self.dir(&["kept", harness.key()]))
    }

    fn see(&self, s: &Session) -> Seen {
        let screen = match self.host.orca().screen(s.terminal) {
            Term::Running(screen) => screen,
            Term::Exited => return Seen::Gone,
            Term::Unreadable(_) => return Seen::Unknown,
        };
        match s.harness.stopped(&screen) {
            Some(Stop::Quota(said)) => return Seen::Quota(said),
            Some(Stop::Refused(said)) => return Seen::Stopped(said),
            None => {}
        }
        match s.harness.turn(&self.session_home(s.harness, s.account), s.cwd, s.brief) {
            Some(Turn::Open { .. }) => Seen::Open,
            Some(Turn::Closed { since, .. }) => Seen::Closed(Some(since as i64 * 1000)),
            None => Seen::Closed(None),
        }
    }

    /// Whether the watch's session, on `terminal`, is up and idle: it can
    /// take a round.
    pub(crate) fn watch_ready(&self, watch: &Watch, terminal: Option<&str>) -> bool {
        let (Some(terminal), Some(harness)) = (terminal, watch.harness.as_deref().and_then(Harness::read)) else {
            return false;
        };
        let (home, brief) = self.watch_home();
        let (cwd, brief) = (home.to_string_lossy(), brief.to_string_lossy());
        let session = Session { terminal, harness, account: watch.account.as_deref(), cwd: &cwd, brief: &brief };
        matches!(self.see(&session), Seen::Closed(Some(_)))
    }

    /// Looks at one live worker, whose processes hold `load`; answers what
    /// it found, if anything.
    pub(crate) async fn worker_health(
        &self,
        worker: Uuid,
        load: &hot::Load,
        settings: &Settings,
    ) -> Answer<Option<Value>> {
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
        let session = Session {
            terminal: &st.terminal,
            harness,
            account: st.account.as_deref(),
            cwd: &st.worktree,
            brief: &st.brief,
        };
        let seen = match self.see(&session) {
            Seen::Unknown => return Ok(None),
            Seen::Gone => return wait("its session's terminal is gone".into()).await,
            Seen::Quota(quota) => {
                if let Some(account) = &st.account {
                    let note = AccountNote { name: account.clone(), windows: Vec::new(), until: now + QUOTA_REST_MS };
                    self.submit::<LimitAccount>(note).await?;
                }
                self.end_worker(worker, &format!("its account is used up: {quota}"), false).await?;
                return Ok(Some(json!({ "worker": worker, "ended": "quota", "said": quota })));
            }
            Seen::Stopped(said) => return wait(format!("its harness stopped: {said}")).await,
            seen => seen,
        };
        // Hot: over its share for `hot_for`, by the clock the database keeps.
        let over = hot::over(load, settings);
        self.submit::<w::NoteLoad>(w::Load { worker, over: over.is_some() }).await?;
        let since = self.state::<Workers>(worker).await?.over_since;
        if let (Some(what), true) = (&over, hot::hot(since, now, settings)) {
            let evidence = self.evidence(worker);
            return wait(format!("hot: {what}; evidence in {}", evidence.display())).await;
        }
        // Silent: its turn ended, and no done, past the bound.
        let idle_since = match seen {
            Seen::Closed(at) => at.map_or(wk.since, |t| t.max(wk.since)),
            _ => return Ok(None),
        };
        if wk.status == Status::Working && now - idle_since > settings.ms(Time::SilentBound) {
            return wait(format!("silent for {} min without `factorium worker done`", (now - idle_since) / 60_000))
                .await;
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
