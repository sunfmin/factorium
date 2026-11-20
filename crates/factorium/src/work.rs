//! A worker's own commands: `factorium worker gate|done|escalate|comment|exclusive`.
//! The worker is named by `FACTORIUM_WORKER`, which its session was
//! started with.

use std::path::Path;
use std::time::Duration;

use factorium_domain::declared::Kind;
use factorium_domain::issue::Author;
use factorium_domain::settings::Time;
use factorium_domain::worker::{self as w, Started, Status, Workers};
use factorium_host::Priority;
use factum::Uuid;
use serde_json::json;

use crate::gate::At;
use crate::heavy::{Job, Turn};
use crate::{Answer, App, Refusal, fail};

impl App {
    pub(crate) async fn started(&self, worker: Uuid) -> Answer<(w::Worker, Started)> {
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
        let (_, st) = self.started(worker).await?;
        let git = self.host.git(&st.worktree);
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
        let (_, st) = self.started(worker).await?;
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
        let (_, st) = self.started(worker).await?;
        self.note(&st.checkout, st.number, Author::Worker(worker), text).await?;
        Ok(json!({ "commented": true, "checkout": st.checkout, "number": st.number }))
    }

    /// Runs a command through the heavy-command queue at a gate's
    /// priority, so a time it measures (a test run, `slimfit check` or
    /// `ratchet`) is one a gate's run of the same tree agrees with.
    pub async fn worker_exclusive(&self, worker: Option<Uuid>, dir: &Path, cmd: &str) -> Answer {
        self.exclusive(worker, dir, cmd, Priority::Normal).await
    }

    /// [`App::worker_exclusive`] at background priority: for work whose
    /// time no gate compares (a mutation run), so a gate beside it gets the
    /// CPU first.
    pub async fn worker_exclusive_background(&self, worker: Option<Uuid>, dir: &Path, cmd: &str) -> Answer {
        self.exclusive(worker, dir, cmd, Priority::Background).await
    }

    async fn exclusive(&self, worker: Option<Uuid>, dir: &Path, cmd: &str, priority: Priority) -> Answer {
        let limit = Duration::from_millis(self.settings().await?.ms(Time::HoldBound).max(60_000) as u64 * 4);
        let steps = [cmd.to_string()];
        let job = Job { what: cmd.into(), worker, bound_at: None, dir, steps: &steps, env: &[], limit, priority };
        let ran = self.heavy(job, Turn::Wait).await?.and_then(|mut r| r.pop()).expect("a waited turn");
        Ok(json!({ "ok": ran.ok, "code": ran.code, "took_ms": ran.took_ms, "tail": ran.tail }))
    }
}
