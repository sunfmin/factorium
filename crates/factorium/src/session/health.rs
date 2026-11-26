//! A session's health, seen from outside: whether the watch's session can
//! take a round, and a worker's
//! observation, which the domain's health rules decide on: what goes wrong
//! goes to the watch; a used-up account is told to stablehand and the issue
//! is started again on another; an engine at its limit parks its worker
//! until the reset, told to stablehand of its account, else noted on its
//! harness.

use std::path::Path;

use factorium_domain::health::{Observation, Observe, Sight, Verdict};
use factorium_domain::kit::{HarnessNote, LimitHarness};
use factorium_domain::outbox::{HoldTaken, HoldTakenMessages};
use factorium_domain::settings::Settings;
use factorium_domain::watch::Watch;
use factorium_domain::worker::Started;
use factum::{Uuid, Value};
use serde_json::json;

use super::Session;
use crate::life::End;
use crate::load::Share;
use crate::work::Live;
use crate::{Answer, App, Want};

/// How long before its limit is first seen a message its engine took may
/// have met it instead of being read (agy ends that turn at once on its
/// quota): one submitted since then is held again, ms.
const TAKEN_MS: i64 = 10 * 60 * 1000;

impl App {
    /// The watch's session, when it is up, handed its brief, and idle: it
    /// can take a round.
    pub(crate) async fn watch_ready(&self, watch: &Watch) -> Answer<Option<Session>> {
        let Some(s) = self.watch_session(watch) else { return Ok(None) };
        let handed = self.outbox(&s).await?.held().is_none();
        Ok((handed && matches!(self.see(&s, None), Some(Sight::Closed(Some(_))))).then_some(s))
    }

    /// Tries again what each live worker's session holds, the sessions at
    /// once (factorium#279), before [`App::workers_health`] looks at them:
    /// a brief held is tried again even while its worker waits, as it is
    /// its brief.
    pub(crate) async fn retry_workers(&self, workers: &[Uuid]) -> Answer<()> {
        let mut sessions = Vec::new();
        for &worker in workers {
            sessions.extend(self.live_or_none(worker, Want::Any).await?.map(|live| live.session));
        }
        self.retry(&sessions.iter().collect::<Vec<_>>()).await
    }

    /// Looks at each live worker of `workers`, with its `share` of the
    /// host: their screens read at once, each on a thread of
    /// its own, so one Orca is slow to answer holds up the tick by one read,
    /// not by all; then each in turn, in `workers`' order, as
    /// [`App::worker_health`] decides. Answers what it found, if anything.
    pub(crate) async fn workers_health(
        &self,
        workers: &[Uuid],
        share: impl Fn(Uuid) -> Share,
        s: &Settings,
    ) -> Answer<Vec<Value>> {
        let mut lives = Vec::new();
        for &worker in workers {
            lives.extend(self.live_or_none(worker, Want::Any).await?.map(|live| (worker, live)));
        }
        // Orca did not answer one: nothing is known of it this tick.
        let sights: Vec<(Uuid, Live, Sight)> = std::thread::scope(|scope| {
            let reading: Vec<_> = lives
                .into_iter()
                .map(|(worker, live)| {
                    scope.spawn(move || {
                        let past = live.limit.as_ref().map(|l| l.said.as_str());
                        let sight = self.see(&live.session, past)?;
                        Some((worker, live, sight))
                    })
                })
                .collect();
            reading.into_iter().filter_map(|r| r.join().expect("a screen read does not panic")).collect()
        });
        let mut seen = Vec::new();
        for (worker, live, sight) in sights {
            seen.extend(self.worker_health(worker, live, sight, share(worker), s).await?);
        }
        Ok(seen)
    }

    /// Decides on one live worker, `live`, as its screen showed `sight`,
    /// with its `share` of the host: the observation is recorded and
    /// decided on in one command, and what it decided is carried out. One
    /// waiting for the watch is looked at too: its engine may be spent.
    /// Answers what it found, if anything.
    async fn worker_health(
        &self,
        worker: Uuid,
        live: Live,
        sight: Sight,
        share: Share,
        s: &Settings,
    ) -> Answer<Option<Value>> {
        let evidence = self.data().evidence(worker, self.now()).to_string_lossy().into_owned();
        let Share { load, quiet, heavy } = share;
        let issue = self.issue(&live.started.checkout, live.started.number).await?;
        let blocked = self.blockers(&issue).await?.iter().any(|w| w.met.is_none());
        let seen = Observation { worker, sight, load, quiet, heavy, blocked, evidence, settings: s.clone() };
        match Verdict::of(&self.decide::<Observe>(seen).await?) {
            Verdict::Fine => Ok(None),
            Verdict::Waits { why, hot } => {
                if let Some(path) = hot {
                    self.write_evidence(worker, Path::new(&path));
                }
                Ok(Some(json!({ "worker": worker, "waits": why })))
            }
            Verdict::UsedUp { said, until } => {
                let unbooked = self.rest(&live.started, until, false).await?;
                self.end_worker(worker, End::because(format!("its account is used up: {said}"), true)).await?;
                Ok(Some(unbooked_in(json!({ "worker": worker, "ended": "quota", "said": said }), unbooked)))
            }
            // Parked until the reset; its account, or its harness on the host's
            // own login, starts no worker before it.
            Verdict::Limited { said, until } => {
                let unbooked = self.rest(&live.started, until, true).await?;
                // What it took just before is no proof it read it: held again.
                let submitted = live.since.max(self.now() - TAKEN_MS);
                let terminal = live.session.terminal().to_string();
                let taken = HoldTaken { terminal, submitted, queued: live.since, until };
                self.submit::<HoldTakenMessages>(taken).await?;
                Ok(Some(unbooked_in(json!({ "worker": worker, "limited": until, "said": said }), unbooked)))
            }
        }
    }

    /// Worker `st`'s account met its limit: stablehand is told so
    /// (`stablehand limit`), until `until` (ms) where the screen said when it
    /// `resets`, else until stablehand next reads the account; its harness
    /// on the host's own login starts no worker before `until`. Answers why
    /// stablehand could not be told, if it could not: the worker's end
    /// does not wait on it.
    async fn rest(&self, st: &Started, until: i64, resets: bool) -> Answer<Option<String>> {
        match &st.account {
            Some(account) => Ok(self.report_limit(account, resets.then_some(until)).err()),
            None => {
                self.submit::<LimitHarness>(HarnessNote { harness: st.harness.clone(), until }).await?;
                Ok(None)
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

/// What a tick saw of a worker, with why stablehand could not be told of
/// its account's limit (`unbooked`), if it could not.
fn unbooked_in(mut seen: Value, unbooked: Option<String>) -> Value {
    if let Some(why) = unbooked {
        seen["unbooked"] = json!(why);
    }
    seen
}
