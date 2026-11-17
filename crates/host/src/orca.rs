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

/// Runs `orca <args> --json`. An error names the subcommand alone, never
/// the arguments: a terminal's command line is one, and may be long.
fn orca(args: &[&str]) -> Result<Value, String> {
    let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
    let out = Command::new("orca")
        .args(args)
        .arg("--json")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("orca: {e}"))?;
    let answer: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| format!("orca {what}: {}", String::from_utf8_lossy(&out.stderr).trim()))?;
    if !out.status.success() || answer["ok"] != true {
        return Err(format!("orca {what}: {}", answer["error"]["message"].as_str().unwrap_or("failed")));
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
        let create =
            |sel: &str| orca(&["terminal", "create", "--worktree", sel, "--title", title, "--command", line]);
        // A plain git directory (the watch's home) is no Orca workspace
        // until Orca holds it as a repo, and Orca finds the repo's own
        // worktree a moment after it is added.
        let made = match create(&format!("path:{}", worktree.display())) {
            Err(e) if e.contains("selector_not_found") => {
                let repo = orca(&["repo", "add", "--path", &worktree.to_string_lossy()])?;
                let id = repo["repo"]["id"].as_str().ok_or("orca repo add gave no id")?;
                let sel = format!("id:{id}::{}", worktree.display());
                let mut tries = 0;
                loop {
                    match create(&sel) {
                        Err(e) if e.contains("selector_not_found") && tries < 5 => {
                            tries += 1;
                            std::thread::sleep(std::time::Duration::from_secs(1));
                        }
                        other => break other?,
                    }
                }
            }
            other => other?,
        };
        made["terminal"]["handle"].as_str().map(str::to_string).ok_or_else(|| "orca gave no terminal handle".into())
    }

    fn send(&self, terminal: &str, text: &str) -> Result<(), String> {
        // Text sent before the session is up goes to the shell, ahead of
        // the line Orca types to start it: wait for its screen to settle.
        let _ = orca(&["terminal", "wait", "--terminal", terminal, "--for", "tui-idle", "--timeout-ms", "60000"]);
        orca(&["terminal", "send", "--terminal", terminal, "--text", text, "--enter"]).map(|_| ())
    }

    fn screen(&self, terminal: &str) -> Option<Vec<String>> {
        let read = orca(&["terminal", "read", "--terminal", terminal, "--screen", "--limit", "200"]).ok()?;
        // Orca answers the rendered rows as `terminal.tail`.
        let lines = read["terminal"]["tail"].as_array()?;
        Some(lines.iter().map(|l| l.as_str().unwrap_or_default().to_string()).collect())
    }

    fn close(&self, terminal: &str) {
        let _ = orca(&["terminal", "close", "--terminal", terminal]);
    }
}
