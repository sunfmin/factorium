//! What one session of a harness is started with (`Harness::launch`): the
//! command that runs it, the directory it starts in, the worktree it works
//! on, the hook run before each of its tool calls, its model, the directory
//! kept for it and its brief. Whoever starts the session says each; nothing
//! here knows why.

use std::path::Path;

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
    /// The shell line run before each tool call; it refuses a call by
    /// exiting `Harness::refused`.
    pub hook: &'a str,
    /// The directory this host keeps for the session's harness: its user
    /// configuration, apart from the host user's.
    pub kept: &'a str,
    /// The host user's home, whose login files the kept directory links to.
    pub home: &'a Path,
    /// The variables put in the session's environment, in this order, after
    /// the harness's own. Each value goes in as one word.
    pub env: Vec<(&'static str, String)>,
    /// The file of instructions the session is handed.
    pub brief: &'a str,
}

/// How a session is started: the line its terminal runs, and the line typed
/// at it once it runs, where its brief is not on the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub line: String,
    pub typed: Option<String>,
}
