//! Orca: the worktrees agents work in and the terminals they run in. Every
//! call is `orca … --json`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// A new worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Made {
    pub path: PathBuf,
    pub branch: String,
}

pub trait Orca: Send + Sync {
    /// Makes a worktree of the repo at `repo`, named `name`, on a new branch
    /// `branch` at `sha`.
    fn worktree(&self, repo: &Path, name: &str, branch: &str, sha: &str) -> Result<Made, String>;
    /// Removes a worktree and the terminals in it.
    fn remove(&self, worktree: &Path) -> Result<(), String>;
    /// Starts a terminal in `worktree` running `line`; answers its handle.
    fn open(&self, worktree: &Path, title: &str, line: &str) -> Result<String, String>;
    /// Types `text` and Enter at a terminal.
    fn send(&self, terminal: &str, text: &str) -> Result<(), String>;
    /// The terminal's screen as rendered, or `None` when it is gone.
    fn screen(&self, terminal: &str) -> Option<Vec<String>>;
    /// Ends a terminal.
    fn close(&self, terminal: &str);
}

pub struct OrcaCli;

fn orca(args: &[&str]) -> Result<Value, String> {
    let out = Command::new("orca")
        .args(args)
        .arg("--json")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("orca: {e}"))?;
    let answer: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| format!("orca {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))?;
    if !out.status.success() || answer["ok"] != true {
        return Err(format!("orca {}: {}", args.join(" "), answer["error"]));
    }
    Ok(answer["result"].clone())
}

impl Orca for OrcaCli {
    fn worktree(&self, repo: &Path, name: &str, branch: &str, sha: &str) -> Result<Made, String> {
        let repo_sel = format!("path:{}", repo.display());
        if orca(&["repo", "show", "--repo", &repo_sel]).is_err() {
            orca(&["repo", "add", "--path", &repo.to_string_lossy()])?;
        }
        let made = orca(&[
            "worktree",
            "create",
            "--repo",
            &repo_sel,
            "--name",
            name,
            "--branch",
            branch,
            "--base-branch",
            sha,
            "--no-parent",
            "--setup",
            "skip",
        ])?;
        let path = made["worktree"]["path"].as_str().map(PathBuf::from).filter(|p| p.is_dir());
        let path = path.ok_or_else(|| format!("orca worktree create returned no usable path: {made}"))?;
        Ok(Made { path, branch: branch.to_string() })
    }

    fn remove(&self, worktree: &Path) -> Result<(), String> {
        let sel = format!("path:{}", worktree.display());
        match orca(&["worktree", "rm", "--worktree", &sel, "--force"]) {
            Err(_) if !worktree.exists() => Ok(()),
            other => other.map(|_| ()),
        }
    }

    fn open(&self, worktree: &Path, title: &str, line: &str) -> Result<String, String> {
        let sel = format!("path:{}", worktree.display());
        let made = orca(&["terminal", "create", "--worktree", &sel, "--title", title, "--command", line])?;
        made["terminal"]["handle"].as_str().map(str::to_string).ok_or_else(|| "orca gave no terminal handle".into())
    }

    fn send(&self, terminal: &str, text: &str) -> Result<(), String> {
        orca(&["terminal", "send", "--terminal", terminal, "--text", text, "--enter"]).map(|_| ())
    }

    fn screen(&self, terminal: &str) -> Option<Vec<String>> {
        let read = orca(&["terminal", "read", "--terminal", terminal, "--screen", "--limit", "200"]).ok()?;
        let lines = read["lines"].as_array().or_else(|| read["terminal"]["lines"].as_array())?;
        Some(lines.iter().map(|l| l.as_str().unwrap_or_default().to_string()).collect())
    }

    fn close(&self, terminal: &str) {
        let _ = orca(&["terminal", "close", "--terminal", terminal]);
    }
}
