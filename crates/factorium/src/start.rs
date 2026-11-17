//! Starting sessions: a worker on an issue, and the watch. A worker gets a
//! worktree branched from the proving tip and its brief; the session itself
//! is opened as [`crate::launch`] says.

use std::path::Path;

use factorium_domain::issue::{Take, TakeIssue};
use factorium_domain::watch::{self, StartWatch};
use factorium_domain::worker::{self as w, Started};
use factorium_domain::worker_id;
use factorium_host::Term;
use serde_json::json;

use crate::launch::{Opened, Pick, Role};
use crate::{Answer, App, fail};

impl App {
    /// Starts a worker on issue `number` of `checkout`.
    pub(crate) async fn start_worker(&self, checkout: &str, number: u64) -> Answer {
        let c = self.checkout(checkout).await?;
        let pick = self.pick(checkout).await?;
        let issue = self.issue(checkout, number).await?;
        let attempt = issue.attempts + 1;
        let worker = worker_id(checkout, number, attempt);
        self.submit::<TakeIssue>(Take { checkout: checkout.into(), number, worker }).await?;
        match self.open_worker(&c, &issue, attempt, worker, &pick) {
            Ok(st) => {
                let shown = json!({ "worker": worker, "issue": format!("{checkout}#{number}"), "harness": st.harness, "account": st.account, "worktree": st.worktree });
                self.submit::<w::StartWorker>(w::Start { worker, started: st }).await?;
                self.hand_worker_brief(worker, true).await?;
                Ok(shown)
            }
            Err(r) => {
                let release = Take { checkout: checkout.into(), number, worker };
                self.submit::<factorium_domain::issue::ReleaseIssue>(release).await?;
                Err(r)
            }
        }
    }

    fn open_worker(
        &self,
        c: &factorium_domain::checkout::Checkout,
        issue: &factorium_domain::issue::Issue,
        attempt: u32,
        worker: factum::Uuid,
        pick: &Pick,
    ) -> Answer<Started> {
        let git = self.host.git(&c.path);
        let tip = git.fetch(&c.proving).map_err(fail("git"))?;
        let name = format!("{}-{}-{attempt}", c.name, issue.number);
        let branch = format!("factorium/{name}");
        let made = self.host.orca().worktree(Path::new(&c.path), &name, &branch, &tip).map_err(fail("orca"))?;
        let title = format!("{}#{}", c.name, issue.number);
        let brief = worker_brief(c, issue, &branch);
        let role = Role::Worker { id: worker, worktree: &made.path, name: &name, title: &title, brief: &brief };
        let opened = self.open_session(role, pick)?;
        Ok(Started {
            checkout: c.name.clone(),
            number: issue.number,
            attempt,
            harness: pick.harness.key().into(),
            account: pick.account.clone(),
            model: pick.model.clone(),
            worktree: made.path.to_string_lossy().into_owned(),
            branch: made.branch,
            terminal: opened.terminal,
            brief: opened.brief.to_string_lossy().into_owned(),
            base: tip,
            at: 0,
            typed: opened.typed,
        })
    }

    /// Starts the watch's session if the kit has a harness and none runs:
    /// answers its terminal, or `None` when it cannot run.
    pub(crate) async fn ensure_watch(&self, before: &watch::Watch) -> Answer<Option<String>> {
        if let Some(t) = &before.terminal {
            match self.host.orca().screen(t) {
                Term::Running(_) => return Ok(Some(t.clone())),
                // Not known to be gone: no second session; none to type at now.
                Term::Unreadable(_) => return Ok(None),
                Term::Exited => {}
            }
        }
        let pick = match self.pick("").await {
            Ok(pick) => pick,
            Err(r) if r.code == "no_harness" => return Ok(None),
            Err(r) => return Err(r),
        };
        let Opened { terminal, typed, .. } = self.open_session(Role::Watch, &pick)?;
        self.submit::<StartWatch>(watch::Started {
            terminal: terminal.clone(),
            harness: pick.harness.key().into(),
            account: pick.account.clone(),
            typed,
        })
        .await?;
        self.hand_watch_brief(true).await?;
        Ok(Some(terminal))
    }
}

/// The brief a worker is handed: its issue and how it finishes.
fn worker_brief(c: &factorium_domain::checkout::Checkout, i: &factorium_domain::issue::Issue, branch: &str) -> String {
    let mut b = format!("# {}#{}: {}\n\n{}\n", c.name, i.number, i.title, i.body);
    if !i.comments.is_empty() {
        b.push_str("\n## Comments\n\n");
        for comment in &i.comments {
            b.push_str(&format!("- {comment}\n"));
        }
    }
    b.push_str(&format!(
        "\n## How you finish\n\nYou work in this worktree on the branch `{branch}`, made from `{}`'s proving branch \
         `{}`. Commit your work on it. Run `factorium worker gate` to run the repo's fast gate, as often as you \
         like. When the work is committed and the gate is green, run `factorium worker done`: factorium merges your \
         branch onto the proving branch and runs the gate there. If you cannot go on without a decision or \
         something you lack, run `factorium worker escalate \"what you need\"` and stop.\n",
        c.name, c.proving
    ));
    b
}
