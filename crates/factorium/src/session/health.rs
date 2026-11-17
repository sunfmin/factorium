//! A session's health, seen from outside: whether its brief is still to be
//! typed, whether the watch's session can take a round, and what goes wrong
//! with a worker, which goes to the watch; a used-up account is noted and
//! the issue is started again on another.

use std::path::PathBuf;

use factorium_domain::hot;
use factorium_domain::kit::{AccountNote, LimitAccount};
use factorium_domain::settings::{Settings, Time};
use factorium_domain::watch::{At, HandWatchBrief, Watch, Watches, watch_id};
use factorium_domain::worker::{self as w, Status, Workers};
use factum::{Uuid, Value};
use serde_json::json;

use super::{Seen, Session};
use crate::{Answer, App};

/// How long a used-up account is left alone when its screen gave no time.
const QUOTA_REST_MS: i64 = 60 * 60 * 1000;

/// How long a session's harness has to come up before its worker goes to
/// the watch.
const UP_BOUND_MS: i64 = 5 * 60 * 1000;

impl App {
    /// Hands a worker its brief if it is still to be typed and its harness
    /// is up; `wait`: first gives the harness up to a minute to settle.
    pub(crate) async fn hand_worker_brief(&self, worker: Uuid, wait: bool) -> Answer<()> {
        let wk = self.state::<Workers>(worker).await?;
        let (Some(st), Some(typed)) = (&wk.started, wk.unhanded()) else { return Ok(()) };
        if wait {
            self.settle(&st.terminal);
        }
        match self.type_at(&Self::worker_session(st)?, typed) {
            Ok(()) => self.submit::<w::HandBrief>(w::Of { worker }).await,
            // Typed once the harness is up, on a later look.
            Err(r) if r.code == "not_up" || !wait => Ok(()),
            // Up, yet Orca would not type at it: the start fails, and what
            // factorium opened, factorium closes.
            Err(r) => {
                self.end_worker(worker, &format!("its brief could not be typed: {}", r.message), true).await?;
                Err(r)
            }
        }
    }

    /// Hands the watch's session its brief if it is still to be typed and
    /// its harness is up; `wait` as for a worker.
    pub(crate) async fn hand_watch_brief(&self, wait: bool) -> Answer<()> {
        let watch = self.state::<Watches>(watch_id()).await?;
        let (Some(s), Some(typed)) = (self.watch_session(&watch), &watch.unhanded) else { return Ok(()) };
        if wait {
            self.settle(&s.terminal);
        }
        if self.type_at(&s, typed).is_ok() {
            self.submit::<HandWatchBrief>(At { terminal: s.terminal.clone() }).await?;
        }
        Ok(())
    }

    /// The watch's session, when it is up, handed its brief, and idle: it
    /// can take a round.
    pub(crate) fn watch_ready(&self, watch: &Watch) -> Option<Session> {
        let s = self.watch_session(watch).filter(|_| watch.unhanded.is_none())?;
        matches!(self.see(&s), Seen::Closed(Some(_))).then_some(s)
    }

    /// Hung: its turn open, its processes near idle (`quiet`) and its
    /// screen unchanged past the silent bound — a command blocked on what
    /// never comes. Answers why it waits, if it does.
    async fn hung(
        &self,
        wk: &w::Worker,
        worker: Uuid,
        seen: &Seen,
        quiet: bool,
        s: &Settings,
    ) -> Answer<Option<String>> {
        let screen = match seen {
            Seen::Open(h) if quiet && wk.status == Status::Working => Some(*h),
            _ => None,
        };
        self.submit::<w::NoteQuiet>(w::Quiet { worker, screen }).await?;
        let since = self.state::<Workers>(worker).await?.quiet.map(|(_, at)| at);
        let now = self.now();
        Ok(since.filter(|t| now - t > s.ms(Time::SilentBound)).map(|t| {
            format!(
                "hung for {} min: its turn is open, its processes use next to no CPU and its screen has not changed",
                (now - t) / 60_000
            )
        }))
    }

    /// Looks at one live worker, whose processes hold `load` and are near
    /// idle if `quiet`; answers what it found, if anything.
    pub(crate) async fn worker_health(
        &self,
        worker: Uuid,
        load: &hot::Load,
        quiet: bool,
        settings: &Settings,
    ) -> Answer<Option<Value>> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.clone() else { return Ok(None) };
        if matches!(wk.status, Status::Ended | Status::Waiting) {
            return Ok(None);
        }
        self.hand_worker_brief(worker, false).await?;
        let now = self.now();
        let wait = |why: String| async move {
            self.submit::<w::Wait>(w::Because { worker, why: why.clone() }).await?;
            Ok::<_, crate::Refusal>(Some(json!({ "worker": worker, "waits": why })))
        };
        let Ok(session) = Self::worker_session(&st) else { return Ok(None) };
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
            Seen::Down(why) if now - st.at > UP_BOUND_MS => {
                let after = (now - st.at) / 60_000;
                return wait(format!("its harness is not up {after} min after it started: {why}")).await;
            }
            Seen::Down(_) => return Ok(None),
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
        if let Some(why) = self.hung(&wk, worker, &seen, quiet, settings).await? {
            return wait(why).await;
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
