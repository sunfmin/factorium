//! A harness: the program a session runs in. How it starts, what is added to
//! its start line, how a tool call reaches the hook and is answered, where it
//! keeps its session and which lines start or end a turn are each one
//! harness's own, in its module here and nowhere else.

use serde::{Deserialize, Deserializer};

use crate::line::Line;
use crate::start::Start;
use crate::turn::Turn;

mod claude_code;
mod codex;
mod dsh;
mod grok;
mod kimi;
mod pi;
mod session;

pub use session::{epoch_number, parse_iso};

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
pub enum Call {
    /// It runs this shell line: the only kind a hook judges.
    Shell(String),
    Other,
}

impl Call {
    pub fn shell(line: &str) -> Call {
        Call::Shell(line.to_string())
    }
}

/// A program besides the harness itself that a session cannot start without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needed {
    /// DeepSeek Harness's own program, which dsh-tui runs.
    Dsh,
}

/// The variable a session's environment names its harness in
/// (`Harness::key`): how a hook knows whose call it is asked about.
pub const HARNESS_ENV: &str = "FACTORIUM_HARNESS";

/// The word of a launch command whose resolving decides whether it can run.
pub fn first_word(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

/// A file or directory of the host user's that a kept directory links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    /// Where it is, under the host user's home.
    pub home: &'static str,
    /// Where the link is, under the kept directory.
    pub kept: &'static str,
}

/// Where a harness reads its user's hooks. The file holds a JSON object whose
/// `table` key holds, by event, groups of
/// `{"matcher": ..., "hooks": [{"type": "command", "command": ..., "timeout": ...}]}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserHooks {
    /// The settings file, under the host user's home.
    pub settings: &'static str,
    pub table: &'static str,
    /// The event before each tool call.
    pub event: &'static str,
    /// The matcher of calls that run a shell command.
    pub shell: &'static str,
    /// The matcher of calls that edit a file.
    pub edits: &'static str,
}

/// How a harness that keeps each session apart (Codex) is started: names
/// only; what they mean is that harness's module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    /// The prefix of a fresh directory under the worktree's temporary
    /// directory, which holds the session's kept directory.
    pub fresh: &'static str,
    /// The note in the worktree naming the session's kept directory.
    pub note: &'static str,
    /// The user's config, under their home, an automatic start reads.
    pub config: &'static str,
    /// The note in the worktree recording what the start resolved.
    pub record: &'static str,
}

/// A model and an effort a session saved into its own config after it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub model: Option<String>,
    pub model_source: Option<String>,
    pub effort: Option<String>,
    pub effort_source: Option<String>,
}

/// What a host keeps for one harness before it starts a session of it, in a
/// directory of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    /// The host user's files it links to; one the user lacks is left out.
    pub links: &'static [Link],
    /// The host user's own skills and rules, linked to only in the directory
    /// kept for a session given them (`Start::host_user`).
    pub host_user: &'static [Link],
    /// The files the tool carries: where under the directory, and what.
    pub files: &'static [(&'static str, &'static str)],
}

/// What a directory holds where its limits for one harness go
/// (`Harness::limits`).
pub enum Held<D> {
    Nothing,
    /// A file, with what reads it.
    File(D),
    /// A directory of this many entries.
    Entries(usize),
}

/// Where one harness keeps the sessions of one worktree. The reader opens
/// the files; a missing directory is no turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlace {
    /// The harness directory under a user's home. A kept directory, and a
    /// home [`Harness::noted_home`] names, are already this directory.
    pub under_home: &'static str,
    pub layout: SessionLayout,
    /// Another layout whose sessions count only when they record this
    /// worktree's turn: Claude Code started elsewhere files its words under
    /// its start directory's project.
    pub also: Option<SessionLayout>,
}

/// How the session files of one worktree are laid out under the harness
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLayout {
    /// Each file of `extension` directly in `dir` is one session.
    Files { dir: String, extension: &'static str },
    /// Each file of `extension` anywhere under `dir` is one session.
    Tree { dir: String, extension: &'static str },
    /// Each `dir/*/<name>` is one session; `compressed` means zstd. `usage`
    /// names the file beside it holding usage, where it is kept apart.
    Named { dir: String, name: &'static str, compressed: bool, usage: Option<&'static str> },
    /// `file` names the session files ([`Harness::indexed_sessions`]).
    Index { file: String },
}

/// One session file an index names ([`Harness::indexed_sessions`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    /// Absolute, or relative to the harness directory.
    pub file: String,
    /// The index line's directory is the worktree read for.
    pub here: bool,
}

/// One file a stand-in writes under the harness directory; the body is the
/// text [`Harness::recorded_turn`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    /// Path under the harness directory.
    pub path: String,
    /// The session text, before `compressed` storage.
    pub body: String,
    /// The body is stored as zstd.
    pub compressed: bool,
    /// Before writing, delete every file of this name under the directory.
    pub replaces: Option<String>,
}

/// Where a probe's answer says which model answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsweredAs {
    Said(String),
    /// In the session file under `under` (in the kept directory) whose name
    /// ends `ends` (`Harness::answered_in_session`); the prober deletes it.
    InSession {
        under: &'static str,
        ends: String,
    },
    Unsaid,
}

/// One keystroke typed at a harness: `text` (`None` is the key alone), and
/// whether Enter follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keystroke {
    pub text: Option<&'static str>,
    pub enter: bool,
}

/// How a session nobody types at is handed its brief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BriefAtStart {
    /// The start line with the brief as its first prompt.
    Started(String),
    /// The start line runs as it is, and this line ([`Harness::brief_line`])
    /// is typed once the session is idle.
    Typed(String),
}

/// Where a harness keeps a session's goal: a SQLite file under the kept
/// directory, and the query whose answer is its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalStore {
    pub file: &'static str,
    pub query: &'static str,
}

/// A session's goal, where its harness keeps one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    /// Never set, or cleared.
    Unset,
    /// The harness continues it after each turn its model ends.
    Active,
    Paused,
    /// The same blocker on three goal turns in a row.
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

impl Goal {
    /// A goal that stopped short of its brief: it is continued no more.
    pub fn stopped(self) -> bool {
        matches!(self, Goal::Blocked | Goal::BudgetLimited)
    }

    /// The word the store says it by.
    pub fn key(self) -> &'static str {
        match self {
            Goal::Unset => "unset",
            Goal::Active => "active",
            Goal::Paused => "paused",
            Goal::Blocked => "blocked",
            Goal::UsageLimited => "usage_limited",
            Goal::BudgetLimited => "budget_limited",
            Goal::Complete => "complete",
        }
    }
}

/// What a session record says of its goal: its thread, the objective it was
/// set with, and when each goal turn started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalSession {
    pub thread: Option<String>,
    pub objective: Option<String>,
    pub turns: Vec<u64>,
}

/// One session's goal row: status and tokens used against the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalRow {
    pub status: Goal,
    pub tokens_used: u64,
    pub token_budget: Option<u64>,
}

/// How a harness is asked whether it is logged in, and logged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Login {
    /// The shell line whose answer `Harness::logged_in` reads.
    pub question: &'static str,
    /// What it is said to be when it is not.
    pub logged_out: &'static str,
    /// The line that logs it in.
    pub line: &'static str,
}

/// How a harness's engine is asked for its account's usage windows outside
/// any session: run `line`, read what it printed with
/// `Harness::windows_answer`. Nothing in this crate runs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsQuestion {
    /// Run in the environment a session on the account starts with
    /// (`account::Account::session`). It puts no credential on a command
    /// line, and ends once it has its answer.
    pub line: &'static str,
    /// Whether asking spends a (smallest possible) model turn.
    pub spends_turn: bool,
}

/// The directory a harness reads its user configuration from, and the
/// variable that points it elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigDir {
    pub variable: &'static str,
    /// Where it is under the user's home while the variable is unset.
    pub home: &'static str,
}

/// Login question and line for a harness pointed at one configuration
/// directory (`Harness::login_in`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginIn {
    pub question: String,
    pub logged_out: &'static str,
    pub line: String,
}

/// One file a fresh configuration directory is given so the harness asks no
/// first-run question with nobody at its terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fresh {
    /// Where the file is, under the directory.
    pub file: &'static str,
    pub given: Given,
}

/// What one fresh file is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Given {
    /// Members (a JSON object) merged over the file's object, every other
    /// member — a login among them — kept.
    Members(&'static str),
    /// The whole file, where there is none.
    Whole(&'static str),
}

impl Fresh {
    /// What the file is to hold, given what it `held` (`None`: no file). An
    /// error is a file that holds no JSON object where members are merged.
    pub fn written(self, held: Option<&str>) -> Result<String, String> {
        match self.given {
            Given::Whole(text) => Ok(held.unwrap_or(text).to_string()),
            Given::Members(members) => {
                let members: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(members).map_err(|e| e.to_string())?;
                let mut object = match held.map(str::trim) {
                    None | Some("") => serde_json::Map::new(),
                    Some(text) => match serde_json::from_str(text) {
                        Ok(serde_json::Value::Object(object)) => object,
                        Ok(_) => return Err(format!("{} holds no JSON object", self.file)),
                        Err(e) => return Err(format!("{} holds no JSON object: {e}", self.file)),
                    },
                };
                object.extend(members);
                serde_json::to_string_pretty(&serde_json::Value::Object(object))
                    .map(|text| text + "\n")
                    .map_err(|e| e.to_string())
            }
        }
    }
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

/// A harness is written as its key.
impl serde::Serialize for Harness {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

/// A harness is read as its key; any other word is refused.
impl<'de> Deserialize<'de> for Harness {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let key = String::deserialize(deserializer)?;
        Harness::read(&key).ok_or_else(|| serde::de::Error::custom(Harness::unknown(&key)))
    }
}

impl Harness {
    /// The harness's fixed capacity-stop words on its screen, wrapping
    /// allowed. Only a harness whose screen was observed has a rule.
    pub fn capacity_stop(self, output: &[String]) -> Option<String> {
        match self {
            Harness::Codex => codex::capacity_stop(output),
            _ => None,
        }
    }

    /// The harness's fixed task-refusal words on its screen, wrapping allowed.
    pub fn task_refusal(self, output: &[String]) -> Option<String> {
        match self {
            Harness::Codex => codex::task_refusal(output),
            _ => None,
        }
    }

    /// The key that hands `line` to the model now, when `screen` shows it
    /// waiting in this harness's input queue; `None` otherwise.
    pub fn send_queued(self, screen: &[String], line: &str) -> Option<Keystroke> {
        match self {
            Harness::Grok => grok::send_queued(screen, line),
            _ => None,
        }
    }

    /// The line showing this harness's own account-quota words on its
    /// screen or in a runtime message, wrapping allowed.
    pub fn account_quota(self, output: &[String]) -> Option<String> {
        match self {
            Harness::Codex => codex::account_quota(output),
            Harness::Pi => pi::account_quota(output),
            Harness::Kimi => kimi::account_quota(output),
            _ => None,
        }
    }

    /// The account's usage windows one record says, as the engine reports
    /// them: Claude Code's status line input and `rate_limit_event`, a Codex
    /// `token_count` line, Kimi Code's `/usage` answer. None otherwise.
    pub fn windows(self, record: &serde_json::Value) -> Vec<crate::window::Window> {
        match self {
            Harness::ClaudeCode => claude_code::windows(record),
            Harness::Codex => codex::windows(record),
            Harness::Kimi => kimi::windows(record),
            Harness::Pi | Harness::Dsh | Harness::Grok => Vec::new(),
        }
    }

    /// Whether its engine reports its account's usage windows at all.
    pub const fn reports_windows(self) -> bool {
        match self {
            Harness::ClaudeCode | Harness::Codex | Harness::Kimi => true,
            Harness::Pi | Harness::Dsh | Harness::Grok => false,
        }
    }

    /// How its engine is asked for the account's windows without a session:
    /// Codex's app server and Kimi Code's `/usages` spend no turn; Claude
    /// Code spends a one-word turn on its smallest model to get its
    /// `rate_limit_event`. `None` where it reports no window.
    pub const fn windows_question(self) -> Option<WindowsQuestion> {
        match self {
            Harness::ClaudeCode => Some(claude_code::WINDOWS_QUESTION),
            Harness::Codex => Some(codex::WINDOWS_QUESTION),
            Harness::Kimi => Some(kimi::WINDOWS_QUESTION),
            Harness::Pi | Harness::Dsh | Harness::Grok => None,
        }
    }

    /// The windows in what its [`WindowsQuestion::line`] printed; an error,
    /// a refused login or nothing gives none.
    pub fn windows_answer(self, printed: &str) -> Vec<crate::window::Window> {
        match self {
            Harness::ClaudeCode => printed
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .map(|record| claude_code::windows(&record))
                .find(|windows| !windows.is_empty())
                .unwrap_or_default(),
            Harness::Codex => codex::windows_answer(printed),
            Harness::Kimi => serde_json::from_str::<serde_json::Value>(printed)
                .map(|answer| kimi::windows(&answer))
                .unwrap_or_default(),
            Harness::Pi | Harness::Dsh | Harness::Grok => Vec::new(),
        }
    }

    /// The limit one record's structured fields say was met, with when it
    /// lifts where said: beside the screen words of `account_quota`.
    pub fn limit_event(self, record: &serde_json::Value) -> Option<crate::window::Limit> {
        match self {
            Harness::ClaudeCode => claude_code::limit_event(record),
            Harness::Codex => codex::limit_event(record),
            _ => None,
        }
    }

    /// Where a host installs a status line command, which this harness hands
    /// what [`Harness::windows`] reads.
    pub const fn status_line(self) -> Option<crate::window::StatusLine> {
        match self {
            Harness::ClaudeCode => Some(claude_code::STATUS_LINE),
            _ => None,
        }
    }

    /// The harness where nothing names one.
    pub const DEFAULT: Harness = Harness::ClaudeCode;

    /// The harness of a record written before records named one.
    pub const UNNAMED: Harness = Harness::ClaudeCode;

    pub const ALL: [Harness; 6] =
        [Harness::ClaudeCode, Harness::Pi, Harness::Codex, Harness::Dsh, Harness::Kimi, Harness::Grok];

    /// What a config or a record writes it as.
    pub const fn key(self) -> &'static str {
        each!(self, KEY)
    }

    /// The harness written as `key`.
    pub fn read(key: &str) -> Option<Harness> {
        Harness::ALL.into_iter().find(|harness| harness.key() == key)
    }

    /// Why `key` is no harness, with the ones there are.
    pub fn unknown(key: &str) -> String {
        let keys: Vec<&str> = Harness::ALL.iter().map(|harness| harness.key()).collect();
        format!("`{key}` is no harness; the harnesses are: {}", keys.join(", "))
    }

    /// What a human calls it.
    pub const fn name(self) -> &'static str {
        each!(self, NAME)
    }

    /// The word that runs it.
    pub const fn program(self) -> &'static str {
        each!(self, PROGRAM)
    }

    /// Whether the launch command `launch` runs it by its own name, as its
    /// first word; what a wrapper runs is the wrapper's business.
    pub fn runs_itself(self, launch: &str) -> bool {
        first_word(launch) == Some(self.program())
    }

    /// The line that installs it.
    pub const fn install(self) -> &'static str {
        each!(self, INSTALL)
    }

    /// How it is asked whether it is logged in, and logged in; `None`: it
    /// logs in to nothing a start asks about.
    pub const fn login(self) -> Option<Login> {
        each!(self, LOGIN)
    }

    /// The line logging it in with a device code, where its login refreshes
    /// and is bound to one machine (`account::Account::login`).
    pub const fn device_login(self) -> Option<&'static str> {
        match self {
            Harness::Codex => codex::DEVICE_LOGIN,
            Harness::Grok => grok::DEVICE_LOGIN,
            Harness::Kimi => kimi::DEVICE_LOGIN,
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh => None,
        }
    }

    /// Its user configuration directory and the variable that points it at
    /// another. A start points each harness at the kept directory by it:
    /// Claude Code only where `Start::kept_config` says so.
    pub const fn config_dir(self) -> ConfigDir {
        let variable = match self {
            Harness::ClaudeCode => claude_code::CONFIG_DIR,
            Harness::Pi => pi::USER_DIR,
            Harness::Codex => codex::USER_DIR,
            Harness::Dsh => dsh::USER_DIR,
            Harness::Kimi => kimi::USER_DIR,
            Harness::Grok => grok::USER_DIR,
        };
        ConfigDir { variable, home: each!(self, HOME) }
    }

    /// `login`, of the configuration in `dir`: each line exports
    /// `config_dir`'s variable as `dir` first.
    pub fn login_in(self, dir: &str) -> Option<LoginIn> {
        let login = self.login()?;
        let (question, logged_out) = match self {
            Harness::Codex => (codex::LOGIN_IN_CONFIG, codex::LOGGED_OUT_IN_CONFIG),
            _ => (login.question, login.logged_out),
        };
        let variable = self.config_dir().variable;
        let pointed = |line: &str| {
            Line::new().fixed("export ").fixed(variable).fixed("=").word(dir).fixed(" && ").launch(line).done()
        };
        Some(LoginIn { question: pointed(question), logged_out, line: pointed(login.line) })
    }

    /// What a fresh configuration directory is given so it asks no
    /// first-run question: Claude Code's onboarding answered, Kimi Code's
    /// migration offer declined.
    pub const fn fresh(self) -> &'static [Fresh] {
        match self {
            Harness::ClaudeCode => &claude_code::FRESH,
            Harness::Kimi => &kimi::FRESH,
            _ => &[],
        }
    }

    /// Whether the answer to `Login::question` says it is logged in.
    pub fn logged_in(self, succeeded: bool, printed: &str) -> bool {
        each!(self, logged_in(succeeded, printed))
    }

    /// The program besides itself a session cannot start without.
    pub const fn needs(self) -> Option<Needed> {
        match self {
            Harness::Dsh => Some(Needed::Dsh),
            _ => None,
        }
    }

    /// Whether a session started without a `Start` (a probe, an asking
    /// launch) gets a home of its own, so its generated hook files never go
    /// into a home another session shares.
    pub const fn own_home_outside_start(self) -> bool {
        match self {
            Harness::Kimi => kimi::OWN_HOME_OUTSIDE_START,
            Harness::Grok => grok::OWN_HOME_OUTSIDE_START,
            _ => false,
        }
    }

    /// The hosts its session's login shell reaches without a proxy unless
    /// the host says otherwise.
    pub const fn no_proxy(self) -> Option<&'static str> {
        match self {
            Harness::Dsh => Some(dsh::NO_PROXY),
            _ => None,
        }
    }

    /// Where under the kept directory its installed package is linked.
    pub const fn kept_package(self) -> Option<&'static str> {
        match self {
            Harness::Dsh => Some(dsh::KEPT_PACKAGE),
            _ => None,
        }
    }

    /// Where it reads its user's global instructions, under the user's home.
    pub const fn instructions(self) -> &'static str {
        each!(self, INSTRUCTIONS)
    }

    /// The agent `npx skills` installs skills for (`-a`); `None`: it reads
    /// the shared copies.
    pub const fn skills_agent(self) -> Option<&'static str> {
        each!(self, SKILLS_AGENT)
    }

    /// Where it reads its user's hooks, and how one runs before a tool call.
    pub const fn user_hooks(self) -> Option<UserHooks> {
        match self {
            Harness::ClaudeCode => Some(claude_code::USER_HOOKS),
            Harness::Codex => Some(codex::USER_HOOKS),
            _ => None,
        }
    }

    /// The launch command where nobody said another.
    pub const fn stock(self) -> &'static str {
        each!(self, STOCK)
    }

    /// The variable that points it at a custom provider. A session's shell
    /// inherits none of the caller's, so it must be passed on. pi and Codex
    /// read providers from their user directory; Kimi from its kept home.
    pub const fn provider_variable(self) -> Option<&'static str> {
        match self {
            Harness::ClaudeCode => Some(claude_code::PROVIDER),
            _ => None,
        }
    }

    /// Every flag a start adds: what a launch command of it must take.
    pub const fn flags(self) -> &'static [&'static str] {
        each!(self, FLAGS.as_slice())
    }

    /// What a host keeps for it before it starts a session of it
    /// (`Start::kept`).
    pub const fn kept(self) -> Kept {
        each!(self, KEPT)
    }

    /// The host user's native rule paths project discovery could also find:
    /// kept out of a session not given the host user's rules.
    pub fn host_rule_excludes(self, home: &str) -> Vec<String> {
        match self {
            Harness::ClaudeCode => claude_code::host_rule_excludes(home),
            _ => vec![],
        }
    }

    /// The host user's config, under their home, its kept home's config is
    /// generated from.
    pub const fn user_config(self) -> Option<&'static str> {
        match self {
            Harness::Kimi => Some(kimi::USER_CONFIG),
            _ => None,
        }
    }

    /// Where `kept_system` is written, under the kept home.
    pub const fn kept_system_file(self) -> &'static str {
        match self {
            Harness::Grok => grok::SYSTEM_FILE,
            _ => "SYSTEM.md",
        }
    }

    /// The generated configuration its kept home needs, if any, from the
    /// host user's config `user`, the start directory's limits file `limits`,
    /// the hook `guard`, whether the host user's skills and rules are given,
    /// and the start directory's skills directory and its entries (Grok
    /// names it and ignores a host skill of the same name).
    pub fn kept_configuration(
        self,
        user: Option<&str>,
        limits: Option<&str>,
        guard: Option<&str>,
        host_user: bool,
        start_skills: Option<&str>,
        skill_names: &[String],
    ) -> Result<Option<String>, String> {
        match self {
            Harness::Kimi => kimi::configuration(user, limits, guard).map(Some),
            Harness::Grok => grok::configuration(limits, guard, host_user, start_skills, skill_names).map(Some),
            _ => Ok(None),
        }
    }

    /// The generated system prompt its kept home needs, if any.
    pub fn kept_system(self, told: &crate::start::Told) -> Option<String> {
        match self {
            Harness::Kimi => kimi::system(told),
            Harness::Grok => grok::system(told),
            _ => None,
        }
    }

    /// Where a start directory's limits for it are, in its own form.
    pub const fn limits(self) -> &'static str {
        each!(self, LIMITS)
    }

    /// Whether a directory holds limits for it, given what it `held` at
    /// `limits`. What they refuse is not read. An error is something there
    /// that cannot be read as it must be.
    pub fn limited<'de, D: Deserializer<'de>>(self, held: Held<D>) -> Result<bool, D::Error> {
        match self {
            Harness::ClaudeCode => claude_code::limited(held),
            Harness::Pi => Ok(pi::limited(held)),
            Harness::Codex => codex::limited(held),
            Harness::Dsh => Ok(matches!(held, Held::File(_))),
            Harness::Kimi => kimi::limited(held),
            Harness::Grok => grok::limited(held),
        }
    }

    /// `limited`, of a limits file's text.
    pub fn limited_file(self, text: &str) -> Result<bool, String> {
        match self {
            Harness::Kimi => kimi::limited_file(text),
            Harness::Grok => grok::limited_file(text),
            _ => {
                let mut json = serde_json::Deserializer::from_str(text);
                self.limited(Held::File(&mut json)).map_err(|error| error.to_string())
            }
        }
    }

    /// Where it reads the start directory's rules, when it cannot read the
    /// shared rules file and needs a link to it.
    pub const fn rules_link(self) -> Option<&'static str> {
        match self {
            Harness::Kimi => Some(kimi::RULES_LINK),
            _ => None,
        }
    }

    /// The files it would load from a start directory in place of the shared
    /// rules file (`start::RULES`).
    pub const fn before_the_rules(self) -> &'static [&'static str] {
        match self {
            Harness::Pi => &pi::BEFORE_THE_RULES,
            Harness::Codex => &codex::BEFORE_THE_RULES,
            Harness::Kimi => &kimi::BEFORE_THE_RULES,
            _ => &[],
        }
    }

    /// The files the repo's own rules are told from on its start line (the
    /// first the worktree holds), with the added rules (`Start::told`).
    /// Empty: it loads both itself.
    pub const fn told(self) -> &'static [&'static str] {
        match self {
            Harness::Codex => &codex::REPO_RULES,
            Harness::Kimi => &kimi::REPO_RULES,
            Harness::Grok => &grok::REPO_RULES,
            _ => &[],
        }
    }

    /// The start directory's rules file it is told (`Told::own`), for a
    /// harness that starts in the worktree instead. `None`: it loads it.
    pub const fn rules_file(self) -> Option<&'static str> {
        match self {
            Harness::Grok => Some(grok::RULES_FILE),
            _ => None,
        }
    }

    /// The directories of its start directory where it finds skills, when
    /// not `start::SKILLS`: a start directory holding none of them gets the
    /// first linked to its skills.
    pub const fn skills(self) -> &'static [&'static str] {
        match self {
            Harness::Codex => &codex::SKILLS,
            _ => &[],
        }
    }

    /// The host user's skill directories it reads beside the start
    /// directory's, showing both of two skills of one name: the host one is
    /// turned off (`Start::shadowed`). Empty: the start directory's wins.
    pub const fn shadowed_in(self) -> &'static [&'static str] {
        match self {
            Harness::Codex => &codex::SHADOWED_IN,
            _ => &[],
        }
    }

    /// The shell line whose answer `models` reads: the models its own login
    /// runs, asked of `command` with the kept directory `kept`.
    pub fn models_question(self, command: &str, kept: &str) -> String {
        match self {
            Harness::ClaudeCode => claude_code::models_question(command),
            Harness::Pi => pi::models_question(command, kept),
            Harness::Codex => codex::models_question(command, kept),
            Harness::Dsh => dsh::models_question(kept),
            Harness::Kimi => kimi::models_question(command, kept),
            Harness::Grok => grok::models_question(command, kept),
        }
    }

    /// The shell line sending `model` (or the default) the `prompt` through
    /// `command`, printing the answer.
    pub fn prompt_question(self, command: &str, kept: &str, model: Option<&str>, prompt: &str) -> String {
        match self {
            Harness::ClaudeCode => claude_code::prompt_question(command, model, prompt),
            Harness::Pi => pi::prompt_question(command, kept, model, prompt),
            Harness::Codex => codex::prompt_question(command, kept, model, prompt),
            Harness::Dsh => dsh::prompt_question(kept, model, prompt),
            Harness::Kimi => kimi::prompt_question(command, kept, model, prompt),
            Harness::Grok => grok::prompt_question(command, kept, model, prompt),
        }
    }

    /// As `prompt_question`, answering in a form that says which model
    /// answered (`answered_as`).
    pub fn probe_question(self, command: &str, kept: &str, model: Option<&str>, prompt: &str) -> String {
        match self {
            Harness::ClaudeCode => claude_code::probe_question(command, model, prompt),
            Harness::Pi => pi::probe_question(command, kept, model, prompt),
            Harness::Codex => codex::probe_question(command, kept, model, prompt),
            Harness::Dsh => dsh::prompt_question(kept, model, prompt),
            Harness::Kimi => kimi::probe_question(command, kept, model, prompt),
            Harness::Grok => grok::probe_question(command, kept, model, prompt),
        }
    }

    /// Which model answered a probe, from what `probe_question` printed.
    pub fn answered_as(self, answer: &str) -> AnsweredAs {
        let said = match self {
            Harness::ClaudeCode => claude_code::answered_as(answer),
            Harness::Pi => pi::answered_as(answer),
            Harness::Codex => return codex::answered_as(answer),
            Harness::Dsh => dsh::answered_as(answer),
            Harness::Kimi => kimi::answered_as(answer),
            Harness::Grok => grok::answered_as(answer),
        };
        said.map_or(AnsweredAs::Unsaid, AnsweredAs::Said)
    }

    /// Which model answered, from the session file `AnsweredAs::InSession`
    /// names.
    pub fn answered_in_session(self, session: &str) -> Option<String> {
        match self {
            Harness::Codex => codex::answered_in_session(session),
            _ => None,
        }
    }

    /// Each listed model's display name, by the name `models` gives it.
    pub fn shown(self, answer: &str) -> Vec<(String, String)> {
        match self {
            Harness::ClaudeCode => claude_code::shown(answer),
            Harness::Dsh => dsh::shown(answer),
            Harness::Kimi => kimi::shown(answer),
            Harness::Grok => grok::shown(answer),
            Harness::Pi | Harness::Codex => vec![],
        }
    }

    /// Why the model listing failed, where the harness says.
    pub fn listing_failure(self, answer: &str) -> Option<String> {
        match self {
            Harness::Dsh => dsh::listing_failure(answer),
            _ => None,
        }
    }

    /// `command` started on `model`, its model flag after it; DSH, which
    /// takes no model flag, as written.
    pub fn with_model(self, command: &str, model: &str) -> String {
        let Some(flag) = self.model_flag() else { return command.to_string() };
        Line::new().launch(command).fixed(" ").fixed(flag).fixed(" ").word(model).done()
    }

    fn model_flag(self) -> Option<&'static str> {
        match self {
            Harness::Dsh => None,
            Harness::ClaudeCode => Some(claude_code::MODEL),
            Harness::Pi => Some(pi::MODEL),
            Harness::Codex => Some(codex::MODEL),
            Harness::Kimi => Some(kimi::MODEL),
            Harness::Grok => Some(grok::MODEL),
        }
    }

    /// The models it knows, from what `models_question` printed.
    pub fn models(self, answer: &str) -> Option<Vec<String>> {
        each!(self, models(answer))
    }

    /// Whether `model`, its own name for one, is among `listed`. Every
    /// harness but pi takes any name and fails at its first turn on a wrong
    /// one, so only a whole name counts.
    pub fn knows(self, listed: &[String], model: &str) -> bool {
        match self {
            Harness::Pi => pi::knows(listed, model),
            _ => whole(listed, model),
        }
    }

    /// Whether `model` is the whole name of one of `listed`.
    pub fn names(self, listed: &[String], model: &str) -> bool {
        match self {
            Harness::Pi => pi::names(listed, model),
            _ => whole(listed, model),
        }
    }

    /// The launch configuration that trusts a session's worktree.
    pub const fn worktree_trust(self) -> &'static str {
        each!(self, TRUST)
    }

    /// Whether `worktree_trust` is flags the launch command must take.
    pub const fn trust_is_flags(self) -> bool {
        each!(self, TRUST_IS_FLAGS)
    }

    /// A session in `worktree` with nothing added, trusting it in this
    /// harness's own way. Codex also gets its own user directory `kept`.
    pub fn in_worktree(self, command: &str, worktree: &str, kept: &str) -> String {
        match self {
            Harness::ClaudeCode => claude_code::in_worktree(command),
            Harness::Pi => pi::in_worktree(command),
            Harness::Codex => codex::in_worktree(command, worktree, kept),
            Harness::Dsh => dsh::in_worktree(command),
            Harness::Kimi => kimi::in_worktree(command),
            Harness::Grok => grok::in_worktree(command),
        }
    }

    /// `command` with the hook `guard` asked before each tool call, and
    /// `HARNESS_ENV` set. Kimi's and Grok's `kept` homes already hold it.
    pub fn guarded(self, command: &str, worktree: &str, kept: &str, guard: &str) -> String {
        let command = match self {
            Harness::ClaudeCode => claude_code::guarded(command, guard),
            Harness::Pi => pi::guarded(command, worktree, kept, guard),
            Harness::Codex => codex::guarded(command, guard),
            Harness::Dsh => dsh::guarded(command, guard),
            Harness::Kimi => kimi::guarded(command, kept),
            Harness::Grok => grok::guarded(command, kept),
        };
        Line::new()
            .fixed("export ")
            .fixed(HARNESS_ENV)
            .fixed("=")
            .word(self.key())
            .fixed(" && ")
            .launch(&command)
            .done()
    }

    /// An interactive session with `in_worktree`'s trust, the login and
    /// `model`. Its brief is typed after startup.
    pub fn interactive(self, command: &str, worktree: &str, kept: &str, model: Option<&str>) -> String {
        match self {
            Harness::ClaudeCode => claude_code::interactive(command, model),
            Harness::Pi => pi::interactive(command, kept, model),
            Harness::Codex => codex::interactive(command, worktree, kept, model),
            Harness::Dsh => dsh::interactive(command, worktree, kept, model),
            Harness::Kimi => kimi::interactive(command, kept, model),
            Harness::Grok => grok::interactive(command, model),
        }
    }

    /// How it keeps a session of its own (Codex: a fresh directory, the
    /// user's model unless one is named). `None`: the launch command, the
    /// model and the kept directory are the whole start.
    pub const fn session(self) -> Option<Session> {
        match self {
            Harness::Codex => Some(codex::SESSION),
            _ => None,
        }
    }

    /// The model of an automatic start: `explicit` wins, else the user's,
    /// read from `text`, the config `file` (`Session::config`), which must be
    /// one of `listed`. `None`: no such preference. An error is a preference
    /// the start cannot use.
    pub fn automatic_model(
        self,
        explicit: Option<&str>,
        text: &str,
        file: &str,
        listed: &[String],
    ) -> Option<Result<(Option<String>, &'static str), String>> {
        match self {
            Harness::Codex => Some(codex::automatic_model(explicit, text, file, listed)),
            _ => None,
        }
    }

    /// The effort of a start, from the same config; `None`: it names none.
    pub fn effort(self, text: &str, file: &str) -> Option<Result<(Option<String>, String), String>> {
        match self {
            Harness::Codex => Some(codex::effort(text, file)),
            _ => None,
        }
    }

    /// The service tier of a start, from the same config. Outer `None`: it
    /// names none; inner `None`: Codex chooses its default.
    pub fn service_tier(self, text: &str, file: &str) -> Option<Result<Option<String>, String>> {
        match self {
            Harness::Codex => Some(codex::service_tier(text, file)),
            _ => None,
        }
    }

    /// What the session at `file` saved of its model and effort, from `text`.
    pub fn saved(self, file: &str, text: &str) -> Option<Saved> {
        match self {
            Harness::Codex => codex::saved(file, text),
            _ => None,
        }
    }

    /// `command` naming `model`, unless it already does.
    pub fn ensure_model(self, command: &str, model: Option<&str>) -> String {
        match (self, model, self.model_flag()) {
            (Harness::Codex, _, _) => codex::ensure_model(command, model),
            (_, Some(model), Some(flag)) if !command.contains(flag) => self.with_model(command, model),
            _ => command.to_string(),
        }
    }

    /// `line` with this harness's `effort`, where it names one.
    pub fn with_effort(self, line: &str, effort: Option<&str>) -> String {
        match self {
            Harness::Codex => codex::with_effort(line, effort),
            _ => line.to_string(),
        }
    }

    /// `line` with this harness's service tier, where it names one.
    pub fn with_service_tier(self, line: &str, service_tier: Option<&str>) -> String {
        match self {
            Harness::Codex => codex::with_service_tier(line, service_tier),
            _ => line.to_string(),
        }
    }

    /// The line an interactive session starts with, in `kept`. A harness
    /// that keeps a session of its own adds search and `effort`; every other
    /// is `interactive`.
    pub fn session_interactive(
        self,
        command: &str,
        worktree: &str,
        kept: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> String {
        match self {
            Harness::Codex => codex::session_interactive(command, worktree, kept, model, effort),
            _ => self.interactive(command, worktree, kept, model),
        }
    }

    /// The line a terminal runs to start a session with the launch command
    /// `command`, as `start` says: change to the directory it loads from,
    /// export its own variables and `start.env`, and add what it does not
    /// load from there — the worktree and rules, the user's configuration
    /// left out, the hook and the status report, the host user's skills and
    /// rules where given, the model. Grok works only in its cwd, so it starts
    /// in `start.worktree`; every other harness in `start.dir`.
    pub fn start_line(self, command: &str, start: &Start) -> String {
        self.start_built(command, start).done()
    }

    fn start_built(self, command: &str, start: &Start) -> Line {
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
        let line = line.fixed(" && ").launch(command);
        each!(self, added(line, start))
    }

    /// The line continuing a session `stop_answer` kept, started as
    /// `start_line` starts one, with `instruction` as its next user message.
    /// `session` is its id (`session_id`); `None` continues the latest of the
    /// directory. `None` of a harness that cannot continue a session.
    pub fn resumed(self, command: &str, start: &Start, session: Option<&str>, instruction: &str) -> Option<String> {
        let line = self.start_built(command, start);
        let line = match self {
            Harness::ClaudeCode => claude_code::resumed(line, session, instruction),
            Harness::Codex => codex::resumed(line, session, instruction),
            _ => return None,
        };
        Some(line.done())
    }

    /// The id of the session whose file holds `text`, which `resumed` takes.
    pub fn session_id(self, text: &str) -> Option<String> {
        match self {
            Harness::ClaudeCode => claude_code::session_id(text),
            Harness::Codex => codex::session_id(text),
            _ => None,
        }
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

    /// The shell line whose answer says which flags `command` takes.
    pub fn flags_question(self, command: &str) -> String {
        each!(self, asked_its_flags(Line::new().launch(command))).done()
    }

    /// Whether `command` takes every flag a start adds, given what
    /// `flags_question` printed (`None`: it failed or printed nothing). An
    /// error names the reason.
    pub fn takes(self, command: &str, answer: Option<&str>) -> Result<(), String> {
        let refused = |why: String| {
            Err(format!(
                "the launch command {command:?} cannot take the start flags: {why}. A launch \
                 command must end in {} taking flags",
                self.name()
            ))
        };
        let question = self.flags_question(command);
        let Some(answer) = answer.filter(|answer| !answer.trim().is_empty()) else {
            return refused(format!("`{question}` failed in a login shell, or said nothing"));
        };
        let missing: Vec<&str> = self.flags().iter().copied().filter(|flag| !answer.contains(flag)).collect();
        if missing.is_empty() { Ok(()) } else { refused(format!("`{question}` names no {}", missing.join(", no "))) }
    }

    /// Whether `screen`, the end of a stopped session's terminal, shows work
    /// it started still running in the background.
    pub fn shows_background(self, screen: &[String]) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::shows_background(screen),
            Harness::Kimi => kimi::shows_background(screen),
            _ => false,
        }
    }

    /// Whether a session file's `text` records background work of the
    /// session's own process (an agent or shell it launched) not yet
    /// reported ended. A child session's lines do not count.
    pub fn background_pending(self, text: &str) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::background_pending(text),
            _ => false,
        }
    }

    /// Whether any harness shows background work on `screen`.
    pub fn any_shows_background(screen: &[String]) -> bool {
        Harness::ALL.into_iter().any(|harness| harness.shows_background(screen))
    }

    /// The tool call `described` to a hook; an error is no tool call.
    pub fn call<'de, D: Deserializer<'de>>(self, described: D) -> Result<Call, D::Error> {
        each!(self, call(described))
    }

    /// Where it keeps the sessions of the worktree `cwd`. A kept directory,
    /// and a home [`Harness::noted_home`] names, are already `under_home`.
    pub fn session_place(self, cwd: &str) -> SessionPlace {
        let layout = each!(self, session_layout(cwd));
        SessionPlace { under_home: each!(self, HOME), layout, also: None }
    }

    /// The turn a session file's `text` records for the worktree `cwd`.
    /// `None` when the file is another worktree's, or holds no turn that
    /// started at or after `not_before`. A child session does not start or
    /// end the turn. `brief` is the file handed to this session: a harness
    /// started elsewhere is known by its first prompt naming it.
    pub fn recorded_turn(self, cwd: &str, text: &str, not_before: Option<u64>, brief: Option<&str>) -> Option<Turn> {
        match self {
            Harness::ClaudeCode => claude_code::recorded_turn(cwd, text, not_before, brief),
            Harness::Pi => pi::recorded_turn(cwd, text, not_before),
            Harness::Codex => codex::recorded_turn(cwd, text, not_before, brief),
            Harness::Dsh => dsh::recorded_turn(cwd, text, not_before),
            Harness::Kimi => kimi::recorded_turn(cwd, text, not_before),
            Harness::Grok => grok::recorded_turn(cwd, text, not_before),
        }
    }

    /// Whether `text`, whole lines of a session file, could make it `cwd`'s
    /// session for `recorded_turn`, without parsing it: false only where no
    /// file made of such lines has that turn, so a reader of a growing file
    /// parses it only once a new line is true here. A harness writing every
    /// worktree's sessions into one tree knows its own by a line naming
    /// `cwd` or `brief`, which JSON writes as themselves unless they hold a
    /// quote, a backslash or a control character.
    pub fn could_record(self, cwd: &str, text: &str, brief: Option<&str>) -> bool {
        match self {
            Harness::ClaudeCode | Harness::Codex => {
                let written_as_is = |path: &str| !path.chars().any(|ch| ch == '"' || ch == '\\' || ch.is_control());
                let cwd = cwd.trim_end_matches('/');
                if cwd.is_empty() || !written_as_is(cwd) || brief.is_some_and(|b| !written_as_is(b)) {
                    return true;
                }
                text.contains(cwd) || brief.is_some_and(|brief| text.contains(brief))
            }
            _ => true,
        }
    }

    /// The token use a session file's `text` records: all of it, or from
    /// `window.0` through `window.1`. `None` is no usage evidence, not zero.
    pub fn tokens(self, text: &str, window: Option<(u64, u64)>) -> Option<crate::tokens::Tokens> {
        crate::tokens::fold(text, window, |line, n| self.usage(line, n))
    }

    /// The steps a session file's `text` records, oldest first: each tool
    /// call with its result and whether it was marked an error. A call with
    /// no result is no step; a child session's lines are not counted. Grok's
    /// calls are in the file its layout names beside its events.
    pub fn steps(self, text: &str) -> Vec<crate::steps::Step> {
        crate::steps::fold(text, |line| self.said(line))
    }

    fn said(self, line: &serde_json::Value) -> Vec<crate::steps::Said> {
        each!(self, said(line))
    }

    fn usage(self, line: &serde_json::Value, n: usize) -> Option<crate::tokens::Usage> {
        match self {
            Harness::ClaudeCode => claude_code::usage(line, n),
            Harness::Pi => pi::usage(line, n),
            Harness::Codex => codex::usage(line, n),
            Harness::Kimi => kimi::usage(line, n),
            Harness::Grok => grok::usage(line, n),
            Harness::Dsh => None,
        }
    }

    /// The session files an index names, each marked whether its line names
    /// `cwd`. A session filed under another directory counts only when it
    /// names the brief ([`Harness::names_brief`]).
    pub fn indexed_sessions(self, cwd: &str, index: &str) -> Vec<Indexed> {
        match self {
            Harness::Kimi => kimi::indexed_sessions(cwd, index),
            _ => Vec::new(),
        }
    }

    /// The line typed at a session to hand it `brief`: for Codex, set as its
    /// goal, which it continues by itself; for the rest, the brief pointer.
    pub fn brief_line(self, brief: &str) -> String {
        match self {
            Harness::Codex => codex::brief_line(brief),
            _ => crate::brief::brief_pointer(brief),
        }
    }

    /// How a session nobody types at is handed `brief`, given its start
    /// `line`. Claude Code takes the pointer as its first prompt; Codex's
    /// goal is set only by `/goal` typed at its prompt; the rest take it
    /// typed.
    pub fn brief_at_start(self, line: &str, brief: &str) -> BriefAtStart {
        match self {
            Harness::ClaudeCode => BriefAtStart::Started(claude_code::brief_at_start(line, brief)),
            _ => BriefAtStart::Typed(self.brief_line(brief)),
        }
    }

    /// `line` with its goal's token budget, where it keeps a goal.
    pub fn with_goal_budget(self, line: &str) -> String {
        match self {
            Harness::Codex => codex::with_goal_budget(line),
            _ => line.to_string(),
        }
    }

    /// A goal's token budget, where it keeps one.
    pub fn goal_tokens(self) -> Option<u64> {
        match self {
            Harness::Codex => Some(codex::GOAL_TOKENS),
            _ => None,
        }
    }

    /// Where it keeps a session's goal.
    pub fn goal_store(self) -> Option<GoalStore> {
        match self {
            Harness::Codex => Some(codex::GOAL_STORE),
            _ => None,
        }
    }

    /// The goal the store's `answer` to its query says.
    pub fn goal(self, answer: &str) -> Option<Goal> {
        match self {
            Harness::Codex => codex::goal(answer),
            _ => None,
        }
    }

    /// What the session record `text` says of its goal.
    pub fn goal_session(self, text: &str) -> Option<GoalSession> {
        match self {
            Harness::Codex => Some(codex::goal_session(text)),
            _ => None,
        }
    }

    /// The store query whose answer is the row of the session `thread`;
    /// `None` where it keeps no goal or `thread` is no id it writes.
    pub fn goal_row_query(self, thread: &str) -> Option<String> {
        match self {
            Harness::Codex => codex::goal_row_query(thread),
            _ => None,
        }
    }

    /// The row the store's `answer` to `goal_row_query` says: `Some(None)`
    /// where there is none.
    pub fn goal_row(self, answer: &str) -> Option<Option<GoalRow>> {
        match self {
            Harness::Codex => codex::goal_row(answer),
            _ => None,
        }
    }

    /// What, typed, ends a session's goal, its turn left to finish.
    pub fn goal_end(self) -> Option<Keystroke> {
        match self {
            Harness::Codex => Some(codex::GOAL_END),
            _ => None,
        }
    }

    /// Whether the session `text` shows a goal turn begun by itself after an
    /// earlier turn ended.
    pub fn goal_continued(self, text: &str) -> bool {
        match self {
            Harness::Codex => codex::goal_continued(text),
            _ => false,
        }
    }

    /// Whether a session file's `text` was prompted with the brief at
    /// `brief`, by the user messages that identify a session (not tool
    /// output or model replies).
    pub fn names_brief(self, text: &str, brief: &str) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::names_brief(text, brief),
            Harness::Codex => codex::names_brief(text, brief),
            Harness::Kimi => kimi::names_brief(text, brief),
            _ => false,
        }
    }

    /// The files a stand-in writes under the harness directory so a later
    /// read sees a turn in `cwd`: left open when `open`, written at `at`
    /// (epoch seconds).
    pub fn session_files(self, cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
        each!(self, session_files(cwd, open, at))
    }

    /// Where it keeps the words of `cwd`'s session: Claude Code's project
    /// directory, Grok's message file, or where it keeps its turn.
    pub fn note_place(self, cwd: &str) -> SessionPlace {
        match self {
            Harness::ClaudeCode => SessionPlace {
                under_home: claude_code::HOME,
                layout: claude_code::note_layout(cwd),
                also: Some(claude_code::session_layout(cwd)),
            },
            Harness::Grok => SessionPlace { under_home: grok::HOME, layout: grok::note_layout(cwd), also: None },
            _ => self.session_place(cwd),
        }
    }

    /// Whether a session file's `text` holds `mark` in a message its model
    /// was given as the user's; a line only queued, or a tool's output, is
    /// not one.
    pub fn holds_note(self, text: &str, mark: &str) -> bool {
        session::holds_note(text, mark)
    }

    /// When the first such message holding `mark` was recorded, in epoch
    /// seconds.
    pub fn noted_at(self, text: &str, mark: &str) -> Option<u64> {
        session::noted_at(text, mark)
    }

    /// The files a stand-in writes for `line` typed at a session in `cwd`
    /// mid-turn: given to its model when `taken`, else only queued.
    pub fn note_files(self, cwd: &str, line: &str, taken: bool, at: u64) -> Vec<SessionFile> {
        match self {
            Harness::ClaudeCode => claude_code::note_files(cwd, line, taken, at),
            Harness::Grok => grok::note_files(cwd, line, taken, at),
            _ => Vec::new(),
        }
    }

    /// The index after `cwd`'s turn is recorded, given the `existing` index
    /// (`""`: none). `None` where it keeps no index. Other lines stay.
    pub fn session_index(self, cwd: &str, existing: &str) -> Option<String> {
        match self {
            Harness::Kimi => Some(kimi::session_index(cwd, existing)),
            _ => None,
        }
    }

    /// The name of a note beside the worktree's git directory whose text is
    /// another harness directory holding this worktree's sessions.
    pub const fn noted_home(self) -> Option<&'static str> {
        match self {
            Harness::Codex => Some(codex::HOME_NOTE),
            _ => None,
        }
    }

    /// The hook exit code that refuses a call: it is not made, and the
    /// hook's stderr is shown to the session.
    pub const fn refused(self) -> u8 {
        each!(self, REFUSED)
    }

    /// What a hook before a tool call answers to end the session's turn
    /// cleanly before that call, the session kept to be `resumed`. `reason`
    /// is shown. Whether it is honoured is `stop_reliable`.
    pub fn stop_answer(self, reason: &str) -> Option<Answer> {
        match self {
            Harness::ClaudeCode => Some(claude_code::stop_answer(reason)),
            Harness::Codex => Some(codex::stop_answer(reason)),
            _ => None,
        }
    }

    /// Whether it is known to honour its `stop_answer`.
    pub const fn stop_reliable(self) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::STOP_RELIABLE,
            Harness::Codex => codex::STOP_RELIABLE,
            _ => false,
        }
    }

    /// The hook events whose payloads say a session's status (`hook_status`).
    /// `None`: its hooks do not say it reliably; read its session file.
    pub fn status_events(self) -> Option<&'static [&'static str]> {
        match self {
            Harness::ClaudeCode => Some(&claude_code::STATUS_EVENTS),
            Harness::Codex => Some(&codex::STATUS_EVENTS),
            _ => None,
        }
    }

    /// The hook settings running `command` at each of `status_events`, the
    /// payload on its stdin: settings for Claude Code, `hooks.json` for Codex.
    pub fn status_hooks(self, command: &str) -> Option<String> {
        self.status_events().map(|events| crate::status::hooks_json(events, command))
    }

    /// What one hook event's `payload` says of its session's status (OSC
    /// 7501's words), with the line, the tool and the call's end.
    pub fn hook_status(self, payload: &serde_json::Value) -> Option<crate::status::Reading> {
        match self {
            Harness::ClaudeCode => Some(claude_code::hook_status(payload)),
            Harness::Codex => Some(codex::hook_status(payload)),
            _ => None,
        }
    }
}

/// What a hook prints on its standard output, and the code it exits with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub printed: String,
    pub code: u8,
}

/// Whether `model` is one of `listed` by its whole name.
fn whole(listed: &[String], model: &str) -> bool {
    listed.iter().any(|name| name == model.trim())
}
