//! Starting sessions: a worker on an issue, and the watch. A worker resumes
//! in the worktree its issue's last attempt kept, if that one holds work
//! that is nowhere else; else it gets a worktree branched from the proving
//! tip ([`crate::kept`]). Either way it gets its brief; the session itself is
//! opened as [`crate::launch`] says.

use factorium_domain::issue::{Take, TakeIssue};
use factorium_domain::watch::{self, StartWatch};
use factorium_domain::worker::{self as w, Started};
use factorium_domain::worker_id;
use factorium_host::Term;
use serde_json::json;

use crate::kept::Resumed;
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
        // Unread, the tip leaves no worktree to start in (SPEC §2.6): nothing is taken.
        let tip = self.host.git(&c.path).fetch(&c.proving).map_err(fail("git"))?;
        self.submit::<TakeIssue>(Take { checkout: checkout.into(), number, worker }).await?;
        match self.open_worker(&c, &issue, attempt, worker, &pick, tip).await {
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

    async fn open_worker(
        &self,
        c: &factorium_domain::checkout::Checkout,
        issue: &factorium_domain::issue::Issue,
        attempt: u32,
        worker: factum::Uuid,
        pick: &Pick,
        tip: String,
    ) -> Answer<Started> {
        let name = format!("{}-{}-{attempt}", c.name, issue.number);
        let next = self.next_worktree(c, issue, &name, &tip).await?;
        let (path, branch, resumed) = (next.path, next.branch, next.resumed);
        let title = format!("{}#{}", c.name, issue.number);
        let brief = worker_brief(c, issue, &branch, resumed.as_ref());
        let role = Role::Worker { id: worker, worktree: &path, name: &name, title: &title, brief: &brief };
        let opened = self.open_session(role, pick)?;
        Ok(Started {
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

/// The brief a worker is handed: its issue, what an earlier attempt left in
/// its worktree, and how it finishes.
fn worker_brief(
    c: &factorium_domain::checkout::Checkout,
    i: &factorium_domain::issue::Issue,
    branch: &str,
    resumed: Option<&Resumed>,
) -> String {
    let mut b = format!("# {}#{}: {}\n\n{}\n", c.name, i.number, i.title, i.body);
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
         it. Run `factorium worker gate` to run the repo's fast gate, as often as you like. When the work is \
         committed and the gate is green, run `factorium worker done`: factorium merges your branch onto the \
         proving branch and runs the gate there. If you cannot go on without a decision or something you lack, run \
         `factorium worker escalate \"what you need\"` and stop.\n"
    ));
    b
}
