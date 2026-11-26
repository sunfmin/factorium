//! A worker's life, from start to end: the one module that starts a worker
//! and ends it. A start takes the issue, opens the worker's session in its
//! worktree ([`crate::kept`]), records it started and hands it its brief; a
//! start that fails after its session opened is undone: what it opened is
//! closed and its issue is free again; one the daemon died in the middle of
//! is undone when it starts again ([`App::free_unstarted`]). Every end,
//! whatever its cause, runs in one order ([`App::finish`]).

use factorium_domain::checkout::Checkout;
use factorium_domain::issue::{
    Because, Claim, Claimant, Claiming, CloseIssue, Issue, ReleaseIssue, Take, TakeIssue, Taking,
};
use factorium_domain::outbox::Outcome;
use factorium_domain::tracker;
use factorium_domain::worker::{self as w, Started, StartedRole, Status, Workers};
use factorium_domain::worker_id;
use factum::Uuid;
use serde_json::json;

use crate::kept::{Resumed, WorkerBranch};
use crate::session::{Pick, Role};
use crate::{Answer, App, Refusal, Want, fail};

/// Why a worker ends.
pub(crate) enum End {
    /// It landed as this commit: its issue closes, and its worktree goes.
    Landed(String),
    /// It ends for `why`: its issue is free again, if it holds it still;
    /// its worktree is kept, or goes unless it holds work of its own.
    Because { why: String, keep: bool },
}

impl End {
    pub(crate) fn because(why: impl Into<String>, keep: bool) -> End {
        End::Because { why: why.into(), keep }
    }
}

impl App {
    /// Starts a worker on issue `number` of `checkout`. Its take claims the
    /// issue ([`TakeIssue`]) until the start is recorded or undone, so no
    /// command that abandons a worktree acts on it meanwhile (factorium#189).
    pub(crate) async fn start_worker(&self, checkout: &str, number: u64) -> Answer {
        let c = self.checkout(checkout).await?;
        let issue = self.issue(checkout, number).await?;
        let pick = self.pick(checkout, &issue.on).await?;
        let attempt = issue.attempts + 1;
        let worker = worker_id(checkout, number, attempt);
        // Unread, the tip leaves no worktree to start in (SPEC §2.6): nothing is taken.
        let tip = self.host.git(&c.path).fetch(&c.proving).map_err(fail("git"))?;
        let pid = std::process::id();
        self.submit::<TakeIssue>(Taking { checkout: checkout.into(), number, worker, pid }).await?;
        let started = async {
            let (st, typed) = match self.open_worker(&c, &issue, attempt, worker, &pick, tip).await {
                Ok(opened) => opened,
                Err(r) => {
                    self.submit::<ReleaseIssue>(Take { checkout: checkout.into(), number, worker }).await?;
                    return Err(r);
                }
            };
            let shown = json!({ "worker": worker, "issue": format!("{checkout}#{number}"), "harness": st.harness, "account": st.account, "worktree": st.worktree });
            match self.begin(worker, &st, typed).await {
                Ok(()) => Ok(shown),
                Err((why, r)) => Err(self.undo_start(worker, &st, why, r).await),
            }
        }
        .await;
        let claim = Claim { by: Claimant::Start, pid };
        self.unclaim(Claiming { checkout: checkout.into(), number, claim }).await?;
        started
    }

    /// Frees each issue taken by a worker never recorded started: the
    /// daemon, the one process that starts workers, died in the middle of
    /// its start (factorium#190), so it runs as the daemon starts, with no
    /// start under way. As [`App::undo_start`] does, its issue is free again
    /// and the worktree Orca made for it goes, unless it holds work of its
    /// own (held, doctor names it). Answers what it freed.
    pub(crate) async fn free_unstarted(&self) -> Answer<Vec<serde_json::Value>> {
        let mut freed = Vec::new();
        for row in self.rows(tracker::held()).await? {
            let Some(worker) = row.holder.worker else { continue };
            if self.state::<Workers>(worker).await?.started.is_some() {
                continue;
            }
            let (checkout, number) = (row.checkout, row.number);
            let attempt = self.issue(&checkout, number).await?.attempts;
            self.submit::<ReleaseIssue>(Take { checkout: checkout.clone(), number, worker }).await?;
            // Its start's claim (factorium#189) was the dead daemon's: it goes too.
            if let Some(claim) = self.issue(&checkout, number).await?.claim.filter(|c| c.by == Claimant::Start) {
                self.unclaim(Claiming { checkout: checkout.clone(), number, claim }).await?;
            }
            let branch = WorkerBranch { checkout: checkout.clone(), number, attempt }.name();
            let c = self.checkout(&checkout).await?;
            let made = self.host.git(&c.path).worktrees().unwrap_or_default();
            let made = made.into_iter().find(|(_, b)| b.as_deref() == Some(branch.as_str()));
            if let Some((path, _)) = &made {
                let worktree = path.to_string_lossy().into_owned();
                let st =
                    Started { checkout: checkout.clone(), number, attempt, worktree, branch, ..Started::default() };
                self.release_worker(&st, true).await?;
            }
            freed.push(
                json!({ "issue": format!("{checkout}#{number}"), "worker": worker, "worktree": made.map(|(p, _)| p) }),
            );
        }
        Ok(freed)
    }

    /// Records a worker whose session opened as started, and hands it its
    /// brief. `Err`: why it cannot go on, and the refusal its start answers.
    async fn begin(&self, worker: Uuid, st: &Started, typed: Option<String>) -> Result<(), (String, Refusal)> {
        let failed = |r: Refusal| (format!("its start failed: {r}"), r);
        self.submit::<w::StartWorker>(w::Start { worker, started: st.clone() }).await.map_err(failed)?;
        let Some(typed) = typed else { return Ok(()) };
        let session = self.live(worker, Want::Working).await.map_err(failed)?.session;
        match self.hand_brief(&session, &typed).await.map_err(failed)? {
            // Typed once the harness is up, on a later look.
            Outcome::NotTyped { code, .. } if code == "not_up" => Ok(()),
            // Up, yet Orca would not type at it: the start fails.
            Outcome::NotTyped { code, why } => {
                Err((format!("its brief could not be typed: {why}"), Refusal::new(&code, why)))
            }
            _ => Ok(()),
        }
    }

    /// Undoes a start that failed after its session opened, for `why`: what
    /// factorium opened, factorium closes, and the issue is free again.
    /// Answers the start's refusal `r`, with what could not be undone.
    async fn undo_start(&self, worker: Uuid, st: &Started, why: String, r: Refusal) -> Refusal {
        let undone = match self.state::<Workers>(worker).await {
            // Not recorded started, it is ended as one never recorded.
            Ok(wk) => self.finish(worker, st, wk.started.map(|_| wk.status), End::because(why, false)).await,
            Err(e) => Err(e),
        };
        match undone {
            Ok(None) => r,
            Ok(Some(unclosed)) => {
                Refusal::new(&r.code, format!("{}; its terminal {} is not closed: {unclosed}", r.message, st.terminal))
            }
            Err(e) => Refusal::new(&r.code, format!("{}; undoing its start failed: {e}", r.message)),
        }
    }

    /// Ends a worker, as `end` says.
    pub(crate) async fn end_worker(&self, worker: Uuid, end: End) -> Answer<()> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started else { return Ok(()) };
        self.finish(worker, &st, Some(wk.status), end).await.map(drop)
    }

    /// The one order a worker ends in. Its terminal is closed first, before
    /// it is recorded ended (factorium#43): an ended worker's terminal that
    /// Orca lists still is an orphan, which the watch would close in its
    /// stead. Then it is recorded landed or ended (`status`: as it stands;
    /// `None`: never recorded started); then its issue closes, or is free
    /// again; then its worktree goes, unless kept or holding work of its
    /// own. Answers why its terminal is not closed, if it is not.
    async fn finish(&self, worker: Uuid, st: &Started, status: Option<Status>, end: End) -> Answer<Option<String>> {
        let unclosed = self.close_worker_session(worker, st).await?;
        // What it spent, read once it stopped spending and kept: its session
        // files are the harness's, which may clear them.
        if let (Some(Status::Working | Status::Landing | Status::Waiting), Some(tokens)) = (status, self.spent(st)) {
            self.submit::<w::NoteSpent>(w::Spend { worker, tokens }).await?;
        }
        match (&end, status) {
            (_, None | Some(Status::Ended)) => {}
            (End::Landed(commit), _) => {
                self.submit::<w::GreenLanding>(w::Land { worker, commit: commit.clone() }).await?
            }
            (End::Because { why, .. }, _) => {
                self.submit::<w::EndWorker>(w::Because { worker, why: why.clone() }).await?
            }
        }
        self.leave_issue(worker, st, &end).await?;
        Ok(unclosed)
    }

    /// The rest of a worker's end, once it is recorded landed or ended:
    /// its issue closes, or is free again; then its worktree goes, unless
    /// kept or holding work of its own.
    async fn leave_issue(&self, worker: Uuid, st: &Started, end: &End) -> Answer<()> {
        let (checkout, number) = (st.checkout.clone(), st.number);
        let keep = match end {
            End::Landed(commit) => {
                let reason = factorium_domain::issue::landed_as(commit);
                self.submit::<CloseIssue>(Because { checkout, number, reason }).await?;
                false
            }
            End::Because { keep, .. } => {
                match self.submit::<ReleaseIssue>(Take { checkout, number, worker }).await {
                    Ok(()) => {}
                    Err(r) if r.code == "no_issue" || r.code == "ended" => {}
                    Err(r) => return Err(r),
                }
                *keep
            }
        };
        self.release_worker(st, !keep).await
    }

    /// Each open issue a worker that landed holds still, closed: a daemon
    /// that died between recording the landing and closing its issue left
    /// it so, and no tick lands the worker again (factorium#192). The rest
    /// of its end, in [`App::finish`]'s order. Answers the issues closed.
    pub(crate) async fn close_landed(&self) -> Answer<Vec<serde_json::Value>> {
        let mut closed = Vec::new();
        for issue in self.rows(tracker::held()).await? {
            let Some(worker) = issue.holder.worker else { continue };
            let wk = self.state::<Workers>(worker).await?;
            let (Status::Ended, Some(st), Some(commit)) = (wk.status, wk.started, wk.landed) else { continue };
            self.leave_issue(worker, &st, &End::Landed(commit.clone())).await?;
            closed.push(json!({ "closed": format!("{}#{}", issue.checkout, issue.number), "commit": commit }));
        }
        Ok(closed)
    }

    /// Each worker that asked to finish without a landing
    /// (`worker done --no-landing`): its issue closes with its evidence
    /// as the reason, no gate run, and it ends in [`App::finish`]'s order,
    /// its worktree going unless it holds work of its own. The daemon does
    /// it, not the worker's own command: that ends the worker's terminal,
    /// which the command runs in. Answers the issues closed.
    pub(crate) async fn finish_unlanded(&self) -> Answer<Vec<serde_json::Value>> {
        let mut finished = Vec::new();
        for row in self.rows(tracker::workers(Some(Status::Finishing), false)).await? {
            let evidence = row.state.why.unwrap_or_default();
            let reason = format!("finished without a landing, its work changing no code: {evidence}");
            self.close(&row.checkout, row.number, &reason, false).await?;
            finished.push(json!({ "finished": format!("{}#{}", row.checkout, row.number), "evidence": evidence }));
        }
        Ok(finished)
    }

    async fn open_worker(
        &self,
        c: &Checkout,
        issue: &Issue,
        attempt: u32,
        worker: Uuid,
        pick: &Pick,
        tip: String,
    ) -> Answer<(Started, Option<String>)> {
        let branch = WorkerBranch { checkout: c.name.clone(), number: issue.number, attempt };
        let name = branch.worker();
        let next = self.next_worktree(c, issue, &branch, &tip).await?;
        let (path, branch, resumed) = (next.path, next.branch, next.resumed);
        let title = format!("{}#{}", c.name, issue.number);
        let brief = worker_brief(c, issue, &branch, resumed.as_ref());
        let role_for = c.role_for(issue.on.role.as_deref());
        let role = Role::Worker {
            id: worker,
            worktree: &path,
            name: &name,
            title: &title,
            brief: &brief,
            role: role_for.map(|(r, _)| r),
        };
        let opened = self.open_session(role, pick)?;
        let role = role_for.zip(opened.role).map(|((dir, from), read)| StartedRole {
            dir: dir.into(),
            from: from.into(),
            read,
        });
        let started = Started {
            checkout: c.name.clone(),
            number: issue.number,
            attempt,
            harness: pick.harness.key().into(),
            account: pick.account.clone(),
            model: pick.model.clone(),
            worktree: path.to_string_lossy().into_owned(),
            branch,
            terminal: opened.terminal,
            brief: opened.brief.to_string_lossy().into_owned(),
            base: tip,
            at: 0,
            role,
        };
        Ok((started, opened.typed))
    }
}

/// The head of a worker's brief: its issue's title and body, as an edit
/// rewrites it ([`App::pass_edit`]).
pub(crate) fn brief_head(i: &factorium_domain::issue::Issue) -> String {
    format!("# {}#{}: {}\n\n{}\n", i.checkout, i.number, i.title, i.body)
}

/// The brief a worker is handed: its issue, what an earlier attempt left in
/// its worktree, and how it finishes.
fn worker_brief(
    c: &factorium_domain::checkout::Checkout,
    i: &factorium_domain::issue::Issue,
    branch: &str,
    resumed: Option<&Resumed>,
) -> String {
    let mut b = brief_head(i);
    if !i.comments.is_empty() {
        b.push_str("\n## Comments\n\n");
        for comment in &i.comments {
            b.push_str(&format!("- {}: {}\n", comment.by, comment.text));
        }
    }
    let made = match resumed {
        None => format!("made from `{}`'s proving branch `{}`", c.name, c.proving),
        Some(r) => {
            b.push_str(&format!(
                "\n## You resume\n\nAn earlier attempt at this issue ended and kept this worktree and its branch \
                 `{branch}`: they hold work that is nowhere else. You resume in them; build on what is there. The \
                 branch may be behind `{}`'s proving branch `{}`: bring it up to date with it yourself (merge it in \
                 and resolve) before you finish.\n",
                c.name, c.proving
            ));
            let list = |b: &mut String, head: &str, items: &[String]| {
                if items.is_empty() {
                    b.push_str(&format!("\n{head}: none.\n"));
                } else {
                    b.push_str(&format!("\n{head}:\n\n"));
                    for item in items {
                        b.push_str(&format!("- `{item}`\n"));
                    }
                }
            };
            list(&mut b, &format!("Commits ahead of `{}`", c.proving), &r.commits);
            list(&mut b, "Files with changes not committed", &r.changes);
            "kept from an earlier attempt".into()
        }
    };
    b.push_str(&format!(
        "\n## How you finish\n\nYou work in this worktree on the branch `{branch}`, {made}. Commit your work on \
         it. Check what you changed, through `factorium worker exclusive -- <command>`: fmt, clippy and the \
         tests of the crates you touched. When the work is committed and those pass, run `factorium worker \
         done`: factorium merges your branch onto the proving branch and runs the repo's fast gate there, the \
         one full gate of your change. `factorium worker gate` runs that gate on your worktree; you may, you \
         need not. To keep a record on your issue (measurements, what you found), run \
         `factorium worker comment --text \"…\"`: it calls no one, so go on. Before a long wait on a background \
         command of your own (a download), run `factorium worker waiting --why \"<what>\" --until <45m|2h>`, or \
         you are reported silent 30 min after your turn ends. If you cannot go on without a \
         decision or something you lack, run `factorium worker escalate \"what you need\"` and stop.\n"
    ));
    b
}
