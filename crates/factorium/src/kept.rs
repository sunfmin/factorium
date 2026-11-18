//! Kept worktrees (SPEC §2.6): for an issue, the worktree its next start
//! uses (the one its last attempt kept, or a fresh one) and what it holds;
//! whether a worker's worktree stays when the worker ends; and how a kept
//! one is discarded on purpose. One test decides all three ([`holds_own`]),
//! and one policy says what an unreadable proving tip means ([`tip`]).

use std::path::{Path, PathBuf};

use factorium_domain::checkout::Checkout;
use factorium_domain::issue::Issue;
use factorium_domain::worker::{Started, Workers};
use factorium_domain::worker_id;
use factorium_host::Git;

use crate::{Answer, App, fail};

/// The worktree a start uses.
pub(crate) struct Next {
    pub path: PathBuf,
    pub branch: String,
    /// What it holds that is nowhere else, if it was kept by an earlier attempt.
    pub resumed: Option<Resumed>,
}

/// What a resumed worktree holds that is nowhere else, as it was found.
pub(crate) struct Resumed {
    /// Its branch's commits not on the proving tip, newest first.
    pub commits: Vec<String>,
    /// Its files with changes not committed.
    pub changes: Vec<String>,
}

impl App {
    /// The worktree of the last attempt at `issue` that started, if any did.
    async fn last_started(&self, issue: &Issue) -> Answer<Option<Started>> {
        for n in (1..=issue.attempts).rev() {
            if let Some(st) = self.state::<Workers>(worker_id(&issue.checkout, issue.number, n)).await?.started {
                return Ok(Some(st));
            }
        }
        Ok(None)
    }

    /// The worktree `issue`'s last attempt kept: still on disk and holding
    /// work that is nowhere else against `tip`.
    async fn kept(&self, git: &Git, issue: &Issue, tip: Option<&str>) -> Answer<Option<Started>> {
        let last = self.last_started(issue).await?;
        Ok(last.filter(|st| Path::new(&st.worktree).exists() && holds_own(git, st, tip)))
    }

    /// The worktree attempt `name` at `issue` starts in: the one its last
    /// attempt kept, with what it holds, else one Orca makes on a new
    /// branch from the proving `tip`.
    pub(crate) async fn next_worktree(&self, c: &Checkout, issue: &Issue, name: &str, tip: &str) -> Answer<Next> {
        let git = self.host.git(&c.path);
        if let Some(st) = self.kept(&git, issue, Some(tip)).await? {
            let here = git.at(&st.worktree);
            let resumed = Resumed {
                commits: here.commits(tip, &st.branch).map_err(fail("git"))?,
                changes: here.changes().map_err(fail("git"))?,
            };
            return Ok(Next { path: PathBuf::from(st.worktree), branch: st.branch, resumed: Some(resumed) });
        }
        let branch = format!("factorium/{name}");
        let path = self.host.orca().worktree(Path::new(&c.path), name, &branch, tip).map_err(fail("orca"))?;
        Ok(Next { path, branch, resumed: None })
    }

    /// Frees an ended worker's worktree: it goes if `remove` and it holds
    /// nothing that is nowhere else.
    pub(crate) async fn release_worker(&self, st: &Started, remove: bool) -> Answer<()> {
        let c = self.checkout(&st.checkout).await?;
        let git = self.host.git(&c.path);
        if remove && !holds_own(&git, st, tip(&git, &c).as_deref()) {
            self.remove_worktree(&git, st)?;
        }
        Ok(())
    }

    /// Discards the worktree `issue`'s last attempt kept, keeping all it
    /// held: its uncommitted changes are committed and its branch is kept
    /// as `<branch>-abandoned`. Answers the worktree and that branch;
    /// `None`: nothing was kept.
    pub(crate) async fn discard_kept(&self, c: &Checkout, issue: &Issue) -> Answer<Option<(String, String)>> {
        let git = self.host.git(&c.path);
        let Some(st) = self.kept(&git, issue, tip(&git, c).as_deref()).await? else { return Ok(None) };
        let tree = git.at(&st.worktree);
        if tree.dirty().map_err(fail("git"))? {
            tree.run(&["add", "-A"]).map_err(fail("git"))?;
            let message = format!("Uncommitted work of {}, kept at start-over", st.branch);
            tree.run(&["commit", "--quiet", "--no-verify", "-m", &message]).map_err(fail("git"))?;
        }
        let abandoned = format!("{}-abandoned", st.branch);
        let head = tree.sha("HEAD").map_err(fail("git"))?;
        git.run(&["branch", "--no-track", &abandoned, &head]).map_err(fail("git"))?;
        self.remove_worktree(&git, &st)?;
        Ok(Some((st.worktree, abandoned)))
    }

    /// Removes a worker's worktree through Orca, then whatever git keeps of
    /// it and its branch.
    fn remove_worktree(&self, git: &Git, st: &Started) -> Answer<()> {
        let path = Path::new(&st.worktree);
        self.host.orca().remove(path).map_err(fail("orca"))?;
        git.forget(path, &st.branch);
        Ok(())
    }
}

/// The proving tip of `c` as origin has it now. `None`, the one tip-unknown
/// policy: what a worktree holds cannot be read against it, so it is held.
fn tip(git: &Git, c: &Checkout) -> Option<String> {
    git.fetch(&c.proving).ok()
}

/// Whether a worker's worktree holds work that is nowhere else: commits of
/// its branch not on the proving `tip` (`None`: the tip could not be read),
/// or changes not committed. What cannot be read is held; a worktree gone
/// from disk holds only its branch's commits.
fn holds_own(git: &Git, st: &Started, tip: Option<&str>) -> bool {
    let committed = tip.is_none_or(|tip| !matches!(git.ahead(tip, &st.branch), Ok(0)));
    let path = Path::new(&st.worktree);
    committed || path.exists() && git.at(path).dirty().unwrap_or(true)
}
