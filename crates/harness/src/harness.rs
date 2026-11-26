//! A harness: the program a session runs in. How it starts, how a tool call
//! reaches the hook, where it keeps its session and which lines start or end
//! a turn, which screen words mean a quota or a refusal, and how an account
//! of it logs in are each one harness's own: its module here implements
//! [`Kind`], and nothing else names it but its one line of `harnesses!`.

use std::path::Path;

use serde::{Deserialize, Deserializer};

use crate::ask::Headless;
use crate::line::Line;
use crate::start::{Home, Kept, Launch, Start};
use crate::turn::Turn;

/// Everything one harness's module says of it. No item has a default: a
/// harness that leaves one out does not compile.
trait Kind {
    /// What a config or a record writes it as.
    const KEY: &'static str;
    /// How an account of it is logged in.
    const LOGIN: Login;
    /// Where its sessions keep their files, when no login account has a home
    /// of its own: a directory under the host user's home, or `None`: the
    /// directory the host keeps for it.
    const USER_HOME: Option<&'static str>;
    /// The host user's files its home links to; a login's file first.
    const KEPT_LINKS: &'static [Link];
    /// Its program, as a session starts it when stablehand keeps no
    /// command for it (`Harness::program`).
    const PROGRAM: &'static str;
    /// What a bare program is started with (`Harness::command`).
    const STOCK_ARGUMENTS: &'static str;
    /// Words of a question it asks before its screen is up, which typed
    /// text would answer.
    const ASKS_FIRST: Option<&'static str>;
    /// Words its own screen shows once it is up, where it has some: until
    /// they show, it is still starting, and what is typed is lost.
    const SHOWS_UP: Option<&'static str>;
    /// The questions it opens on its own mid-session, in a dialog that
    /// takes what is typed at it, each with the key that dismisses it.
    const ASKS_MIDWAY: &'static [Dialog];
    /// The variables that switch it, each set to 1.
    const ENVIRONMENT: &'static [&'static str];
    /// The hook exit code that refuses a call: it is not made, and the
    /// hook's stderr is shown to the session.
    const REFUSED: u8;
    /// The key that, typed without Enter, stops its running turn and leaves
    /// it at its prompt.
    const INTERRUPT: &'static str;
    /// How a fresh session of it is asked one question headless; `None`:
    /// it is not, here.
    const HEADLESS: Option<Headless>;
    /// The files the tool carries or makes for it, by path under its home.
    /// An error: a file of the host user's they are made from could not be
    /// read as it must be.
    fn kept(start: &Start) -> Result<Vec<(&'static str, String)>, String>;
    /// The kept file, under its home, its sessions are told where their
    /// rules are in, where it keeps one.
    const TOLD_IN: Option<&'static str>;
    /// The guard lines the files it keeps under `home` run before each tool
    /// call, as they stand there, a variable its line sets written `$NAME`:
    /// none where they are not there as `kept` makes them. `None`: it keeps
    /// no guard; its line gives it one.
    fn guards(home: &Path) -> Option<Vec<String>>;
    /// The directory a session of it starts in.
    fn start_dir<'a>(start: &Start<'a>) -> &'a str;
    /// `line`, which exports a session's environment, with its own variables.
    fn environment(line: Line, start: &Start) -> Line;
    /// `line`, which starts the launch command, with what it is given.
    fn added(line: Line, start: &Start) -> Line;
    /// The start `line` handing it `brief`, and what is typed at it once it
    /// runs, where the brief is not on the line.
    fn briefed(line: String, brief: &str) -> (String, Option<String>);
    /// The tool call `described` to a hook, as the guard judges one.
    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error>;
    /// What `screen`, the end of a session's terminal, says stopped it.
    fn stopped(screen: &[String]) -> Option<Stop>;
    /// How the session files of the worktree `cwd` lie under its home.
    fn layout(cwd: &str) -> Layout;
    /// Whether `text` could make it `cwd`'s session for `recorded_turn`,
    /// without parsing it: a cheap look before a long file is parsed.
    fn could_record(cwd: &str, text: &str, brief: &str) -> bool;
    /// The turn a session file's `text` records for the worktree `cwd`.
    /// `None` when the file is another worktree's or holds no turn. A child
    /// session does not start or end the turn. `brief` is the file handed to
    /// this session: a harness started elsewhere is known by its first
    /// prompt naming it.
    fn recorded_turn(cwd: &str, text: &str, brief: &str) -> Option<Turn>;
    /// The session file it would write in `cwd` for the session handed
    /// `brief`, its turn left open when `open`, written at `at` (epoch
    /// seconds).
    fn stand_in(cwd: &str, brief: &str, open: bool, at: u64) -> String;
    /// The tokens a session file's `text` says its session spent, as (given,
    /// written), the given counting cached ones. `None`: its files do not say.
    const SPENT: Option<ReadSpent>;
    /// Whether the session file `text` took `said` as a prompt at or after
    /// `since` (epoch seconds). `None`: its session files do not say.
    fn prompted(text: &str, said: &str, since: u64) -> Option<bool>;
}

/// What reads the tokens a session file's text says it spent, as (given,
/// written).
type ReadSpent = fn(text: &str) -> (u64, u64);

/// Lists every harness once, as `Variant in module`, the module's [`Kind`]
/// being its type of the variant's name: makes `Harness`, `ALL`, and
/// `each!`, which evaluates a body with `K` that harness's [`Kind`]. Each
/// module is declared on its own line below, where cargo-mutants finds its
/// file (it reads no `mod` a macro makes); one listed here and not there
/// does not build.
macro_rules! harnesses {
    ($d:tt $($variant:ident in $module:ident),+ $(,)?) => {
        /// A harness a session can be started in.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Harness { $($variant),+ }

        const ALL: &[Harness] = &[$(Harness::$variant),+];

        macro_rules! each {
            ($d harness:expr, |$d kind:ident| $d body:expr) => {
                match $d harness {
                    $(Harness::$variant => {
                        type $d kind = crate::harness::$module::$variant;
                        $d body
                    })+
                }
            };
        }
    };
}

harnesses! { $
    ClaudeCode in claude_code,
    Pi in pi,
    Codex in codex,
    Dsh in dsh,
    Kimi in kimi,
    Grok in grok,
    Agy in agy,
}

mod agy;
mod claude_code;
mod codex;
mod dsh;
mod grok;
mod kimi;
mod pi;
mod seen;
mod session;
mod store;

use store::Layout;

/// One tool call a session is about to make, as far as a hook judges it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// It runs this shell line: the only kind a hook judges.
    Shell(String),
    /// It was read, and runs no shell line.
    NotShell,
    /// It could not be read, for this reason: the hook refuses it, since
    /// what it runs is not known.
    Unreadable(String),
}

/// One tool call as Claude Code describes it to a hook before making it, and
/// Codex and Kimi Code after it: the tool's name, and its input's `command`.
#[derive(Deserialize)]
struct Described {
    tool_name: String,
    #[serde(default)]
    tool_input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    /// The shell line, when the tool runs one (of Codex's `apply_patch`, the
    /// patch).
    #[serde(default)]
    command: Option<String>,
}

/// The tool call `described` to a hook in Claude Code's form, as the guard
/// judges one: a shell call when the tool is `shell_tool`.
fn tool_call<'de, D: Deserializer<'de>>(described: D, shell_tool: &str) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.tool_input.command {
        Some(line) if described.tool_name == shell_tool => Call::Shell(line),
        _ => Call::NotShell,
    })
}

/// A file or directory of the host user's that a kept directory links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Link {
    /// Where it is, under the host user's home.
    pub(crate) home: &'static str,
    /// Where the link is, under the kept directory.
    pub(crate) kept: &'static str,
}

/// A question a harness opens on its own mid-session, in a dialog that
/// takes what is typed at it: a line typed there never reaches the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dialog {
    /// Words of the question, on one of its rows.
    asks: &'static str,
    /// Words of its last row, the keys it takes.
    keys: &'static str,
    /// The key, typed without Enter, that dismisses it.
    dismiss: &'static str,
}

/// How many rows from the end of a screen a dialog's question may be.
const DIALOG_ROWS: usize = 20;

/// How an account of a harness is logged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Login {
    /// By a token the account holds in this field, usable on several
    /// machines at once; a session is given it in the variable of that name.
    Token(&'static str),
    /// By a refreshing login in the account's own home, bound to the one
    /// machine it was made on: that home is its sessions' home, which the
    /// variable `user_dir` names.
    Machine { user_dir: &'static str },
    /// It takes no account.
    Unused,
}

/// What a session's screen says stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Its account is used up: the line that says so.
    Quota(String),
    /// Its engine is at its limit, which resets in this many ms: the lines
    /// that say so, and the window of its account it spent, as stablehand
    /// names it (`None`: the whole account).
    Limit { said: String, resets_in_ms: i64, window: Option<&'static str> },
    /// Its harness refused the task or ran out of capacity: the line.
    Refused(String),
}

impl Harness {
    /// What a config or a record writes it as.
    pub const fn key(self) -> &'static str {
        each!(self, |K| K::KEY)
    }

    /// The harness written as `key`.
    pub fn read(key: &str) -> Option<Harness> {
        ALL.iter().copied().find(|harness| harness.key() == key)
    }

    /// Every harness, in the order they are listed.
    pub fn all() -> &'static [Harness] {
        ALL
    }

    /// The launch command a `given` command (the one stablehand keeps for
    /// it, else [`Harness::program`]) runs as. A bare program (a path or a
    /// name, no arguments) gets this harness's stock arguments, as its stock
    /// command would: naming where a program is must not drop the flags a
    /// worker relies on. A command with arguments of its own is taken as
    /// given.
    pub fn command(self, given: &str) -> String {
        each!(self, |K| command::<K>(given))
    }

    /// Its program: what a session of it starts on when stablehand keeps
    /// no command for it (`stablehand command show`).
    pub fn program(self) -> &'static str {
        each!(self, |K| K::PROGRAM)
    }

    /// How an account of it is logged in.
    pub(crate) const fn login(self) -> Login {
        each!(self, |K| K::LOGIN)
    }

    /// The host user's files its kept directory links, its login first.
    pub(crate) fn links(self) -> &'static [Link] {
        each!(self, |K| K::KEPT_LINKS)
    }

    /// Where its sessions keep their files on this host, decided once from
    /// the host user's home `user_home`, the directory `account` this host
    /// keeps for sessions on a bound login (its login files linked there,
    /// `Start::logins`), if one, and the directory `kept` this host keeps
    /// for the harness: the account's; else its own directory under the host
    /// user's home where it has one, else the kept directory.
    pub fn home(self, user_home: &Path, account: Option<&Path>, kept: &Path) -> Home {
        Home::new(match (account, each!(self, |K| K::USER_HOME)) {
            (Some(home), _) => home.to_path_buf(),
            (_, Some(dir)) => user_home.join(dir),
            _ => kept.to_path_buf(),
        })
    }

    /// Everything it takes to start a session as `start` says, read but
    /// not done: what its home `start.home` must hold — the host user's
    /// login files linked, and the files the tool carries or makes for this
    /// harness, its guard among them — the line a terminal runs, and what is
    /// typed at it once it runs to hand it its brief. Each harness is given
    /// the worktree, the hook, Orca's status report and the model. An error:
    /// a file of the host user's it is made from could not be read as it
    /// must be.
    pub fn launch(self, start: &Start) -> Result<Launch, String> {
        each!(self, |K| launch::<K>(start))
    }

    /// The guard lines the files it keeps under `home` run before each tool
    /// call, as they stand there, a variable its line sets written `$NAME`:
    /// none where they are not there as its start makes them. `None`: it
    /// keeps no guard; its line gives it one.
    pub fn guards(self, home: &Home) -> Option<Vec<String>> {
        each!(self, |K| K::guards(home.path()))
    }

    /// What the file it keeps under `home` tells its sessions of where their
    /// rules are, empty where that file is not there. `None`: it keeps no
    /// such file; it reads its rules where it starts.
    pub fn told(self, home: &Home) -> Option<String> {
        let file = each!(self, |K| K::TOLD_IN)?;
        Some(std::fs::read_to_string(home.path().join(file)).unwrap_or_default())
    }

    /// The tool call a hook is handed as `payload`, as the guard judges it:
    /// unreadable when it is not one whole call as this harness describes one.
    pub fn shell_call(self, payload: &str) -> Call {
        let mut described = serde_json::Deserializer::from_str(payload);
        match each!(self, |K| K::call(&mut described)).and_then(|call| described.end().map(|()| call)) {
            Ok(call) => call,
            Err(e) => Call::Unreadable(format!("{} hook payload: {e}", self.key())),
        }
    }

    /// The hook exit code that refuses a call: it is not made, and the
    /// hook's stderr is shown to the session.
    pub const fn refused(self) -> u8 {
        each!(self, |K| K::REFUSED)
    }

    /// How a fresh session of it is asked one question headless.
    pub(crate) const fn headless(self) -> Option<Headless> {
        each!(self, |K| K::HEADLESS)
    }

    /// The key that, typed without Enter, stops its running turn and leaves
    /// it at its prompt.
    pub const fn interrupt(self) -> &'static str {
        each!(self, |K| K::INTERRUPT)
    }

    /// own screen up, ready to be typed at: blank, a shell's prompt (its
    /// program not started yet, or exited), a question it asks before its
    /// screen is up, or not yet the words its own screen shows once up. `None`: it is up.
    pub fn not_up(self, screen: &[String]) -> Option<String> {
        let Some(row) = crate::screen::last_row(screen) else { return Some("its screen is blank".into()) };
        if crate::screen::shell_prompt(row) {
            return Some(format!("its screen is a shell's: {row}"));
        }
        if each!(self, |K| K::ASKS_FIRST).is_some_and(|words| row.contains(words)) {
            return Some(format!("it asks before it starts: {row}"));
        }
        match each!(self, |K| K::SHOWS_UP) {
            Some(words) if !screen.iter().any(|r| r.contains(words)) => Some(format!("it is still starting: {row}")),
            _ => None,
        }
    }

    /// A question it opened on its own, which `screen` ends with: what it
    /// asks, and the key that dismisses it. Text typed now would go to it,
    /// not to the prompt.
    pub fn asking(self, screen: &[String]) -> Option<(String, &'static str)> {
        let last = crate::screen::last_row(screen)?;
        let rows = screen.iter().rev().take(DIALOG_ROWS);
        let dialogs = each!(self, |K| K::ASKS_MIDWAY).iter().filter(|d| last.contains(d.keys));
        let mut found = dialogs.flat_map(|d| rows.clone().filter(|r| r.contains(d.asks)).map(move |r| (r, d)));
        found.next().map(|(row, d)| (row.trim().to_string(), d.dismiss))
    }

    /// What `screen`, the end of a session's terminal, says stopped it.
    pub fn stopped(self, screen: &[String]) -> Option<Stop> {
        each!(self, |K| K::stopped(screen))
    }
}

fn launch<K: Kind>(start: &Start) -> Result<Launch, String> {
    let home = start.home.path();
    let mut kept = vec![Kept::Dir { path: home.to_path_buf() }];
    // The account's login, where it has one, in place of the host user's.
    let theirs = |kept: &str| start.logins.iter().any(|(at, _)| overlaps(at, kept));
    let links = (K::KEPT_LINKS.iter().filter(|link| !theirs(link.kept)))
        .map(|link| (start.user_home.join(link.home), home.join(link.kept)))
        .filter(|(to, _)| to.exists())
        .map(|(to, path)| Kept::Link { path, to });
    kept.extend(links);
    kept.extend(start.logins.iter().map(|(at, to)| Kept::Link { path: home.join(at), to: to.clone() }));
    let files = K::kept(start)?;
    kept.extend(files.into_iter().map(|(path, text)| Kept::File { path: home.join(path), text }));
    let mut line = Line::new().fixed("cd ").word(K::start_dir(start)).fixed(" && export");
    for variable in K::ENVIRONMENT {
        line = line.arg(variable).fixed("=1");
    }
    let mut line = K::environment(line, start);
    for (name, value) in &start.env {
        line = line.set(name, value);
    }
    let line = K::added(line.fixed(" && ").launch(&command::<K>(start.command)), start).done();
    let (line, typed) = K::briefed(line, start.brief);
    let line = match start.secrets {
        Some(file) => crate::line::secrets_first(file, &line),
        None => line,
    };
    Ok(Launch { kept, line, typed })
}

/// Whether the paths `a` and `b`, relative to one home, are one, or one
/// lies inside the other.
fn overlaps(a: &str, b: &str) -> bool {
    let (a, b) = (Path::new(a), Path::new(b));
    a.starts_with(b) || b.starts_with(a)
}

fn command<K: Kind>(given: &str) -> String {
    let given = given.trim();
    if K::STOCK_ARGUMENTS.is_empty() || given.contains(char::is_whitespace) {
        given.to_string()
    } else {
        Line::new().launch(given).arg(K::STOCK_ARGUMENTS).done()
    }
}
