//! git, as a program: every call runs `git` in a directory and answers what
//! it printed, or why it failed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct Git {
    pub dir: PathBuf,
}

impl Git {
    pub fn at(dir: impl Into<PathBuf>) -> Git {
        Git { dir: dir.into() }
    }

    /// Runs git; its output trimmed, or its error.
    pub fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut git = Command::new("git");
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

    /// The commit `rev` names.
    pub fn sha(&self, rev: &str) -> Result<String, String> {
        self.run(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
    }

    /// The tree of `rev`.
    pub fn tree(&self, rev: &str) -> Result<String, String> {
        self.run(&["rev-parse", &format!("{rev}^{{tree}}")])
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

    /// Adds a detached worktree at `path` on `rev`.
    pub fn worktree_add(&self, path: &Path, rev: &str) -> Result<(), String> {
        let _ = self.run(&["worktree", "prune"]);
        self.run(&["worktree", "add", "--quiet", "--detach", &path.to_string_lossy(), rev]).map(|_| ())
    }

    /// Removes the worktree at `path`; one already gone is removed.
    pub fn worktree_remove(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            let _ = self.run(&["worktree", "prune"]);
            return Ok(());
        }
        self.run(&["worktree", "remove", "--force", &path.to_string_lossy()]).map(|_| ())
    }

    /// Whether the worktree has changes not committed.
    pub fn dirty(&self) -> Result<bool, String> {
        Ok(!self.run(&["status", "--porcelain", "--untracked-files=normal"])?.is_empty())
    }

    /// The commits of `head` not in `base`.
    pub fn ahead(&self, base: &str, head: &str) -> Result<u64, String> {
        self.run(&["rev-list", "--count", &format!("{base}..{head}")])?.parse().map_err(|e| format!("{e}"))
    }

    /// The tags that look like versions, newest by version first.
    pub fn version_tags(&self) -> Vec<String> {
        self.run(&["tag", "--list", "v[0-9]*", "--sort=-version:refname"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The local branch checked out here, if any.
    pub fn branch(&self) -> Option<String> {
        self.run(&["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
    }
}
