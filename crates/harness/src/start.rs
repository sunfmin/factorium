//! What one session of a harness is started with (`Harness::launch`): the
//! command that runs it, the directory it starts in, the worktree it works
//! on, the hook run before each of its tool calls, its model, its home and
//! its brief; and what it takes to start it (`Launch`): the line, how its
//! brief goes in, and what its home must hold. Whoever starts the session
//! says each; nothing here knows why.

use std::path::{Path, PathBuf};

use crate::Harness;
use crate::line::Line;

/// A directory's skills: the directory under it every harness reads one
/// entry per skill from.
pub const SKILLS: &str = ".claude/skills";
/// A directory's rules: the file every harness loads them from, so that they
/// are written once.
pub const RULES: &str = "CLAUDE.md";

/// Where one session starts, and what is added to it.
#[derive(Debug, Clone)]
pub struct Start<'a> {
    /// The launch command: a shell string, put in the line as it is.
    pub command: &'a str,
    /// The directory the session is started in, holding its own rules
    /// ([`RULES`]) and skills ([`SKILLS`]).
    pub dir: &'a str,
    /// The checkout the session works on, with the repo's own rules.
    pub worktree: &'a str,
    /// The model the session runs in, where one is named.
    pub model: Option<&'a str>,
    /// The factorium program a session asks, as its guard hook, before
    /// each tool call; the hook refuses a call by exiting `Harness::refused`.
    pub guard: &'a str,
    /// Where the session keeps its files (`Harness::home`).
    pub home: &'a Home,
    /// The host user's home, whose login files a kept directory links to.
    pub user_home: &'a Path,
    /// The login files of the account the session starts on (`stablehand
    /// credential`), each as (where the home holds it, the file it links
    /// to): linked in place of the host user's, and of any link of its own
    /// that holds one or lies inside one.
    pub logins: &'a [(String, PathBuf)],
    /// The variables put in the session's environment, in this order, after
    /// the harness's own. Each value goes in as one word.
    pub env: Vec<(&'static str, String)>,
    /// The file of instructions the session is handed.
    pub brief: &'a str,
    /// The file of secrets the line reads first and removes, kept off the
    /// line itself: a secret there shows in `ps`, in the terminal's
    /// scrollback and in any error naming the line (`secrets_file`).
    pub secrets: Option<&'a str>,
}

impl Start<'_> {
    /// The guard hook of a session of `harness`.
    pub(crate) fn hook(&self, harness: Harness) -> String {
        crate::line::guard(self.guard, harness.key())
    }

    /// `line` with the variables that name where this session's rules are:
    /// its home, shared, cannot.
    pub(crate) fn rules_env(&self, line: Line) -> Line {
        line.set(WORKTREE_ENV, self.worktree).set(START_DIR_ENV, self.dir)
    }

    /// The guard hook of a session of `harness`, whose hooks fail open: any
    /// failure to ask the guard refuses the call too.
    pub(crate) fn hook_asked(&self, harness: Harness) -> String {
        crate::line::guard_asked(self.guard, harness.key(), harness.refused())
    }
}

/// How a session is started: what its home must hold first, the line its
/// terminal runs, and the line typed at it once it runs, where its brief is
/// not on the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// Put in place, in this order, before the line runs: the session does
    /// not start as it must without each.
    pub kept: Vec<Kept>,
    pub line: String,
    pub typed: Option<String>,
}

/// One thing a session's home holds (`Launch::kept`), at an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// A directory: the home itself, first.
    Dir { path: PathBuf },
    /// A file of the tool's, written afresh at each start, for its user alone.
    File { path: PathBuf, text: String },
    /// A link to a file of the host user's, made where nothing is.
    Link { path: PathBuf, to: PathBuf },
}

/// The variables a session whose home holds the rules it is told is
/// started with: the worktree, and the directory it would start in.
const WORKTREE_ENV: &str = "FACTORIUM_WORKTREE";
const START_DIR_ENV: &str = "FACTORIUM_ADDED_DIR";

/// The rules a harness that reads neither the start directory's rules file
/// nor the repo's is told, in its home. A home is shared by every session of
/// its harness on its account, so this names where each session's rules are,
/// by its variables, and holds nothing of one session's.
pub(crate) const TOLD: &str = "factorium started you. Before anything else, read your rules and work under \
     them as under rules you were given: first the file CLAUDE.md in the directory the environment variable \
     FACTORIUM_ADDED_DIR names (your role's rules), then the repo's own rules, the file AGENTS.md in the \
     worktree the environment variable FACTORIUM_WORKTREE names, or CLAUDE.md there if it has no AGENTS.md. \
     Your skills are the directories under .claude/skills in the FACTORIUM_ADDED_DIR directory: read a \
     skill's SKILL.md before you apply it.\n";

/// Where a harness's sessions keep their configuration and session files on
/// this host, as `Harness::home` decided it: the one place a session is
/// started pointed at, and the one its turn is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    dir: PathBuf,
    text: String,
}

impl Home {
    pub(crate) fn new(dir: PathBuf) -> Home {
        let text = dir.to_string_lossy().into_owned();
        Home { dir, text }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }

    /// The directory as one word of a start line.
    pub(crate) fn dir(&self) -> &str {
        &self.text
    }
}
