//! A session's health, seen from outside: whether its brief is still to be
//! typed, whether the watch's session can take a round, and a worker's
//! observation, which the domain's health rules decide on: what goes wrong
//! goes to the watch; a used-up account is noted and the issue is started
//! again on another.

use std::path::Path;

use factorium_domain::health::{self, Observation, Observe, Sight, Verdict};
use factorium_domain::kit::{AccountNote, LimitAccount};
use factorium_domain::settings::Settings;
use factorium_domain::watch::{At, HandWatchBrief, Watch, Watches, watch_id};
use factorium_domain::worker::{self as w, Status, Workers};
use factum::{Uuid, Value};
use serde_json::json;

use super::Session;
use crate::{Answer, App};

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
        matches!(self.see(&s), Some(Sight::Closed(Some(_)))).then_some(s)
    }

    /// Looks at one live worker, whose processes hold `load` and are near
    /// idle if `quiet`: the observation is recorded and decided on in one
    /// command, and what it decided is carried out. Answers what it found,
    /// if anything.
    pub(crate) async fn worker_health(
        &self,
        worker: Uuid,
        load: health::Load,
        quiet: bool,
        settings: &Settings,
    ) -> Answer<Option<Value>> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.clone() else { return Ok(None) };
        if matches!(wk.status, Status::Ended | Status::Waiting) {
            return Ok(None);
        }
        self.hand_worker_brief(worker, false).await?;
        let Ok(session) = Self::worker_session(&st) else { return Ok(None) };
        let Some(sight) = self.see(&session) else { return Ok(None) };
        let evidence = self.dir(&["evidence", &format!("{worker}-{}.txt", self.now())]);
        let evidence = evidence.to_string_lossy().into_owned();
        let seen = Observation { worker, sight, load, quiet, evidence, settings: settings.clone() };
        match Verdict::of(&self.decide::<Observe>(seen).await?) {
            Verdict::Fine => Ok(None),
            Verdict::Waits { why, hot } => {
                if let Some(path) = hot {
                    self.write_evidence(worker, Path::new(&path));
                }
                Ok(Some(json!({ "worker": worker, "waits": why })))
            }
            Verdict::UsedUp { said, until } => {
                if let Some(account) = &st.account {
                    self.submit::<LimitAccount>(AccountNote { name: account.clone(), windows: Vec::new(), until })
                        .await?;
                }
                self.end_worker(worker, &format!("its account is used up: {said}"), false).await?;
                Ok(Some(json!({ "worker": worker, "ended": "quota", "said": said })))
            }
        }
    }

    /// Writes what the worker's processes are doing now at `path`.
    fn write_evidence(&self, worker: Uuid, path: &Path) {
        let procs: Vec<String> = self
            .host
            .processes()
            .into_iter()
            .filter(|p| p.worker.as_deref() == Some(&worker.to_string()))
            .map(|p| format!("{:>7} {:>7} {:>6.1}% {:>9}KiB {}", p.pid, p.ppid, p.cpu, p.rss_kib, p.command))
            .collect();
        let _ = std::fs::create_dir_all(path.parent().expect("a parent"));
        let _ = std::fs::write(path, format!("    pid    ppid    cpu       rss command\n{}\n", procs.join("\n")));
    }
}
