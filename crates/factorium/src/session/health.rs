//! A session's health, seen from outside: whether the watch's session can
//! take a round, and a worker's
//! observation, which the domain's health rules decide on: what goes wrong
//! goes to the watch; a used-up account is noted and the issue is started
//! again on another.

use std::path::Path;

use factorium_domain::health::{Observation, Observe, Sight, Verdict};
use factorium_domain::kit::{AccountNote, LimitAccount};
use factorium_domain::settings::Settings;
use factorium_domain::watch::Watch;
use factum::{Uuid, Value};
use serde_json::json;

use super::Session;
use crate::life::End;
use crate::load::Share;
use crate::{Answer, App, Want};

impl App {
    /// The watch's session, when it is up, handed its brief, and idle: it
    /// can take a round.
    pub(crate) async fn watch_ready(&self, watch: &Watch) -> Answer<Option<Session>> {
        let Some(s) = self.watch_session(watch) else { return Ok(None) };
        let handed = self.outbox(&s).await?.held().is_none();
        Ok((handed && matches!(self.see(&s), Some(Sight::Closed(Some(_))))).then_some(s))
    }

    /// Looks at one live worker, with its `share` of the host: the
    /// observation is recorded and decided on in one command, and what it
    /// decided is carried out. Answers what it found, if anything.
    pub(crate) async fn worker_health(&self, worker: Uuid, share: Share, s: &Settings) -> Answer<Option<Value>> {
        let Some(live) = self.live_or_none(worker, Want::Any).await? else { return Ok(None) };
        // Its brief held is tried again even while it waits: it is its brief.
        self.retry(&live.session).await?;
        if self.live_or_none(worker, Want::Working).await?.is_none() {
            return Ok(None);
        }
        let Some(sight) = self.see(&live.session) else { return Ok(None) };
        let evidence = self.data().evidence(worker, self.now()).to_string_lossy().into_owned();
        let Share { load, quiet, heavy } = share;
        let seen = Observation { worker, sight, load, quiet, heavy, evidence, settings: s.clone() };
        match Verdict::of(&self.decide::<Observe>(seen).await?) {
            Verdict::Fine => Ok(None),
            Verdict::Waits { why, hot } => {
                if let Some(path) = hot {
                    self.write_evidence(worker, Path::new(&path));
                }
                Ok(Some(json!({ "worker": worker, "waits": why })))
            }
            Verdict::UsedUp { said, until } => {
                if let Some(account) = &live.started.account {
                    self.submit::<LimitAccount>(AccountNote { name: account.clone(), until }).await?;
                }
                self.end_worker(worker, End::because(format!("its account is used up: {said}"), true)).await?;
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
