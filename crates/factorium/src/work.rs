//! A worker's own commands: `factorium worker gate|done|escalate|comment|waiting|file|exclusive`.
//! The worker is named by `FACTORIUM_WORKER`, which its session was
//! started with.

use std::path::Path;

use factorium_domain::declared::Kind;
use factorium_domain::issue::{Author, Blocker};
use factorium_domain::worker::{self as w, Started, Status, Workers};
use factum::Uuid;
use serde_json::json;

use crate::gate::At;
use crate::heavy::{Asker, Exclusive};
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
    /// The last limit its engine was seen at, and whether it waits on it.
    pub(crate) limit: Option<w::Limit>,
    pub(crate) on_limit: bool,
    /// When its status last changed, ms.
    pub(crate) since: i64,
}

impl App {
    /// Worker `worker`, live as `want` accepts it, with its session: one read.
    pub(crate) async fn live(&self, worker: Uuid, want: Want) -> Answer<Live> {
        let wk = self.state::<Workers>(worker).await?;
        let on_limit = wk.waits_on_limit().is_some();
        let started = wk.started.ok_or_else(|| Refusal::new("no_worker", format!("no worker {worker}")))?;
        let waiting = wk.status == Status::Waiting;
        match want {
            _ if wk.status == Status::Ended => Err(Refusal::new("ended", "this worker has ended")),
            Want::Working if waiting => Err(Refusal::new("waiting", "this worker waits for the watch")),
            _ => Ok(Live {
                session: Self::worker_session(worker, &started)?,
                started,
                waiting,
                on_limit,
                since: wk.since,
                limit: wk.limit,
            }),
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
    /// With `no_landing`, the evidence of an issue whose work changes no code,
    /// asks to finish without a landing instead, refused when it has commits:
    /// the daemon closes the issue with that evidence, running no gate.
    pub async fn worker_done(&self, worker: Uuid, no_landing: Option<&str>) -> Answer {
        let st = self.live(worker, Want::Any).await?.started;
        let git = self.host.git(&st.worktree);
        if git.dirty().map_err(fail("git"))? {
            return Err(Refusal::new("dirty", "commit or remove your changes first: landing takes only commits"));
        }
        let ahead = git.ahead(&st.base, "HEAD").map_err(fail("git"))?;
        if let Some(evidence) = no_landing {
            if ahead > 0 {
                let why = "your branch has commits of its own: they land with `factorium worker done`, never dropped";
                return Err(Refusal::new("has_commits", why));
            }
            self.submit::<w::AskFinish>(w::Because { worker, why: evidence.into() }).await?;
            let said = "Finishing is queued: your issue closes with your evidence, and your session ends. Stop here.";
            return Ok(json!({ "finishing": true, "evidence": evidence, "said": said }));
        }
        if ahead == 0 {
            let why = "your branch has no commits to land. If your issue's work changes no code (a publish, an \
                       upload, a measurement), finish it without a landing: `factorium worker done --no-landing \
                       \"<what you did and how you verified it>\"`";
            return Err(Refusal::new("nothing", why));
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

    /// Declares a wait on a long command of the worker's own (a download it
    /// runs in the background): no silence is found in it until `for_ms`
    /// from now, held to two hours. Kept on its issue, as its comment.
    pub async fn worker_waiting(&self, worker: Uuid, why: &str, for_ms: i64) -> Answer {
        let st = self.live(worker, Want::Working).await?.started;
        self.submit::<w::DeclareWait>(w::Declare { worker, why: why.into(), for_ms }).await?;
        let d = self.state::<Workers>(worker).await?.declared.expect("a wait just declared");
        let mins = (d.until - d.at) / 60_000;
        let text = format!("waits {mins} min for: {why}");
        self.note(&st.checkout, st.number, Author::Worker(worker), &text).await?;
        let said = "Not found silent before then. Past it, the watch is told what you said.";
        Ok(json!({ "waiting": d.why, "until": d.until, "minutes": mins, "said": said }))
    }

    /// Files an issue the worker's issue asks for, as `issue add` does (on
    /// any checkout on the list, waiting from the start for `blocked_by`),
    /// never waiting for a person: a decision only the owner makes is
    /// escalated. Each side keeps the record, as the worker's comment: the
    /// filed issue names the issue and worker that filed it, the worker's
    /// issue the issue it filed. Filed on the worker's own checkout, it also
    /// waits for the worker's issue, whose unlanded change a follow-up
    /// usually builds on (factorium#184), unless `independent`. Answers the
    /// filed issue, its blocks among it.
    pub async fn worker_file(
        &self,
        worker: Uuid,
        checkout: &str,
        title: &str,
        body: &str,
        blocked_by: &[Blocker],
        independent: bool,
    ) -> Answer {
        let st = self.live(worker, Want::Any).await?.started;
        let mut blocked_by = blocked_by.to_vec();
        if checkout == st.checkout && !independent {
            blocked_by.push(Blocker::Here(st.number));
        }
        let filed = self.issue_add(checkout, title, body, false, &blocked_by).await?;
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
        self.worker_exclusive_as(worker, dir, cmd, Exclusive::default()).await
    }

    /// [`App::worker_exclusive`] as `how` says: at background priority, or
    /// holding a resource the repo declares, for as long as it declares.
    pub async fn worker_exclusive_as(&self, worker: Option<Uuid>, dir: &Path, cmd: &str, how: Exclusive) -> Answer {
        self.exclusive(Asker::Worker(worker), dir, cmd, how).await
    }

    /// [`App::worker_exclusive`] at background priority: for work whose
    /// time no gate compares (a mutation run), so a gate beside it gets the
    /// CPU first.
    pub async fn worker_exclusive_background(&self, worker: Option<Uuid>, dir: &Path, cmd: &str) -> Answer {
        self.worker_exclusive_as(worker, dir, cmd, Exclusive { background: true, ..Exclusive::default() }).await
    }
}
