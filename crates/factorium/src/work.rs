//! A worker's own commands: `factorium worker gate|done|escalate|exclusive`.
//! The worker is named by `FACTORIUM_WORKER`, which its session was
//! started with.

use std::path::Path;
use std::time::Duration;

use factorium_domain::declared::Kind;
use factorium_domain::gate::{Gates, Ran as GateRan, RecordGate, record_id};
use factorium_domain::issue::{ReleaseIssue, Take};
use factorium_domain::settings::Time;
use factorium_domain::worker::{self as w, Started, Status, Workers};
use factorium_host::Git;
use factum::Uuid;
use serde_json::json;

use crate::{Answer, App, Refusal, fail};

impl App {
    async fn started(&self, worker: Uuid) -> Answer<(w::Worker, Started)> {
        let wk = self.state::<Workers>(worker).await?;
        let st = wk.started.clone().ok_or_else(|| Refusal::new("no_worker", format!("no worker {worker}")))?;
        if wk.status == Status::Ended {
            return Err(Refusal::new("ended", "this worker has ended"));
        }
        Ok((wk, st))
    }

    /// Runs the fast gate on the worktree as it stands. A clean worktree's
    /// green is recorded against its tree, so landing does not run it again
    /// on that tree.
    pub async fn worker_gate(&self, worker: Uuid) -> Answer {
        let (_, st) = self.started(worker).await?;
        let git = Git::at(&st.worktree);
        let declared = self.declared(&git, "HEAD")?;
        let (cmd, limit) = declared.gate.of(Kind::Fast).expect("a fast gate");
        let dirty = git.dirty().map_err(fail("git"))?;
        let tree = git.tree("HEAD").map_err(fail("git"))?;
        if !dirty && self.state::<Gates>(record_id(&st.checkout, &tree, Kind::Fast)).await?.green {
            return Ok(json!({ "green": true, "tree": tree, "recorded": true, "ran": false }));
        }
        let what = format!("fast gate of {}#{}", st.checkout, st.number);
        let ran =
            self.heavy(&what, Some(worker), Path::new(&st.worktree), cmd, &[], Duration::from_secs(limit)).await?;
        if !dirty {
            let record = GateRan {
                checkout: st.checkout.clone(),
                tree: tree.clone(),
                kind: Kind::Fast,
                green: ran.ok,
                took_ms: ran.took_ms,
                tail: ran.tail.clone(),
            };
            self.submit::<RecordGate>(record).await?;
        }
        Ok(
            json!({ "green": ran.ok, "tree": tree, "recorded": !dirty, "took_ms": ran.took_ms, "tail": ran.tail, "timed_out": ran.timed_out }),
        )
    }

    /// Asks to land: the worktree must be committed, with commits of its own.
    pub async fn worker_done(&self, worker: Uuid) -> Answer {
        let (_, st) = self.started(worker).await?;
        let git = Git::at(&st.worktree);
        if git.dirty().map_err(fail("git"))? {
            return Err(Refusal::new("dirty", "commit or remove your changes first: landing takes only commits"));
        }
        if git.ahead(&st.base, "HEAD").map_err(fail("git"))? == 0 {
            return Err(Refusal::new("nothing", "your branch has no commits to land"));
        }
        if git.branch().as_deref() != Some(st.branch.as_str()) {
            return Err(Refusal::new(
                "off_branch",
                format!("check out your branch {} again: landing takes it", st.branch),
            ));
        }
        self.submit::<w::AskLanding>(w::Of { worker }).await?;
        Ok(
            json!({ "landing": true, "said": "Landing is queued. Stop here: if it is red you will be told why in this terminal; if it is green your session ends." }),
        )
    }

    /// Hands the worker to the watch.
    pub async fn worker_escalate(&self, worker: Uuid, why: &str) -> Answer {
        if why.trim().is_empty() {
            return Err(Refusal::new("no_reason", "say what you need"));
        }
        self.started(worker).await?;
        self.submit::<w::Wait>(w::Because { worker, why: format!("escalated: {why}") }).await?;
        Ok(json!({ "escalated": true, "said": "The watch will answer in this terminal. Stop until it does." }))
    }

    /// Runs a command through the heavy-command queue.
    pub async fn worker_exclusive(&self, worker: Option<Uuid>, dir: &Path, cmd: &str) -> Answer {
        let limit = Duration::from_millis(self.settings().await?.ms(Time::HoldBound).max(60_000) as u64 * 4);
        let ran = self.heavy(cmd, worker, dir, cmd, &[], limit).await?;
        Ok(json!({ "ok": ran.ok, "code": ran.code, "took_ms": ran.took_ms, "tail": ran.tail }))
    }

    /// Ends a worker and frees its issue; its worktree goes if `remove` and
    /// it holds nothing that is nowhere else.
    pub(crate) async fn end_worker(&self, worker: Uuid, why: &str, remove: bool) -> Answer<()> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started else { return Ok(()) };
        if wk.status != Status::Ended {
            self.submit::<w::EndWorker>(w::Because { worker, why: why.into() }).await?;
        }
        let take = Take { checkout: st.checkout.clone(), number: st.number, worker };
        match self.submit::<ReleaseIssue>(take).await {
            Ok(()) => {}
            Err(r) if r.code == "no_issue" || r.code == "ended" => {}
            Err(r) => return Err(r),
        }
        self.release_worker(worker, remove).await
    }
}
