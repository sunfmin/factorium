//! Kept worktrees (SPEC §2.6): for an issue, the worktree its next start
//! uses (the one its last attempt kept, or a fresh one) and what it holds;
//! whether a worker's worktree stays when the worker ends; how a kept one
//! is discarded on purpose, or once its issue closes; and the worktrees no
//! open issue owns, which doctor finds. One test decides all three ([`holds_own`]),
//! and one policy says what an unreadable proving tip means ([`tip`]).

use std::path::{Path, PathBuf};

use factorium_domain::checkout::Checkout;
use factorium_domain::doctor::Orphaned;
use factorium_domain::issue::{Author, Issue, Issues, Status};
use factorium_domain::tracker;
use factorium_domain::worker::{Started, Workers};
use factorium_domain::{issue_id, worker_id};
use factorium_host::Git;
use serde_json::json;

use crate::{Answer, App, Refusal, fail};

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
        Ok(last
            .filter(|st| Path::new(&st.worktree).exists() && holds_own(git, Path::new(&st.worktree), &st.branch, tip)))
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
        if remove && !holds_own(&git, Path::new(&st.worktree), &st.branch, tip(&git, &c).as_deref()) {
            self.remove_worktree(&git, Path::new(&st.worktree), &st.branch)?;
        }
        Ok(())
    }

    /// Discards the worktree `issue`'s last attempt kept ([`App::discard`];
    /// `at` says when). Answers the worktree and the branch it is kept as;
    /// `None`: nothing was kept.
    pub(crate) async fn discard_kept(&self, c: &Checkout, issue: &Issue, at: &str) -> Answer<Option<(String, String)>> {
        let git = self.host.git(&c.path);
        let Some(st) = self.kept(&git, issue, tip(&git, c).as_deref()).await? else { return Ok(None) };
        let abandoned = self.discard(&git, Path::new(&st.worktree), &st.branch, at)?;
        Ok(Some((st.worktree, abandoned)))
    }

    /// The worktree `issue`'s last attempt left on disk, whatever it holds.
    pub(crate) async fn left(&self, issue: &Issue) -> Answer<Option<Started>> {
        Ok(self.last_started(issue).await?.filter(|st| Path::new(&st.worktree).exists()))
    }

    /// Discards the worktree `st` left as a close does ([`App::discard`]):
    /// answers the branch it is kept as.
    pub(crate) fn discard_left(&self, c: &Checkout, st: &Started) -> Answer<String> {
        self.discard(&self.host.git(&c.path), Path::new(&st.worktree), &st.branch, "close")
    }

    /// The worktrees of workers that no open issue's attempt owns (SPEC
    /// §2.6): on a worker's branch, `factorium/<checkout>-<n>-<attempt>`,
    /// of an issue that is closed or this database does not hold, or not its
    /// last started attempt's (which may resume an older attempt's branch).
    pub(crate) async fn orphaned(&self) -> Answer<Vec<Orphaned>> {
        let mut found = Vec::new();
        for c in self.rows(tracker::listed()).await? {
            let Ok(trees) = self.host.git(&c.path).worktrees() else { continue };
            for (path, branch) in trees {
                let Some((number, attempt)) = branch.as_deref().and_then(|b| attempt_of(&c.name, b)) else {
                    continue;
                };
                let issue = self.state::<Issues>(issue_id(&c.name, number)).await?;
                let name = format!("{}#{number}", c.name);
                let why = if !issue.opened() {
                    format!("{name} is not an issue here")
                } else if issue.status == Status::Closed {
                    format!("{name} is closed")
                } else {
                    let Some(last) = self.last_started(&issue).await? else { continue };
                    if branch.as_deref() == Some(last.branch.as_str()) || attempt > last.attempt {
                        continue;
                    }
                    format!("attempt {attempt} is older than {name}'s last, {}", last.attempt)
                };
                let (checkout, path, branch) = (c.name.clone(), path.display().to_string(), branch.unwrap_or_default());
                found.push(Orphaned { checkout, number, attempt, path, branch, why });
            }
        }
        Ok(found)
    }

    /// `discard`: removes a worktree doctor finds that no open issue's
    /// attempt owns ([`App::discard`]); a comment on its issue, if that is
    /// open, names the branch it is kept as. Refused for any other path.
    pub async fn discard_orphaned(&self, worktree: &Path) -> Answer {
        let at = worktree.display().to_string();
        let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let found = self.orphaned().await?.into_iter().find(|o| real(Path::new(&o.path)) == real(worktree));
        let Some(o) = found else {
            let why = format!("{at} is no worker's worktree that no open issue owns: doctor names those");
            return Err(Refusal::new("not_orphaned", why));
        };
        let c = self.checkout(&o.checkout).await?;
        let abandoned = self.discard(&self.host.git(&c.path), worktree, &o.branch, "discard")?;
        // A closed issue takes no comment: the answer names the branch.
        if self.issue(&o.checkout, o.number).await.is_ok_and(|i| i.status != Status::Closed) {
            let text = format!(
                "Discarded attempt {}'s worktree ({}); its branch is kept as `{abandoned}` (not landed).",
                o.attempt, o.why
            );
            self.note(&o.checkout, o.number, Author::Factorium, &text).await?;
        }
        Ok(json!({ "discarded": at, "abandoned": abandoned, "why": o.why }))
    }

    /// Discards the worktree at `path`, on `branch`, keeping all it holds:
    /// its uncommitted changes are committed (the message says at `at`)
    /// and its branch is kept as `<branch>-abandoned`, which this answers.
    /// Then it goes, with its terminals and its branch.
    fn discard(&self, git: &Git, path: &Path, branch: &str, at: &str) -> Answer<String> {
        let tree = git.at(path);
        if tree.dirty().map_err(fail("git"))? {
            tree.run(&["add", "-A"]).map_err(fail("git"))?;
            let message = format!("Uncommitted work of {branch}, kept at {at}");
            tree.run(&["commit", "--quiet", "--no-verify", "-m", &message]).map_err(fail("git"))?;
        }
        let kept = format!("{branch}-abandoned");
        let head = tree.sha("HEAD").map_err(fail("git"))?;
        // Forced: a discard Orca refused before made it, at an ancestor of HEAD.
        git.run(&["branch", "--no-track", "--force", &kept, &head]).map_err(fail("git"))?;
        self.remove_worktree(git, path, branch)?;
        Ok(kept)
    }

    /// Removes a worker's worktree through Orca, with its terminals, then
    /// whatever git keeps of it and its branch.
    fn remove_worktree(&self, git: &Git, path: &Path, branch: &str) -> Answer<()> {
        self.host.orca().remove(path).map_err(fail("orca"))?;
        git.forget(path, branch);
        Ok(())
    }
}

/// The issue number and attempt a worker's branch of `checkout` names.
fn attempt_of(checkout: &str, branch: &str) -> Option<(u64, u32)> {
    let rest = branch.strip_prefix("factorium/")?.strip_prefix(checkout)?.strip_prefix('-')?;
    let (number, attempt) = rest.split_once('-')?;
    Some((number.parse().ok()?, attempt.parse().ok()?))
}

/// The proving tip of `c` as origin has it now. `None`, the one tip-unknown
/// policy: what a worktree holds cannot be read against it, so it is held.
fn tip(git: &Git, c: &Checkout) -> Option<String> {
    git.fetch(&c.proving).ok()
}

/// Whether a worker's worktree at `path`, on `branch`, holds work that is
/// nowhere else: commits of its branch not on the proving `tip` (`None`:
/// the tip could not be read), or changes not committed. What cannot be
/// read is held; a worktree gone from disk holds only its branch's commits.
fn holds_own(git: &Git, path: &Path, branch: &str, tip: Option<&str>) -> bool {
    let committed = tip.is_none_or(|tip| !matches!(git.ahead(tip, branch), Ok(0)));
    committed || path.exists() && git.at(path).dirty().unwrap_or(true)
}
