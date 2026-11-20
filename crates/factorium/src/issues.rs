//! The tracker: every `issue` command, and the issue reads and notes the
//! daemon, the line, the watch and a worker's life share.

use factorium_domain::issue::*;
use factorium_domain::issue_id;
use factorium_domain::line::Lines;
use factorium_domain::line_id;
use factorium_domain::tracker::{self, shown};
use factum::Command;
use serde_json::json;

use crate::kept::Why;
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
        for by in blocked_by {
            self.blocker(checkout, by.clone()).await?;
        }
        let (title, body) = (title.into(), body.into());
        let open = Open {
            checkout: checkout.into(),
            title,
            body,
            waiting_for_human,
            blocked_by: blocked_by.to_vec(),
            ..Open::default()
        };
        let number = self.open_issue(open).await?;
        self.issue_show(checkout, number).await
    }

    /// Opens `open` as the checkout's next issue; answers its number.
    pub(crate) async fn open_issue(&self, mut open: Open) -> Answer<u64> {
        loop {
            let last = self.rows(tracker::last_issue(&open.checkout)).await?;
            open.number = last.first().map_or(0, |r| r.number) + 1;
            match self.submit::<OpenIssue>(&open).await {
                // Another command took the number first: take the next.
                Err(r) if r.code == "number_taken" => continue,
                Err(r) => return Err(r),
                Ok(()) => return Ok(open.number),
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
            "blocked_by": i.blocked_by(),
            "blocked_across": i.blocked_across(),
            "reason": i.reason,
            "worker": i.worker,
            "task": i.task,
            "comments": i.comments.iter().map(|c| json!({ "text": c.text, "by": c.by.to_string(), "at": c.at })).collect::<Vec<_>>(),
        }))
    }

    pub async fn issue_edit(&self, checkout: &str, number: u64, title: Option<&str>, body: Option<&str>) -> Answer {
        let edit = Edit { checkout: checkout.into(), number, title: title.map(Into::into), body: body.map(Into::into) };
        self.submit::<EditIssue>(edit).await?;
        self.issue_show(checkout, number).await
    }

    /// Closes an issue; a worker on it ends. The attempt whose worktree its
    /// last attempt left on disk is abandoned ([`App::abandon`]).
    pub async fn issue_close(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        let issue = self.issue(checkout, number).await?;
        // A checkout off the list has no worktrees factorium removes.
        let left = match self.checkout(checkout).await {
            Ok(c) => self.left(&issue).await?.map(|st| (c, st)),
            Err(_) => None,
        };
        let Some((c, st)) = left else { return self.close(checkout, number, reason, false).await };
        let done = self.abandon(&c, &st, Why::Close(reason)).await?;
        Ok(done.shown(self.issue_show(checkout, number).await?))
    }

    /// Closes an issue for `reason`; a worker on it ends, keeping its worktree if `keep`.
    pub(crate) async fn close(&self, checkout: &str, number: u64, reason: &str, keep: bool) -> Answer {
        let why = format!("its issue closed: {reason}");
        self.leave::<CloseIssue>(checkout, number, reason, End::because(why, keep)).await
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

    /// Starts an issue over on purpose: the attempt whose worktree its last
    /// attempt kept is abandoned ([`App::abandon`]), so its next start
    /// begins from the proving tip; nothing is lost. Refused while a worker
    /// is on the issue; its state (ready or waiting for a person) stays.
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
        let Some(st) = self.kept_now(&c, &issue).await? else {
            let mut shown = self.issue_show(checkout, number).await?;
            shown["discarded"] = json!(false);
            shown["said"] = json!("nothing was kept to discard: the next start begins from the proving tip");
            return Ok(shown);
        };
        let done = self.abandon(&c, &st, Why::StartOver(why)).await?;
        Ok(done.shown(self.issue_show(checkout, number).await?))
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
        let by = by.into();
        if !undo {
            self.blocker(checkout, by.clone()).await?;
        }
        self.submit::<BlockIssue>(Block { checkout: checkout.into(), number, by, undo }).await?;
        // Its waiting worker is parked behind it (SPEC §2): back to work once all it waits for is met.
        if let (false, Some(worker)) = (undo, self.issue(checkout, number).await?.worker) {
            self.submit::<factorium_domain::worker::Park>(factorium_domain::worker::Of { worker }).await?;
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

    /// What `issue` waits for, each named, and how it was met (its issue's
    /// close, such as the landing), or `None` while it is not: met once its
    /// issue is closed (or gone), and, for its cut, its landing is in a cut.
    /// Its landing is recorded before it closes, so a closed issue the line
    /// does not hold either landed in a cut or never landed.
    pub(crate) async fn blockers(&self, issue: &Issue) -> Answer<Vec<(String, Option<String>)>> {
        let mut all = Vec::new();
        for (name, a) in issue.waits_for() {
            let met = match self.issue(&a.checkout, a.number).await {
                Ok(i) if i.status != Status::Closed => None,
                _ if a.cut && self.state::<Lines>(line_id(&a.checkout)).await?.awaits_cut(a.number) => None,
                Ok(i) => Some(i.reason.unwrap_or_else(|| "closed".into()) + if a.cut { ", in a cut" } else { "" }),
                Err(_) => Some("gone".into()),
            };
            all.push((name, met));
        }
        Ok(all)
    }
}
