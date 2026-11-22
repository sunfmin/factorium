//! Kept worktrees (SPEC §2.6): for an issue, the worktree its next start
//! uses (the one its last attempt kept, or a fresh one) and what it holds;
//! whether a worker's worktree stays when the worker ends; how a kept one
//! is discarded on purpose, or once its issue closes; and the worktrees no
//! open issue owns, which doctor finds. One test decides all three ([`holds_own`]),
//! and one policy says what an unreadable proving tip means ([`tip`]). A
//! worker's branch is named one way ([`WorkerBranch`]), and an attempt is
//! abandoned one way ([`App::abandon`]).

use std::path::{Path, PathBuf};

use factorium_domain::checkout::Checkout;
use factorium_domain::doctor::Orphaned;
use factorium_domain::issue::{Author, Claimant, Issue, Issues, Status};
use factorium_domain::tracker;
use factorium_domain::worker::{Started, Workers};
use factorium_domain::{issue_id, worker_id};
use factorium_host::Git;
use serde_json::json;

use crate::{Answer, App, Refusal, fail};

/// A worker's branch, `factorium/<checkout>-<number>-<attempt>`.
pub(crate) struct WorkerBranch {
    pub checkout: String,
    pub number: u64,
    pub attempt: u32,
}

impl WorkerBranch {
    /// What its worktree and session are named: `<checkout>-<number>-<attempt>`.
    pub fn worker(&self) -> String {
        format!("{}-{}-{}", self.checkout, self.number, self.attempt)
    }

    pub fn name(&self) -> String {
        format!("factorium/{}", self.worker())
    }

    /// The worker's branch of `checkout` that `branch` is, if it is one.
    pub fn parse(checkout: &str, branch: &str) -> Option<WorkerBranch> {
        let rest = branch.strip_prefix("factorium/")?.strip_prefix(checkout)?.strip_prefix('-')?;
        let (number, attempt) = rest.split_once('-')?;
        Some(WorkerBranch { checkout: checkout.into(), number: number.parse().ok()?, attempt: attempt.parse().ok()? })
    }
}

/// The branch an abandoned attempt on `branch` is kept as.
fn abandoned(branch: &str) -> String {
    format!("{branch}-abandoned")
}

/// Why an attempt is abandoned ([`App::abandon`]), each with its reason:
/// its issue closes or starts over, or doctor found it orphaned.
pub(crate) enum Why<'a> {
    Close(&'a str),
    StartOver(&'a str),
    Orphaned(&'a str),
}

impl Why<'_> {
    /// What the commit of uncommitted work says it was kept at, and the
    /// comment that names `kept`, the branch attempt `n` is kept as. A
    /// start-over's prefix is how the domain knows factorium's older comments.
    fn said(&self, n: u32, kept: &str) -> (&'static str, String) {
        let (gone, kept) = (format!("attempt {n}'s worktree"), format!("its branch is kept as `{kept}`"));
        match self {
            Why::Close(_) => ("close", format!("Closed unlanded: {gone} is removed; {kept}.")),
            Why::StartOver(why) => {
                ("start-over", format!("Started over: {gone} was discarded; {kept} (not landed). Why: {why}"))
            }
            Why::Orphaned(why) => ("discard", format!("Discarded {gone} ({why}); {kept} (not landed).")),
        }
    }
}

/// An abandoned attempt: the worktree removed, and the branch it is kept as.
pub(crate) struct Abandoned {
    pub worktree: String,
    pub branch: String,
}

impl Abandoned {
    /// `shown`, saying it: `discarded`, the worktree; `abandoned`, the branch.
    pub fn shown(self, mut shown: serde_json::Value) -> serde_json::Value {
        (shown["discarded"], shown["abandoned"]) = (json!(self.worktree), json!(self.branch));
        shown
    }
}

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

    /// The worktree the attempt on `name` at `issue` starts in: the one its
    /// last attempt kept, with what it holds, else one Orca makes on `name`,
    /// a new branch from the proving `tip`.
    pub(crate) async fn next_worktree(
        &self,
        c: &Checkout,
        issue: &Issue,
        name: &WorkerBranch,
        tip: &str,
    ) -> Answer<Next> {
        let git = self.host.git(&c.path);
        if let Some(st) = self.kept(&git, issue, Some(tip)).await? {
            let here = git.at(&st.worktree);
            let resumed = Resumed {
                commits: here.commits(tip, &st.branch).map_err(fail("git"))?,
                changes: here.changes().map_err(fail("git"))?,
            };
            return Ok(Next { path: PathBuf::from(st.worktree), branch: st.branch, resumed: Some(resumed) });
        }
        let branch = name.name();
        let path = self.host.orca().worktree(Path::new(&c.path), &name.worker(), &branch, tip).map_err(fail("orca"))?;
        Ok(Next { path, branch, resumed: None })
    }

    /// Frees an ended worker's worktree: it goes if `remove` and it holds
    /// nothing that is nowhere else.
    pub(crate) async fn release_worker(&self, st: &Started, remove: bool) -> Answer<()> {
        let c = self.checkout(&st.checkout).await?;
        let git = self.host.git(&c.path);
        let path = Path::new(&st.worktree);
        if !remove || holds_own(&git, path, &st.branch, tip(&git, &c).as_deref()) {
            return Ok(());
        }
        // A nested worktree holding work holds this one, as its own work does: doctor names it.
        match clear_nested(&git, &c, path) {
            Err(r) if r.code == "nested_held" => Ok(()),
            cleared => cleared.and_then(|()| self.remove_worktree(&git, path, &st.branch)),
        }
    }

    /// The worktree `issue`'s last attempt kept, holding work that is
    /// nowhere else against `c`'s proving tip as origin has it now.
    pub(crate) async fn kept_now(&self, c: &Checkout, issue: &Issue) -> Answer<Option<Started>> {
        let git = self.host.git(&c.path);
        self.kept(&git, issue, tip(&git, c).as_deref()).await
    }

    /// The worktree `issue`'s last attempt left on disk, whatever it holds.
    pub(crate) async fn left(&self, issue: &Issue) -> Answer<Option<Started>> {
        Ok(self.last_started(issue).await?.filter(|st| Path::new(&st.worktree).exists()))
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
                let Some(w) = branch.as_deref().and_then(|b| WorkerBranch::parse(&c.name, b)) else { continue };
                let (number, attempt) = (w.number, w.attempt);
                let issue = self.state::<Issues>(issue_id(&c.name, number)).await?;
                let name = format!("{}#{number}", c.name);
                let why = if !issue.opened() {
                    format!("{name} is not an issue here")
                } else if issue.status == Status::Closed {
                    format!("{name} is closed")
                } else {
                    // Owned: the last started attempt's own worktree, whatever
                    // attempt its branch names; the attempt's whose worker
                    // holds the issue (the newest), still starting.
                    let last = self.last_started(&issue).await?;
                    if last.as_ref().is_some_and(|l| branch.as_deref() == Some(l.branch.as_str()))
                        || issue.worker == Some(worker_id(&c.name, number, attempt))
                    {
                        continue;
                    }
                    let last = last.map_or("none".to_string(), |l| l.attempt.to_string());
                    format!("attempt {attempt} is neither {name}'s last started ({last}) nor starting")
                };
                let (checkout, path, branch) = (c.name.clone(), path.display().to_string(), branch.unwrap_or_default());
                let kept = abandoned(&branch);
                let nested = held(&nested(&self.host.git(&c.path), &c, Path::new(&path)));
                found.push(Orphaned { checkout, number, attempt, path, branch, kept, why, nested });
            }
        }
        Ok(found)
    }

    /// `discard`: abandons a worktree doctor finds that no open issue's
    /// attempt owns ([`App::abandon`]). Refused for any other path. One of
    /// an open issue is found again under its claim ([`App::claimed`]), so
    /// no start resumes it between the find and the abandon.
    pub async fn discard_orphaned(&self, worktree: &Path) -> Answer {
        let o = self.found_orphan(worktree).await?;
        let issue = self.state::<Issues>(issue_id(&o.checkout, o.number)).await?;
        if !issue.opened() {
            return self.discard_found(worktree).await;
        }
        self.claimed(&o.checkout, o.number, Claimant::Discard, self.discard_found(worktree)).await
    }

    /// The worktree at `worktree`, as doctor finds it no open issue owns.
    async fn found_orphan(&self, worktree: &Path) -> Answer<Orphaned> {
        let at = worktree.display().to_string();
        let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let found = self.orphaned().await?.into_iter().find(|o| real(Path::new(&o.path)) == real(worktree));
        found.ok_or_else(|| {
            let why = format!("{at} is no worker's worktree that no open issue owns: doctor names those");
            Refusal::new("not_orphaned", why)
        })
    }

    async fn discard_found(&self, worktree: &Path) -> Answer {
        let at = worktree.display().to_string();
        let o = self.found_orphan(worktree).await?;
        let (c, Orphaned { checkout, number, attempt, branch, why, .. }) = (self.checkout(&o.checkout).await?, o);
        let st = Started { checkout, number, attempt, worktree: at, branch, ..Started::default() };
        Ok(self.abandon(&c, &st, Why::Orphaned(&why)).await?.shown(json!({ "why": why })))
    }

    /// Abandons attempt `st`, keeping all its worktree holds: uncommitted
    /// changes are committed, its branch is kept as `<branch>-abandoned`,
    /// which a comment says as `why` does (a close's comes first; a closed
    /// issue takes no other), and the worktree goes, with its terminals.
    /// Refused, before anything changes, while a nested worktree in it
    /// holds work ([`nested`]): that is kept where it is, and named.
    pub(crate) async fn abandon(&self, c: &Checkout, st: &Started, why: Why<'_>) -> Answer<Abandoned> {
        let (git, path) = (self.host.git(&c.path), Path::new(&st.worktree));
        clear_nested(&git, c, path)?;
        let kept = abandoned(&st.branch);
        let (at, comment) = why.said(st.attempt, &kept);
        if let Why::Close(reason) = why {
            self.note(&st.checkout, st.number, Author::Factorium, &comment).await?;
            self.close(&st.checkout, st.number, reason, true).await?;
        }
        let tree = git.at(path);
        if tree.dirty().map_err(fail("git"))? {
            tree.run(&["add", "-A"]).map_err(fail("git"))?;
            let message = format!("Uncommitted work of {}, kept at {at}", st.branch);
            tree.run(&["commit", "--quiet", "--no-verify", "-m", &message]).map_err(fail("git"))?;
        }
        let head = tree.sha("HEAD").map_err(fail("git"))?;
        // Forced: an abandon Orca refused before made it, at an ancestor of HEAD.
        git.run(&["branch", "--no-track", "--force", &kept, &head]).map_err(fail("git"))?;
        self.remove_worktree(&git, path, &st.branch)?;
        let open = self.issue(&st.checkout, st.number).await.is_ok_and(|i| i.status != Status::Closed);
        if open && !matches!(why, Why::Close(_)) {
            self.note(&st.checkout, st.number, Author::Factorium, &comment).await?;
        }
        Ok(Abandoned { worktree: st.worktree.clone(), branch: kept })
    }

    /// Removes a worker's worktree through Orca, with its terminals, then
    /// whatever git keeps of it and its branch. Orca refuses one that holds
    /// another git worktree: [`clear_nested`] comes first.
    fn remove_worktree(&self, git: &Git, path: &Path, branch: &str) -> Answer<()> {
        self.host.orca().remove(path).map_err(fail("orca"))?;
        git.forget(path, branch);
        Ok(())
    }
}

/// The proving tip of `c` as origin has it now. `None`, the one tip-unknown
/// policy: what a worktree holds cannot be read against it, so it is held.
fn tip(git: &Git, c: &Checkout) -> Option<String> {
    git.fetch(&c.proving).ok()
}

/// The git worktrees of `c` registered inside the worktree at `path`, each
/// with why it holds work (`None`: it does not, and may go). One goes only
/// when it is clean and detached at a commit on `c`'s proving or base branch
/// as last fetched, as slimfit's comparison worktree is. What cannot be read
/// is held.
fn nested(git: &Git, c: &Checkout, path: &Path) -> Vec<(PathBuf, Option<String>)> {
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let tips = [c.fetched_base(), format!("refs/remotes/origin/{}", c.proving)];
    let why = |p: &Path, branch: Option<String>| {
        let here = git.at(p);
        let landed = here.sha("HEAD").is_ok_and(|sha| tips.iter().any(|t| git.is_ancestor(&sha, t)));
        match branch {
            Some(b) => Some(format!("it is on branch `{b}`, not detached")),
            None if here.dirty().unwrap_or(true) => Some("it has changes not committed".into()),
            None if !landed => Some(format!("its HEAD is on neither {} nor {}", c.proving, c.base)),
            None => None,
        }
    };
    let (outer, trees) = (real(path), git.worktrees().unwrap_or_default());
    let inside = trees.into_iter().filter(|(p, _)| p.exists() && real(p) != outer && real(p).starts_with(&outer));
    inside.map(|(p, b)| (p.clone(), why(&p, b))).collect()
}

/// Of `nested` ([`nested`]), those that hold work, each as `<path>: <why>`.
fn held(nested: &[(PathBuf, Option<String>)]) -> Vec<String> {
    nested.iter().filter_map(|(p, why)| Some(format!("{}: {}", p.display(), why.as_ref()?))).collect()
}

/// Removes the nested worktrees in `path` ([`nested`]), before what it
/// holds is committed (they are no work of its own) and before Orca removes
/// it; refused, removing none and naming those that hold work, while any does.
fn clear_nested(git: &Git, c: &Checkout, path: &Path) -> Answer<()> {
    let inner = nested(git, c, path);
    let held = held(&inner);
    if held.is_empty() {
        for (inner, _) in inner {
            git.worktree_remove(&inner).map_err(fail("git"))?;
        }
        return Ok(());
    }
    let why = format!(
        "{} holds a nested worktree with work, kept where it is: {}. Commit that elsewhere or remove it, then try again",
        path.display(),
        held.join("; ")
    );
    Err(Refusal::new("nested_held", why))
}

/// Whether a worker's worktree at `path`, on `branch`, holds work that is
/// nowhere else: commits of its branch not on the proving `tip` (`None`:
/// the tip could not be read), or changes not committed. What cannot be
/// read is held; a worktree gone from disk holds only its branch's commits.
fn holds_own(git: &Git, path: &Path, branch: &str, tip: Option<&str>) -> bool {
    let committed = tip.is_none_or(|tip| !matches!(git.ahead(tip, branch), Ok(0)));
    committed || path.exists() && git.at(path).dirty().unwrap_or(true)
}
