//! A harness: the program a session runs in. How it starts, how a tool call
//! reaches the hook, where it keeps its session and which lines start or end
//! a turn, which screen words mean a quota or a refusal, and how an account
//! of it logs in are each one harness's own: its module here implements
//! [`Kind`], and nothing else names it but its one line of `harnesses!`.

use std::path::Path;

use serde::Deserializer;

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
    /// What a bare program is started with (`Harness::command`).
    const STOCK_ARGUMENTS: &'static str;
    /// Words of a question it asks before its screen is up, which typed
    /// text would answer.
    const ASKS_FIRST: Option<&'static str>;
    /// The variables that switch it, each set to 1.
    const ENVIRONMENT: &'static [&'static str];
    /// The hook exit code that refuses a call: it is not made, and the
    /// hook's stderr is shown to the session.
    const REFUSED: u8;
    /// The files the tool carries or makes for it, by path under its home.
    /// An error: a file of the host user's they are made from could not be
    /// read as it must be.
    fn kept(start: &Start) -> Result<Vec<(&'static str, String)>, String>;
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
    /// The session file it would write for the worktree `cwd`, its turn left
    /// open when `open`, written at `at` (epoch seconds).
    fn stand_in(cwd: &str, open: bool, at: u64) -> String;
}

/// Lists every harness once, as `Variant in module`, the module's [`Kind`]
/// being its type of the variant's name: makes `Harness`, `ALL`, and
/// `each!`, which evaluates a body with `K` that harness's [`Kind`].
macro_rules! harnesses {
    ($d:tt $($variant:ident in $module:ident),+ $(,)?) => {
        $(mod $module;)+

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
}

mod session;
mod store;

use store::Layout;

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

    /// The launch command a kit entry's `given` command runs as. A bare
    /// program (a path or a name, no arguments) gets this harness's stock
    /// arguments, as its stock command would: naming where a program is must
    /// not drop the flags a worker relies on. A command with arguments of its
    /// own is taken as given.
    pub fn command(self, given: &str) -> String {
        each!(self, |K| command::<K>(given))
    }

    /// The line a person runs on a login account's machine to log it in
    /// once, its home being `home`: the harness, told that home.
    pub(crate) fn login_line(self, home: &Path) -> String {
        let Login::Machine { user_dir } = self.login() else { return String::new() };
        format!("{user_dir}={} {}", crate::line::quoted(&home.to_string_lossy()), self.key())
    }

    /// How an account of it is logged in.
    pub(crate) const fn login(self) -> Login {
        each!(self, |K| K::LOGIN)
    }

    /// The file under its user configuration whose being there means it is
    /// logged in.
    pub(crate) fn login_file(self) -> Option<&'static str> {
        each!(self, |K| K::KEPT_LINKS).first().map(|link| link.kept)
    }

    /// Where its sessions keep their files on this host, decided once from
    /// the host user's home `user_home`, the session's `account` and the
    /// directory `kept` this host keeps for the harness: a login account's
    /// own home, where its login is; else its own directory under the host
    /// user's home where it has one, else the kept directory.
    pub fn home(self, user_home: &Path, account: Option<&str>, kept: &Path) -> Home {
        Home::new(match (account, self.login(), each!(self, |K| K::USER_HOME)) {
            (Some(name), Login::Machine { .. }, _) => crate::account::login_home(user_home, name),
            (_, _, Some(dir)) => user_home.join(dir),
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

    /// The shell line of the tool call a hook is handed as `payload`;
    /// `None` when it is no shell call.
    pub fn shell_call(self, payload: &str) -> Option<String> {
        let mut described = serde_json::Deserializer::from_str(payload);
        match each!(self, |K| K::call(&mut described)) {
            Ok(Call::Shell(line)) => Some(line),
            _ => None,
        }
    }

    /// The hook exit code that refuses a call: it is not made, and the
    /// hook's stderr is shown to the session.
    pub const fn refused(self) -> u8 {
        each!(self, |K| K::REFUSED)
    }

    /// own screen up, ready to be typed at: blank, a shell's prompt (its
    /// program not started yet, or exited), or a question it asks before its
    /// screen is up. `None`: it is up.
    pub fn not_up(self, screen: &[String]) -> Option<String> {
        let Some(row) = crate::screen::last_row(screen) else { return Some("its screen is blank".into()) };
        if crate::screen::shell_prompt(row) {
            return Some(format!("its screen is a shell's: {row}"));
        }
        match each!(self, |K| K::ASKS_FIRST) {
            Some(words) if row.contains(words) => Some(format!("it asks before it starts: {row}")),
            _ => None,
        }
    }

    /// What `screen`, the end of a session's terminal, says stopped it.
    pub fn stopped(self, screen: &[String]) -> Option<Stop> {
        each!(self, |K| K::stopped(screen))
    }
}

fn launch<K: Kind>(start: &Start) -> Result<Launch, String> {
    let home = start.home.path();
    let mut kept = vec![Kept::Dir { path: home.to_path_buf() }];
    let links = K::KEPT_LINKS
        .iter()
        .map(|link| (start.user_home.join(link.home), home.join(link.kept)))
        .filter(|(to, _)| to.exists())
        .map(|(to, path)| Kept::Link { path, to });
    kept.extend(links);
    let files = K::kept(start)?;
    kept.extend(files.into_iter().map(|(path, text)| Kept::File { path: home.join(path), text }));
    let mut line = Line::new().fixed("cd ").word(K::start_dir(start)).fixed(" && export");
    for variable in K::ENVIRONMENT {
        line = line.fixed(" ").fixed(variable).fixed("=1");
    }
    let mut line = K::environment(line, start);
    for (name, value) in &start.env {
        line = line.fixed(" ").fixed(name).fixed("=").word(value);
    }
    let line = K::added(line.fixed(" && ").launch(&command::<K>(start.command)), start).done();
    let (line, typed) = K::briefed(line, start.brief);
    Ok(Launch { kept, line, typed })
}

fn command<K: Kind>(given: &str) -> String {
    let given = given.trim();
    if K::STOCK_ARGUMENTS.is_empty() || given.contains(char::is_whitespace) {
        given.to_string()
    } else {
        format!("{given} {}", K::STOCK_ARGUMENTS)
    }
}
