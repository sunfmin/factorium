//! A harness: the program a session runs in. How it starts, how a tool call
//! reaches the hook, where it keeps its session and which lines start or end
//! a turn, and which screen words mean a quota or a refusal are each one
//! harness's own, in its module here and nowhere else.

use std::path::Path;

use crate::line::Line;
use crate::start::{Home, Launch, Start};

mod claude_code;
mod codex;
mod dsh;
mod grok;
mod kimi;
mod pi;
mod session;
mod store;

/// The same item of each harness's module, for `$harness`.
macro_rules! each {
    ($harness:expr, $($item:tt)+) => {
        match $harness {
            Harness::ClaudeCode => claude_code::$($item)+,
            Harness::Pi => pi::$($item)+,
            Harness::Codex => codex::$($item)+,
            Harness::Dsh => dsh::$($item)+,
            Harness::Kimi => kimi::$($item)+,
            Harness::Grok => grok::$($item)+,
        }
    };
}

/// One tool call a session is about to make, as far as a hook judges it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    /// It runs this shell line: the only kind a hook judges.
    Shell(String),
    Other,
}

/// A file or directory of the host user's that a kept directory links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Link {
    /// Where it is, under the host user's home.
    home: &'static str,
    /// Where the link is, under the kept directory.
    kept: &'static str,
}

/// What a session's screen says stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Its account is used up: the line that says so.
    Quota(String),
    /// Its harness refused the task or ran out of capacity: the line.
    Refused(String),
}

/// A harness a session can be started in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    ClaudeCode,
    Pi,
    Codex,
    Dsh,
    Kimi,
    Grok,
}

const ALL: [Harness; 6] =
    [Harness::ClaudeCode, Harness::Pi, Harness::Codex, Harness::Dsh, Harness::Kimi, Harness::Grok];

impl Harness {
    /// What a config or a record writes it as.
    pub const fn key(self) -> &'static str {
        each!(self, KEY)
    }

    /// The harness written as `key`.
    pub fn read(key: &str) -> Option<Harness> {
        ALL.into_iter().find(|harness| harness.key() == key)
    }

    /// Whether its login refreshes and is bound to one machine, so that an
    /// account of it names that machine.
    pub(crate) const fn logs_in(self) -> bool {
        matches!(self, Harness::Codex | Harness::Grok | Harness::Kimi)
    }

    /// The file under its user configuration whose being there means it is
    /// logged in.
    pub(crate) fn login_file(self) -> Option<&'static str> {
        each!(self, KEPT_LINKS).first().map(|link| link.kept)
    }

    /// Where its sessions keep their files on this host, decided once from
    /// the host user's home `user_home`, the session's `account` and the
    /// directory `kept` this host keeps for the harness: a login account's
    /// own home, where its login is; else Claude Code's under the host
    /// user's home, every other's the kept directory.
    pub fn home(self, user_home: &Path, account: Option<&str>, kept: &Path) -> Home {
        Home::new(match account {
            Some(name) if self.logs_in() => crate::account::login_home(user_home, name),
            _ if self == Harness::ClaudeCode => user_home.join(claude_code::HOME),
            _ => kept.to_path_buf(),
        })
    }

    /// Readies the session's home `start.home` (made, and the host user's
    /// login files linked into a kept directory) and answers the line a terminal runs to start a session as
    /// `start` says, and what is typed at it once it runs to hand it its
    /// brief. Each harness starts in `start.dir` and is given the worktree,
    /// the hook, Orca's status report and the model; Grok works only in its
    /// cwd, so it starts in the worktree.
    pub fn launch(self, start: &Start) -> std::io::Result<Launch> {
        let home = start.home.path();
        std::fs::create_dir_all(home)?;
        for link in each!(self, KEPT_LINKS) {
            let (from, to) = (start.user_home.join(link.home), home.join(link.kept));
            if from.exists() && !to.exists() {
                let _ = std::fs::create_dir_all(to.parent().unwrap_or(home));
                let _ = std::os::unix::fs::symlink(&from, &to);
            }
        }
        let directory = match self {
            Harness::Grok => start.worktree,
            _ => start.dir,
        };
        let mut line = Line::new().fixed("cd ").word(directory).fixed(" && export");
        for variable in self.environment() {
            line = line.fixed(" ").fixed(variable).fixed("=1");
        }
        let mut line = each!(self, environment(line, start));
        for (name, value) in &start.env {
            line = line.fixed(" ").fixed(name).fixed("=").word(value);
        }
        let line = line.fixed(" && ").launch(start.command);
        let line = each!(self, added(line, start)).done();
        Ok(match self {
            Harness::ClaudeCode => Launch { line: claude_code::brief_at_start(&line, start.brief), typed: None },
            Harness::Codex => Launch { line, typed: Some(codex::brief_line(start.brief)) },
            _ => Launch { line, typed: Some(crate::brief::brief_pointer(start.brief)) },
        })
    }

    /// The variables that switch this harness, each set to 1.
    const fn environment(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &claude_code::ENVIRONMENT,
            Harness::Kimi => &kimi::ENVIRONMENT,
            Harness::Grok => &grok::ENVIRONMENT,
            Harness::Pi | Harness::Codex | Harness::Dsh => &[],
        }
    }

    /// The shell line of the tool call a hook is handed as `payload`;
    /// `None` when it is no shell call.
    pub fn shell_call(self, payload: &str) -> Option<String> {
        let mut described = serde_json::Deserializer::from_str(payload);
        match each!(self, call(&mut described)) {
            Ok(Call::Shell(line)) => Some(line),
            _ => None,
        }
    }

    /// The hook exit code that refuses a call: it is not made, and the
    /// hook's stderr is shown to the session.
    pub const fn refused(self) -> u8 {
        each!(self, REFUSED)
    }

    /// Why `screen`, the end of a session's terminal, is not this harness's
    /// own screen up, ready to be typed at: blank, a shell's prompt (its
    /// program not started yet, or exited), or a question it asks before its
    /// screen is up. `None`: it is up.
    pub fn not_up(self, screen: &[String]) -> Option<String> {
        let Some(row) = crate::screen::last_row(screen) else { return Some("its screen is blank".into()) };
        if crate::screen::shell_prompt(row) {
            return Some(format!("its screen is a shell's: {row}"));
        }
        match self {
            Harness::Dsh if row.contains(dsh::ASKS_FIRST) => Some(format!("it asks before it starts: {row}")),
            _ => None,
        }
    }

    /// What `screen`, the end of a session's terminal, says stopped it.
    pub fn stopped(self, screen: &[String]) -> Option<Stop> {
        let quota = match self {
            Harness::Codex => codex::account_quota(screen),
            Harness::Pi => pi::account_quota(screen),
            Harness::Kimi => kimi::account_quota(screen),
            _ => None,
        };
        if let Some(said) = quota {
            return Some(Stop::Quota(said));
        }
        match self {
            Harness::Codex => codex::task_refusal(screen).or_else(|| codex::capacity_stop(screen)).map(Stop::Refused),
            _ => None,
        }
    }
}
