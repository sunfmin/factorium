//! git, as a program: every call runs `git` in a directory as the host runs
//! it — with the host's home, so with the config that home holds and no
//! other — and answers what it printed, or why it failed. The app makes one
//! through [`crate::Host`]; a checkout's repo is one value, and what the app
//! does to it (fetch a tip, a fresh work dir, a push that requires what it
//! read, a file at a rev, a tag) is a method here, not git arguments there.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct Git {
    pub dir: PathBuf,
    home: PathBuf,
}

impl Git {
    /// git in `dir` as the host whose home is `home` runs it.
    pub fn new(home: impl Into<PathBuf>, dir: impl Into<PathBuf>) -> Git {
        Git { dir: dir.into(), home: home.into() }
    }

    /// git in `dir`, as the same host.
    pub fn at(&self, dir: impl Into<PathBuf>) -> Git {
        Git::new(self.home.clone(), dir)
    }

    /// Runs git; its output trimmed, or its error.
    pub fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut git = Command::new("git");
        // The host's own config is what its home holds.
        git.env("HOME", &self.home).env_remove("XDG_CONFIG_HOME").env_remove("GIT_CONFIG_GLOBAL");
        // What factorium itself commits (a landing merge, a cut tag, the
        // watch's home) needs an identity; a host with none commits as factorium.
        if matches!(args.first(), Some(&("commit" | "merge" | "tag"))) && !self.holds(&["config", "user.email"]) {
            for v in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
                git.env(v, "factorium");
            }
            for v in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
                git.env(v, "factorium@localhost");
            }
        }
        let out = git
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
        let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() {
            Ok(said)
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(format!("git {}: {}", args.join(" "), if err.is_empty() { said } else { err }))
        }
    }

    /// Whether git answers yes (exit 0).
    pub fn holds(&self, args: &[&str]) -> bool {
        self.run(args).is_ok()
    }

    /// Makes `dir` a repo of one empty commit, unless it is one.
    pub fn init(&self, message: &str) -> Result<(), String> {
        if self.dir.join(".git").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        self.run(&["init", "--quiet"])?;
        self.run(&["commit", "--quiet", "--allow-empty", "-m", message]).map(|_| ())
    }

    /// The commit `rev` names.
    pub fn sha(&self, rev: &str) -> Result<String, String> {
        self.run(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
    }

    /// The tree of `rev`.
    pub fn tree(&self, rev: &str) -> Result<String, String> {
        self.run(&["rev-parse", &format!("{rev}^{{tree}}")])
    }

    /// The text of `file` at `rev`, if it has one.
    pub fn shown(&self, rev: &str, file: &str) -> Option<String> {
        self.run(&["show", &format!("{rev}:{file}")]).ok()
    }

    /// Fetches `branch` from origin; answers its tip there.
    pub fn fetch(&self, branch: &str) -> Result<String, String> {
        self.run(&["fetch", "--quiet", "origin", &format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")])?;
        self.sha(&format!("refs/remotes/origin/{branch}"))
    }

    /// Whether `a` is an ancestor of `b`.
    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.holds(&["merge-base", "--is-ancestor", a, b])
    }

    /// Pushes `sha` to origin's `branch`, but only while `branch` there is
    /// still `expected` (`None`: absent). The push requires what it read.
    pub fn push_requiring(&self, sha: &str, branch: &str, expected: Option<&str>) -> Result<(), String> {
        let lease = format!("--force-with-lease=refs/heads/{branch}:{}", expected.unwrap_or(""));
        self.run(&["push", "--quiet", &lease, "origin", &format!("{sha}:refs/heads/{branch}")]).map(|_| ())
    }

    /// A work dir of this repo at `path`, detached on `rev`: a clone sharing
    /// this repo's objects, whose `origin` is this one's origin, keeping its
    /// ignored files (builds) from before. Neither git nor Orca counts it
    /// among this repo's worktrees; one made as a worktree before is made
    /// again as a clone.
    pub fn fresh(&self, path: &Path, rev: &str) -> Result<Git, String> {
        let here = self.at(path);
        if path.join(".git").is_file() {
            self.worktree_remove(path)?;
        }
        if path.join(".git").is_dir() {
            here.run(&["checkout", "--quiet", "--detach", "--force", rev])?;
            here.run(&["clean", "-fdq"])?;
            return Ok(here);
        }
        std::fs::create_dir_all(path.parent().unwrap_or(path)).map_err(|e| e.to_string())?;
        let origin = self.run(&["remote", "get-url", "origin"])?;
        let (from, to) = (self.dir.to_string_lossy(), path.to_string_lossy());
        self.run(&["clone", "--quiet", "--shared", "--no-checkout", &from, &to])?;
        here.run(&["remote", "set-url", "origin", &origin])?;
        here.run(&["checkout", "--quiet", "--detach", rev])?;
        Ok(here)
    }

    /// Merges `branch` of the repo at `from` here as one merge commit with
    /// `message`; a merge that fails is aborted.
    pub fn merge_from(&self, from: &Path, branch: &str, message: &str) -> Result<String, String> {
        let refspec = format!("+refs/heads/{branch}:refs/heads/{branch}");
        let merged = self
            .run(&["fetch", "--quiet", &from.to_string_lossy(), &refspec])
            .and_then(|_| self.run(&["merge", "--no-ff", "--no-edit", "-m", message, branch]));
        if merged.is_err() {
            let _ = self.run(&["merge", "--abort"]);
        }
        merged?;
        self.sha("HEAD")
    }

    /// Tags `sha` as `version` and pushes the tag to origin.
    pub fn tag_pushed(&self, version: &str, sha: &str) -> Result<(), String> {
        self.run(&["tag", "-a", version, "-m", version, sha])?;
        self.run(&["push", "--quiet", "origin", &format!("refs/tags/{version}")]).map(|_| ())
    }

    /// Removes the worktree at `path`; one already gone is removed.
    pub fn worktree_remove(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            let _ = self.run(&["worktree", "prune"]);
            return Ok(());
        }
        self.run(&["worktree", "remove", "--force", &path.to_string_lossy()]).map(|_| ())
    }

    /// Forgets a worker's worktree at `path` and its `branch`, whatever of
    /// them is left.
    pub fn forget(&self, path: &Path, branch: &str) {
        let _ = self.worktree_remove(path);
        let _ = self.run(&["branch", "-D", branch]);
    }

    /// Whether the worktree has changes not committed.
    pub fn dirty(&self) -> Result<bool, String> {
        Ok(!self.run(&["status", "--porcelain", "--untracked-files=normal"])?.is_empty())
    }

    /// The commits of `head` not in `base`, newest first, one line each.
    pub fn commits(&self, base: &str, head: &str) -> Result<Vec<String>, String> {
        let log = self.run(&["log", "--oneline", "--no-decorate", &format!("{base}..{head}")])?;
        Ok(log.lines().map(str::to_string).collect())
    }

    /// The files with changes not committed: changed since `HEAD`, staged or
    /// not, then those git does not track (and does not ignore).
    pub fn changes(&self) -> Result<Vec<String>, String> {
        let changed = self.run(&["diff", "--name-only", "HEAD"])?;
        let untracked = self.run(&["ls-files", "--others", "--exclude-standard"])?;
        Ok(changed.lines().chain(untracked.lines()).map(str::to_string).collect())
    }

    /// The commits of `head` not in `base`.
    pub fn ahead(&self, base: &str, head: &str) -> Result<u64, String> {
        self.run(&["rev-list", "--count", &format!("{base}..{head}")])?.parse().map_err(|e| format!("{e}"))
    }

    /// Origin's tags that look like versions, newest by version first.
    pub fn version_tags(&self) -> Vec<String> {
        let _ = self.run(&["fetch", "--quiet", "--tags", "origin"]);
        self.run(&["tag", "--list", "v[0-9]*", "--sort=-version:refname"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The local branch checked out here, if any.
    pub fn branch(&self) -> Option<String> {
        self.run(&["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
    }
}
