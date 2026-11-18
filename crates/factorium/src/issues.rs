//! The tracker: every `issue` command, and the issue reads and notes the
//! daemon, the line, the watch and a worker's life share.

use factorium_domain::issue::*;
use factorium_domain::issue_id;
use factorium_domain::tracker::{self, shown};
use factum::Command;
use serde_json::json;

use crate::life::End;
use crate::{Answer, App, Refusal};

impl App {
    /// Makes an issue numbered one past the checkout's last, waiting from
    /// the start for the issues in `blocked_by`, of its checkout or of
    /// others, so no tick sees it ready without them.
    pub async fn issue_add(
        &self,
        checkout: &str,
        title: &str,
        body: &str,
        waiting_for_human: bool,
        blocked_by: &[Blocker],
    ) -> Answer {
        self.checkout(checkout).await?;
        let (mut here, mut across) = (Vec::new(), Vec::new());
        for by in blocked_by {
            match self.blocker(checkout, by.clone()).await? {
                Blocker::Here(n) => here.push(n),
                Blocker::Across(a) => across.push(a),
            }
        }
        loop {
            let last = self.rows(tracker::last_issue(checkout)).await?;
            let number = last.first().map_or(0, |r| r.number) + 1;
            let open = Open {
                checkout: checkout.into(),
                number,
                title: title.into(),
                body: body.into(),
                waiting_for_human,
                blocked_by: here.clone(),
                across: across.clone(),
            };
            match self.submit::<OpenIssue>(&open).await {
                // Another command took the number first: take the next.
                Err(r) if r.code == "number_taken" => continue,
                Err(r) => return Err(r),
                Ok(()) => return self.issue_show(checkout, number).await,
            }
        }
    }

    /// The issues of a checkout, or of every checkout; open ones unless
    /// `closed` says to list the closed ones too.
    pub async fn issue_list(&self, checkout: Option<&str>, closed: bool) -> Answer {
        Ok(shown(&self.rows(tracker::issues(checkout, closed)).await?))
    }

    pub(crate) async fn issue(&self, checkout: &str, number: u64) -> Answer<Issue> {
        let i = self.state::<Issues>(issue_id(checkout, number)).await?;
        if i.opened() { Ok(i) } else { Err(Refusal::new("no_issue", format!("{checkout}#{number} does not exist"))) }
    }

    pub async fn issue_show(&self, checkout: &str, number: u64) -> Answer {
        let i = self.issue(checkout, number).await?;
        Ok(json!({
            "checkout": i.checkout,
            "number": i.number,
            "title": i.title,
            "body": i.body,
            "status": i.status,
            "blocked_by": i.blocked_by,
            "blocked_across": i.across.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "reason": i.reason,
            "worker": i.worker,
            "comments": i.comments.iter().map(|c| json!({ "text": c.text, "by": c.by.to_string(), "at": c.at })).collect::<Vec<_>>(),
        }))
    }

    pub async fn issue_edit(&self, checkout: &str, number: u64, title: Option<&str>, body: Option<&str>) -> Answer {
        let edit = Edit { checkout: checkout.into(), number, title: title.map(Into::into), body: body.map(Into::into) };
        self.submit::<EditIssue>(edit).await?;
        self.issue_show(checkout, number).await
    }

    /// Closes an issue; a worker on it ends.
    pub async fn issue_close(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        let why = format!("its issue closed: {reason}");
        self.leave::<CloseIssue>(checkout, number, reason, End::because(why, false)).await
    }

    /// Leaves an issue to a person: only `issue ready` moves it again. A
    /// worker on it ends, keeping its worktree.
    pub async fn issue_wait_for_human(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        let why = format!("its issue waits for a person: {reason}");
        self.leave::<WaitForHuman>(checkout, number, reason, End::because(why, true)).await
    }

    /// The issue leaves its worker: command `C` moves it, for `reason`, and
    /// the worker on it, if one is, ends as `end` says.
    async fn leave<C: Command<Params = Because>>(&self, checkout: &str, number: u64, reason: &str, end: End) -> Answer {
        let worker = self.issue(checkout, number).await?.worker;
        self.submit::<C>(Because { checkout: checkout.into(), number, reason: reason.into() }).await?;
        if let Some(w) = worker {
            self.end_worker(w, end).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// Starts an issue over on purpose: the worktree its last attempt kept
    /// is removed, so its next start begins from the proving tip. Nothing is
    /// lost: what was not committed there is committed, the branch is kept
    /// as `<branch>-abandoned`, and a comment names it with `why`. Refused
    /// while a worker is on the issue; its state (ready or waiting for a person) stays.
    pub async fn issue_start_over(&self, checkout: &str, number: u64, why: &str) -> Answer {
        if why.trim().is_empty() {
            return Err(Refusal::new("no_reason", "say why the issue starts over"));
        }
        let issue = self.issue(checkout, number).await?;
        if issue.worker.is_some() {
            return Err(Refusal::new(
                "live_worker",
                format!(
                    "a worker is on {checkout}#{number}: leave it to a person first (factorium issue wait-for-human {checkout} {number} --why …)"
                ),
            ));
        }
        let c = self.checkout(checkout).await?;
        let Some((worktree, abandoned)) = self.discard_kept(&c, &issue).await? else {
            let mut shown = self.issue_show(checkout, number).await?;
            shown["discarded"] = json!(false);
            shown["said"] = json!("nothing was kept to discard: the next start begins from the proving tip");
            return Ok(shown);
        };
        let text = format!(
            "Started over: attempt {}'s worktree was discarded; its branch is kept as `{abandoned}` (not landed). Why: {why}",
            issue.attempts
        );
        self.note(checkout, number, Author::Factorium, &text).await?;
        let mut shown = self.issue_show(checkout, number).await?;
        shown["discarded"] = json!(worktree);
        shown["abandoned"] = json!(abandoned);
        Ok(shown)
    }

    pub async fn issue_ready(&self, checkout: &str, number: u64) -> Answer {
        self.submit::<ReadyIssue>(Ref { checkout: checkout.into(), number }).await?;
        self.issue_show(checkout, number).await
    }

    /// `issue comment`: records the comment, then passes it to the worker
    /// on the issue, if one is ([`App::pass_comment`]). The answer's
    /// `delivery` says how; `undelivered`, why it did not reach the worker.
    /// Comments factorium writes itself are recorded only.
    pub async fn issue_comment(&self, checkout: &str, number: u64, text: &str) -> Answer {
        self.note(checkout, number, Author::Owner, text).await?;
        let (delivery, why) = match self.issue(checkout, number).await?.worker {
            Some(worker) => self.pass_comment(worker, text).await?,
            None => ("none", None),
        };
        let mut shown = self.issue_show(checkout, number).await?;
        shown["delivery"] = json!(delivery);
        if let Some(why) = why {
            shown["undelivered"] = json!(why);
        }
        Ok(shown)
    }

    /// Records a comment on an issue, the one record of what is said about
    /// it: by the owner, its workers, the watch, and factorium. Recording
    /// passes it to no one.
    pub(crate) async fn note(&self, checkout: &str, number: u64, by: Author, text: &str) -> Answer<()> {
        self.submit::<CommentIssue>(Comment { checkout: checkout.into(), number, text: text.into(), by }).await?;
        Ok(())
    }

    /// Makes issue `number` wait for `by`, an issue of its checkout or of
    /// another, or, with `undo`, stop waiting for it.
    pub async fn issue_block(&self, checkout: &str, number: u64, by: impl Into<Blocker>, undo: bool) -> Answer {
        let by = by.into().of(checkout);
        let by = if undo { by } else { self.blocker(checkout, by).await? };
        let c = checkout.to_string();
        match (by, undo) {
            (Blocker::Here(by), false) => self.submit::<BlockIssue>(Block { checkout: c, number, by }).await?,
            (Blocker::Here(by), true) => self.submit::<UnblockIssue>(Block { checkout: c, number, by }).await?,
            (Blocker::Across(by), false) => {
                self.submit::<BlockIssueAcross>(BlockAcross { checkout: c, number, by }).await?
            }
            (Blocker::Across(by), true) => {
                self.submit::<UnblockIssueAcross>(BlockAcross { checkout: c, number, by }).await?
            }
        }
        self.issue_show(checkout, number).await
    }

    /// `by` as an issue of `checkout` waits for it, refused unless that
    /// issue exists and, for its cut, its repo declares one.
    async fn blocker(&self, checkout: &str, by: Blocker) -> Answer<Blocker> {
        let by = by.of(checkout);
        match &by {
            Blocker::Here(n) => drop(self.issue(checkout, *n).await?),
            Blocker::Across(a) => {
                let c = self.checkout(&a.checkout).await?;
                self.issue(&a.checkout, a.number).await?;
                if a.cut && self.declared(&self.host.git(&c.path), &c.base)?.cut.is_none() {
                    let why =
                        format!("{} declares no cut for {a} to wait for: name {}#{}", a.checkout, a.checkout, a.number);
                    return Err(Refusal::new("no_cut", why));
                }
            }
        }
        Ok(by)
    }
}
