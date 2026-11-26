//! Orca: the worktrees agents work in and the terminals they run in. Every
//! call is `orca … --json`, run through [`Programs`]; what Orca's answers
//! mean is read here and nowhere else.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::shell::Programs;

/// A terminal as Orca reports it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// It runs; its screen as rendered.
    Running(Vec<String>),
    /// Its program ended, or Orca no longer knows it.
    Exited,
    /// Orca gave no answer to tell; why. Nothing is known of the terminal.
    Unreadable(String),
}

/// Whether a line typed at an agent reached it, as Orca watched it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// It started a turn: the agent reads it now.
    Submitted,
    /// No turn started while Orca watched: the agent holds it, if at all,
    /// until its running turn ends.
    Queued,
}

/// A worktree as Orca lists it: where it is, the name it shows, and the
/// comment it keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    pub name: String,
    pub comment: String,
}

/// How long, in seconds, a delivery watches for its line to start a turn.
const SUBMIT_WAIT: u32 = 10;

/// How long, in seconds, a key pressed at a terminal is given to bring its
/// program back to idle.
const PRESS_WAIT: u32 = 5;

/// Orca, reached through the programs the host runs.
pub struct Orca<'a>(pub &'a dyn Programs);

/// Why an Orca call failed: its error code (Orca's message is its code), or
/// what it printed when it gave no answer.
struct Failed {
    code: Option<String>,
    why: String,
}

impl From<Failed> for String {
    fn from(f: Failed) -> String {
        f.why
    }
}

impl Orca<'_> {
    /// Runs `orca <args> --json`. An error names the subcommand alone, never
    /// the arguments: a terminal's command line is one, and may be long.
    fn call(&self, args: &[&str]) -> Result<Value, Failed> {
        let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        let mut all = args.to_vec();
        all.push("--json");
        let said = self.0.run("orca", &all);
        let Ok(answer) = serde_json::from_str::<Value>(&said.out) else {
            return Err(Failed { code: None, why: format!("orca {what}: {}", said.err.trim()) });
        };
        if !said.ok || answer["ok"] != true {
            let code = answer["error"]["code"].as_str().map(str::to_string);
            let message = answer["error"]["message"].as_str().unwrap_or("failed");
            return Err(Failed { code, why: format!("orca {what}: {message}") });
        }
        Ok(answer["result"].clone())
    }

    /// Makes a worktree of the repo at `repo`, named `name` and commented
    /// `comment` (what it is for, as Orca shows it), on a new branch `branch`
    /// at `sha`: its path.
    pub fn worktree(&self, repo: &Path, name: &str, branch: &str, sha: &str, comment: &str) -> Result<PathBuf, String> {
        let repo_sel = format!("path:{}", repo.display());
        if self.call(&["repo", "show", "--repo", &repo_sel]).is_err() {
            self.call(&["repo", "add", "--path", &repo.to_string_lossy()])?;
        }
        let made = self.call(&[
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
            "--comment",
            comment,
        ])?;
        let path = made["worktree"]["path"].as_str().map(PathBuf::from).filter(|p| p.is_dir());
        let path = path.ok_or_else(|| format!("orca worktree create returned no usable path: {made}"))?;
        // Orca opens a shell in a new workspace; the session's terminal,
        // opened next, is to be its one tab.
        let sel = format!("path:{}", path.display());
        let _ = self.call(&["terminal", "close", "--worktree", &sel, "--all"]);
        Ok(path)
    }

    /// Every worktree Orca holds on this host, each repo's own among them.
    pub fn worktrees(&self) -> Result<Vec<Listed>, String> {
        let listed = self.call(&["worktree", "list", "--limit", "10000"])?;
        let rows =
            listed["worktrees"].as_array().ok_or_else(|| format!("orca worktree list gave no rows: {listed}"))?;
        if listed["truncated"] == true {
            return Err(format!("orca worktree list left rows out: {} of {}", rows.len(), listed["totalCount"]));
        }
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let row = |w: &Value| Listed {
            path: PathBuf::from(text(&w["path"])),
            name: text(&w["displayName"]),
            comment: text(&w["comment"]),
        };
        Ok(rows.iter().map(row).collect())
    }

    /// Sets the comment Orca keeps of the worktree at `worktree`.
    pub fn comment(&self, worktree: &Path, comment: &str) -> Result<(), String> {
        let sel = format!("path:{}", worktree.display());
        self.call(&["worktree", "set", "--worktree", &sel, "--comment", comment]).map(|_| ()).map_err(String::from)
    }

    /// Removes a worktree and the terminals in it.
    pub fn remove(&self, worktree: &Path) -> Result<(), String> {
        let sel = format!("path:{}", worktree.display());
        match self.call(&["worktree", "rm", "--worktree", &sel, "--force"]) {
            Err(_) if !worktree.exists() => Ok(()),
            other => other.map(|_| ()).map_err(String::from),
        }
    }

    /// Starts a terminal in `worktree` running `line`; answers its handle.
    pub fn open(&self, worktree: &Path, title: &str, line: &str) -> Result<String, String> {
        let create =
            |sel: &str| self.call(&["terminal", "create", "--worktree", sel, "--title", title, "--command", line]);
        let unknown = |f: &Failed| f.code.as_deref() == Some("selector_not_found");
        // A plain git directory (the watch's home) is no Orca workspace
        // until Orca holds it as a repo, and Orca finds the repo's own
        // worktree a moment after it is added.
        let made = match create(&format!("path:{}", worktree.display())) {
            Err(f) if unknown(&f) => {
                let repo = self.call(&["repo", "add", "--path", &worktree.to_string_lossy()])?;
                let id = repo["repo"]["id"].as_str().ok_or("orca repo add gave no id")?;
                let sel = format!("id:{id}::{}", worktree.display());
                let mut tries = 0;
                loop {
                    match create(&sel) {
                        Err(f) if unknown(&f) && tries < 5 => {
                            tries += 1;
                            self.0.run("sleep", &["1"]);
                        }
                        other => break other?,
                    }
                }
            }
            other => other?,
        };
        made["terminal"]["handle"].as_str().map(str::to_string).ok_or_else(|| "orca gave no terminal handle".into())
    }

    /// Waits, up to a minute, for the program a terminal was opened with to
    /// be up and its screen to settle. Text sent before then goes to the
    /// shell, ahead of the line Orca types to start the program.
    pub fn ready(&self, terminal: &str) -> Result<(), String> {
        self.settle(terminal, 60_000).map_err(String::from)
    }

    /// Waits up to `ms` for a terminal's program to sit idle at its prompt.
    fn settle(&self, terminal: &str, ms: u32) -> Result<(), Failed> {
        let ms = ms.to_string();
        let wait = ["terminal", "wait", "--terminal", terminal, "--for", "tui-idle", "--timeout-ms", &ms];
        self.call(&wait).map(|_| ())
    }

    /// Types `text` and Enter at a terminal.
    pub fn send(&self, terminal: &str, text: &str) -> Result<(), String> {
        self.call(&["terminal", "send", "--terminal", terminal, "--text", text, "--enter"])
            .map(|_| ())
            .map_err(String::from)
    }

    /// Types `key` without Enter at a terminal, and waits, up to
    /// `PRESS_WAIT` seconds, for its program to sit idle again. Orca does
    /// not always see it idle (Claude Code 2.1.330 holding a shell in the
    /// background is working to Orca 1.4.267, in a turn or not): a wait that
    /// times out is no failure, and the caller reads what came of the key.
    pub fn press(&self, terminal: &str, key: &str) -> Result<(), String> {
        self.call(&["terminal", "send", "--terminal", terminal, "--text", key])?;
        self.pressed(terminal).map_err(String::from)
    }

    /// Waits for a terminal to sit idle after a key, as [`Orca::press`] does.
    fn pressed(&self, terminal: &str) -> Result<(), Failed> {
        match self.settle(terminal, PRESS_WAIT * 1000) {
            Err(f) if f.code.as_deref() == Some("timeout") => Ok(()),
            other => other,
        }
    }

    /// Types `text` and Enter at an agent's terminal and watches, up to
    /// `SUBMIT_WAIT` seconds, whether it started a turn. `interrupt`: first
    /// types that key (no Enter) to stop the agent's running turn, and waits
    /// for its prompt to be idle, as [`Orca::press`] does. Orca never resends
    /// a prompt it watched. A refusal once the key went says so: the
    /// agent's turn was stopped, and nothing took its place.
    pub fn deliver(&self, terminal: &str, text: &str, interrupt: Option<&str>) -> Result<Delivery, String> {
        if let Some(key) = interrupt {
            self.call(&["terminal", "send", "--terminal", terminal, "--text", key])?;
        }
        let stopped = |f: Failed| match interrupt {
            Some(_) => format!("{}; its running turn was stopped first", f.why),
            None => f.why,
        };
        if interrupt.is_some() {
            self.pressed(terminal).map_err(stopped)?;
        }
        let wait = SUBMIT_WAIT.to_string();
        let send = ["terminal", "send", "--terminal", terminal, "--text", text, "--enter", "--wait-submit", &wait];
        let sent = self.call(&send).map_err(stopped)?;
        let stages = sent["send"]["prompt"]["stages"].as_array().cloned().unwrap_or_default();
        Ok(if stages.iter().any(|s| s == "turn_started") { Delivery::Submitted } else { Delivery::Queued })
    }

    /// The terminal now.
    pub fn screen(&self, terminal: &str) -> Term {
        let read = match self.call(&["terminal", "read", "--terminal", terminal, "--screen", "--limit", "200"]) {
            Ok(read) => read,
            // Orca no longer knows the handle (it restarted, say).
            Err(f) if f.code.as_deref() == Some("terminal_handle_stale") => return Term::Exited,
            Err(f) => return Term::Unreadable(f.why),
        };
        // Orca still answers a closed terminal's last screen.
        if read["terminal"]["status"] == "exited" {
            return Term::Exited;
        }
        // Orca answers the rendered rows as `terminal.tail`.
        match read["terminal"]["tail"].as_array() {
            Some(rows) => Term::Running(rows.iter().map(|l| l.as_str().unwrap_or_default().to_string()).collect()),
            None => Term::Unreadable(format!("orca terminal read gave no rows: {read}")),
        }
    }

    /// The live terminals Orca manages, as (handle, title).
    pub fn terminals(&self) -> Result<Vec<(String, String)>, String> {
        let listed = self.call(&["terminal", "list"])?;
        let rows =
            listed["terminals"].as_array().ok_or_else(|| format!("orca terminal list gave no rows: {listed}"))?;
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        Ok(rows.iter().map(|t| (text(&t["handle"]), text(&t["title"]))).collect())
    }

    /// Ends a terminal; answers why Orca refused to.
    pub fn close(&self, terminal: &str) -> Result<(), String> {
        self.call(&["terminal", "close", "--terminal", terminal]).map(|_| ()).map_err(String::from)
    }
}
