//! A worker's own commands: `factorium worker gate|done|escalate|comment|file|exclusive`.
//! The worker is named by `FACTORIUM_WORKER`, which its session was
//! started with.

use std::path::Path;

use factorium_domain::declared::Kind;
use factorium_domain::issue::{Author, Blocker};
use factorium_domain::worker::{self as w, Started, Status, Workers};
use factum::Uuid;
use serde_json::json;

use crate::gate::At;
use crate::heavy::Asker;
use crate::session::Session;
use crate::{Answer, App, Refusal, fail};

/// Which live workers [`App::live`] accepts: never one not started
/// (`no_worker`) or ended (`ended`); `Working`, not one waiting (`waiting`).
pub(crate) enum Want {
    Working,
    Any,
}

/// A live worker as one read saw it.
pub(crate) struct Live {
    pub(crate) started: Started,
    pub(crate) session: Session,
    pub(crate) waiting: bool,
}

impl App {
    /// Worker `worker`, live as `want` accepts it, with its session: one read.
    pub(crate) async fn live(&self, worker: Uuid, want: Want) -> Answer<Live> {
        let wk = self.state::<Workers>(worker).await?;
        let started = wk.started.ok_or_else(|| Refusal::new("no_worker", format!("no worker {worker}")))?;
        let waiting = wk.status == Status::Waiting;
        match want {
            _ if wk.status == Status::Ended => Err(Refusal::new("ended", "this worker has ended")),
            Want::Working if waiting => Err(Refusal::new("waiting", "this worker waits for the watch")),
            _ => Ok(Live { session: Self::worker_session(worker, &started)?, started, waiting }),
        }
    }

    /// [`App::live`], `None` for a worker not live or of an unknown harness.
    pub(crate) async fn live_or_none(&self, worker: Uuid, want: Want) -> Answer<Option<Live>> {
        match self.live(worker, want).await {
            Err(r) if ["no_worker", "ended", "waiting", "no_harness"].contains(&r.code.as_str()) => Ok(None),
            live => live.map(Some),
        }
    }

    /// Runs the fast gate on the worktree as it stands. A clean worktree's
    /// green is recorded against its tree, so landing does not run it again
    /// on that tree.
    pub async fn worker_gate(&self, worker: Uuid) -> Answer {
        let st = self.live(worker, Want::Any).await?.started;
        let at = At::Worktree { worker, number: st.number };
        let g = self.gate(&st.checkout, &self.host.git(&st.worktree), at, Kind::Fast).await?.expect("a waited turn");
        let Some(ran) = g.ran else {
            return Ok(json!({ "green": g.green, "tree": g.tree, "recorded": g.recorded, "ran": false }));
        };
        Ok(
            json!({ "green": ran.ok, "tree": g.tree, "recorded": g.recorded, "took_ms": ran.took_ms, "tail": ran.tail, "timed_out": ran.timed_out }),
        )
    }

    /// Asks to land: the worktree must be committed, with commits of its own.
    pub async fn worker_done(&self, worker: Uuid) -> Answer {
        let st = self.live(worker, Want::Any).await?.started;
        let git = self.host.git(&st.worktree);
        if git.dirty().map_err(fail("git"))? {
            return Err(Refusal::new("dirty", "commit or remove your changes first: landing takes only commits"));
        }
        if git.ahead(&st.base, "HEAD").map_err(fail("git"))? == 0 {
            return Err(Refusal::new("nothing", "your branch has no commits to land"));
        }
        if git.branch().as_deref() != Some(st.branch.as_str()) {
            let why = format!("check out your branch {} again: landing takes it", st.branch);
            return Err(Refusal::new("off_branch", why));
        }
        self.submit::<w::AskLanding>(w::Of { worker }).await?;
        let said = "Landing is queued. Stop here: if it is red you will be told why in this terminal; if it is green your session ends.";
        Ok(json!({ "landing": true, "said": said }))
    }

    /// Hands the worker to the watch.
    pub async fn worker_escalate(&self, worker: Uuid, why: &str) -> Answer {
        if why.trim().is_empty() {
            return Err(Refusal::new("no_reason", "say what you need"));
        }
        let st = self.live(worker, Want::Any).await?.started;
        self.submit::<w::Wait>(w::Because { worker, why: format!("escalated: {why}") }).await?;
        // Kept on its issue; typed back at no one.
        self.note(&st.checkout, st.number, Author::Worker(worker), why).await?;
        Ok(json!({ "escalated": true, "said": "The watch will answer in this terminal. Stop until it does." }))
    }

    /// Keeps a record on the worker's issue, as the worker's comment. It
    /// leaves the worker as it is and calls no one: the watch is for
    /// decisions ([`App::worker_escalate`]).
    pub async fn worker_comment(&self, worker: Uuid, text: &str) -> Answer {
        if text.trim().is_empty() {
            return Err(Refusal::new("no_text", "say what to keep"));
        }
        let st = self.live(worker, Want::Any).await?.started;
        self.note(&st.checkout, st.number, Author::Worker(worker), text).await?;
        Ok(json!({ "commented": true, "checkout": st.checkout, "number": st.number }))
    }

    /// Files an issue the worker's issue asks for, as `issue add` does (on
    /// any checkout on the list, waiting from the start for `blocked_by`),
    /// never waiting for a person: a decision only the owner makes is
    /// escalated. Each side keeps the record, as the worker's comment: the
    /// filed issue names the issue and worker that filed it, the worker's
    /// issue the issue it filed. Answers the filed issue.
    pub async fn worker_file(
        &self,
        worker: Uuid,
        checkout: &str,
        title: &str,
        body: &str,
        blocked_by: &[Blocker],
    ) -> Answer {
        let st = self.live(worker, Want::Any).await?.started;
        let filed = self.issue_add(checkout, title, body, false, blocked_by).await?;
        let number = filed["number"].as_u64().expect("an issue shown has its number");
        let by = Author::Worker(worker);
        let from = format!("filed by worker {worker}, working {}#{}", st.checkout, st.number);
        self.note(checkout, number, by.clone(), &from).await?;
        self.note(&st.checkout, st.number, by, &format!("filed {checkout}#{number}")).await?;
        self.issue_show(checkout, number).await
    }

    /// Runs a command through the heavy-command queue at a gate's
    /// priority, so a time it measures (a test run, `slimfit check` or
    /// `ratchet`) is one a gate's run of the same tree agrees with.
    pub async fn worker_exclusive(&self, worker: Option<Uuid>, dir: &Path, cmd: &str) -> Answer {
        self.exclusive(Asker::Worker(worker), dir, cmd, false).await
    }

    /// [`App::worker_exclusive`] at background priority: for work whose
    /// time no gate compares (a mutation run), so a gate beside it gets the
    /// CPU first.
    pub async fn worker_exclusive_background(&self, worker: Option<Uuid>, dir: &Path, cmd: &str) -> Answer {
        self.exclusive(Asker::Worker(worker), dir, cmd, true).await
    }
}
