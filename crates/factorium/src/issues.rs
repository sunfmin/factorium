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
        self.issue_add_on(checkout, title, body, waiting_for_human, blocked_by, On::default()).await
    }

    /// [`App::issue_add`], its worker to start as `on` says: with the rules
    /// and skills of the role directory it names, on the account it names.
    /// Refused unless the role is a directory named by its absolute path and
    /// the account is one this host can use.
    pub async fn issue_add_on(
        &self,
        checkout: &str,
        title: &str,
        body: &str,
        waiting_for_human: bool,
        blocked_by: &[Blocker],
        on: On,
    ) -> Answer {
        self.checkout(checkout).await?;
        self.on_here(&on)?;
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
            on,
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
            "claim": i.claim,
            "task": i.task,
            "role": i.on.role,
            "account": i.on.account,
            "spent": self.issue_spent(&i).await?,
            "comments": i.comments.iter().map(|c| json!({ "text": c.text, "by": c.by.to_string(), "at": c.at })).collect::<Vec<_>>(),
        }))
    }

    pub async fn issue_edit(&self, checkout: &str, number: u64, title: Option<&str>, body: Option<&str>) -> Answer {
        let edit = Edit { checkout: checkout.into(), number, title: title.map(Into::into), body: body.map(Into::into) };
        self.submit::<EditIssue>(edit).await?;
        self.issue_show(checkout, number).await
    }

    /// Closes an issue; a worker on it ends. The attempt whose worktree its
    /// last attempt left on disk is abandoned ([`App::abandon`]). Claimed
    /// ([`App::claimed`]): refused while a start is under way.
    pub async fn issue_close(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        self.claimed(checkout, number, Claimant::Close, async {
            let issue = self.issue(checkout, number).await?;
            // A checkout off the list has no worktrees factorium removes.
            let left = match self.checkout(checkout).await {
                Ok(c) => self.left(&issue).await?.map(|st| (c, st)),
                Err(_) => None,
            };
            let Some((c, st)) = left else { return self.close(checkout, number, reason, false).await };
            let done = self.abandon(&c, &st, Why::Close(reason)).await?;
            Ok(done.shown(self.issue_show(checkout, number).await?))
        })
        .await
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
    /// begins from the proving tip; nothing is lost. Claimed
    /// ([`App::claimed`]), so refused while a worker is on the issue; its
    /// state (ready or waiting for a person) stays.
    pub async fn issue_start_over(&self, checkout: &str, number: u64, why: &str) -> Answer {
        if why.trim().is_empty() {
            return Err(Refusal::new("no_reason", "say why the issue starts over"));
        }
        self.issue(checkout, number).await?;
        self.claimed(checkout, number, Claimant::StartOver, async {
            let (c, issue) = (self.checkout(checkout).await?, self.issue(checkout, number).await?);
            let Some(st) = self.kept_now(&c, &issue).await? else {
                let mut shown = self.issue_show(checkout, number).await?;
                shown["discarded"] = json!(false);
                shown["said"] = json!("nothing was kept to discard: the next start begins from the proving tip");
                return Ok(shown);
            };
            let done = self.abandon(&c, &st, Why::StartOver(why)).await?;
            Ok(done.shown(self.issue_show(checkout, number).await?))
        })
        .await
    }

    /// Runs `act` holding issue `number`'s claim for `by`, freed once it
    /// is done (factorium#189): what `act` reads of the issue's worktrees,
    /// no start changes before it acts. Refused while another holds it; a
    /// claim whose process is gone is cleared first. A closed issue takes
    /// no start, so `act` runs on one unclaimed.
    pub(crate) async fn claimed<T>(
        &self,
        checkout: &str,
        number: u64,
        by: Claimant,
        act: impl Future<Output = Answer<T>>,
    ) -> Answer<T> {
        self.clear_dead_claim(&self.issue(checkout, number).await?).await?;
        let claim = Claiming { checkout: checkout.into(), number, claim: Claim { by, pid: std::process::id() } };
        match self.submit::<ClaimIssue>(claim.clone()).await {
            Err(r) if r.code == "ended" => return act.await,
            claimed => claimed?,
        }
        let done = act.await;
        self.unclaim(claim).await?;
        done
    }

    /// Frees `claim`, if it holds its issue still: closed, it holds none.
    pub(crate) async fn unclaim(&self, claim: Claiming) -> Answer<()> {
        match self.submit::<UnclaimIssue>(claim).await {
            Err(r) if r.code == "ended" => Ok(()),
            freed => freed,
        }
    }

    /// Clears `issue`'s claim if the process that holds it is gone.
    pub(crate) async fn clear_dead_claim(&self, issue: &Issue) -> Answer<()> {
        match &issue.claim {
            Some(c) if !self.host.alive(c.pid) => {
                let (checkout, number) = (issue.checkout.clone(), issue.number);
                self.unclaim(Claiming { checkout, number, claim: c.clone() }).await
            }
            _ => Ok(()),
        }
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

impl App {
    /// Refuses `on` unless its role is a directory named by its absolute
    /// path and its account one mytokens holds that this host can use.
    fn on_here(&self, on: &On) -> Answer<()> {
        if let Some(role) = &on.role {
            let path = std::path::Path::new(role);
            if !path.is_absolute() || !path.is_dir() {
                let why = format!("the role `{role}` is not a directory named by its absolute path");
                return Err(Refusal::new("no_role", why));
            }
        }
        if let Some(account) = &on.account {
            let here = self.accounts_here();
            if !here.usable.iter().any(|u| &u.name == account) {
                let skipped = here
                    .skipped
                    .iter()
                    .find(|s| s.secret == format!("{}{account}", factorium_harness::account::PREFIX));
                let why = skipped.map_or("mytokens holds no such account".to_string(), |s| s.why.clone());
                return Err(Refusal::new("no_account", format!("account {account} cannot be used here: {why}")));
            }
        }
        Ok(())
    }

    /// What issue `i` took: each attempt's time, from its start to its end
    /// (a live one's, to now), in ms, and the tokens its session spent, as
    /// recorded when it ended (a live one's, as its session files say now;
    /// `null`: its harness's files do not say); and their sums, `tokens`
    /// summing the attempts that say.
    async fn issue_spent(&self, i: &Issue) -> Answer<serde_json::Value> {
        use factorium_domain::worker::{Status as W, Tokens, Workers};
        let (mut ms, mut tokens, mut attempts) = (0, None::<Tokens>, Vec::new());
        for attempt in 1..=i.attempts {
            let worker = factorium_domain::worker_id(&i.checkout, i.number, attempt);
            let wk = self.state::<Workers>(worker).await?;
            let Some(st) = &wk.started else { continue };
            let ended = wk.status == W::Ended;
            let took = ((if ended { wk.since } else { self.now() }) - st.at).max(0);
            let spent = if ended { wk.spent } else { self.spent(st) };
            ms += took;
            if let Some(s) = spent {
                let sum = tokens.get_or_insert_default();
                (sum.input, sum.output) = (sum.input + s.input, sum.output + s.output);
            }
            attempts.push(json!({ "attempt": attempt, "worker": worker, "harness": st.harness,
                "account": st.account, "ended": ended, "ms": took, "tokens": spent }));
        }
        Ok(json!({ "ms": ms, "tokens": tokens, "attempts": attempts }))
    }
}
