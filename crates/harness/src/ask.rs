//! Asking a fresh session of a harness one question, headless: no
//! terminal, no tools, no rules or settings of anyone's, nothing but the
//! question it reads on stdin; and reading what it answered on stdout, with
//! the session's id, model and tokens. Whoever asks says what the question
//! is; nothing here knows why.

use std::path::Path;

use crate::Harness;
use crate::harness::Login;
use crate::line::Line;

/// Files of a user directory, each its path and text.
pub(crate) type Files = Vec<(&'static str, String)>;

/// What a harness that can be asked headless says of it (`Kind::HEADLESS`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Headless {
    /// The flags, after its launch command, that ask one question read from
    /// stdin and answer on stdout, with no tools and none of a host's rules,
    /// settings or servers loaded.
    pub(crate) flags: &'static str,
    /// Its flag for the model.
    pub(crate) model: &'static str,
    /// Its flag that asks the session it names once more, in the same
    /// directory and user directory; `None`: a session asked is not kept.
    pub(crate) again: Option<&'static str>,
    /// What the fresh user directory of a session asked holds beside its
    /// login, for a harness logged in by a file there: path and text.
    pub(crate) files: fn() -> Result<Files, String>,
    /// What its stdout `out` says it answered, or why it did not answer; it
    /// was asked on the kit's `model`, if the kit names one.
    pub(crate) read: fn(out: &str, model: Option<&str>) -> Result<Asked, String>,
    /// The stdout it would print for `asked`: a stand-in's.
    pub(crate) stand_in: fn(asked: &Asked) -> String,
}

/// What a session asked headless answered, and what it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The session's own id.
    pub session: String,
    /// The model that answered, as the harness names it.
    pub model: String,
    /// The answer's text.
    pub text: String,
    /// Tokens read (cached ones among them) and written.
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// One question asked headless.
#[derive(Debug, Clone)]
pub struct Ask<'a> {
    /// The launch command, as the kit holds it.
    pub command: &'a str,
    pub model: Option<&'a str>,
    /// The host user's home and the PATH: the only variables the session
    /// gets, beside its user directory and those its secrets file exports.
    pub home: &'a Path,
    pub path: &'a str,
    /// The session's own user directory, made by `ask_home`, for a harness
    /// logged in by a file there.
    pub user_dir: &'a Path,
    /// The file the question is read from, and the one the answer goes to.
    pub question: &'a str,
    pub answer: &'a str,
    /// The secrets file the line reads first and removes (`secrets_file`).
    pub secrets: Option<&'a str>,
    /// The session asked before, in this directory, that is asked once
    /// more; `None`: a fresh one.
    pub again: Option<&'a str>,
}

impl Harness {
    /// Whether a session of it can be asked headless.
    pub fn asks(self) -> bool {
        self.headless().is_some()
    }

    /// Whether a session of it asked headless can be asked once more.
    pub fn asks_again(self) -> bool {
        self.headless().is_some_and(|h| h.again.is_some())
    }

    /// Makes the empty directory `fresh` the user directory of one session
    /// asked headless, for a harness logged in by a file there: that login
    /// linked — the `account`'s, else the host user's under `user_home` —
    /// and the files it is asked with; nothing else of either home, so no
    /// hook, memory, skill or setting of theirs. A link, so a login renewed
    /// there stays fresh in both places.
    pub fn ask_home(self, user_home: &Path, account: Option<&str>, fresh: &Path) -> Result<(), String> {
        let (Some(h), Login::Machine { .. }) = (self.headless(), self.login()) else { return Ok(()) };
        let link = self.links().first().ok_or_else(|| format!("{} names no login file", self.key()))?;
        let login = match account {
            Some(name) => crate::account::login_home(user_home, name).join(link.kept),
            None => user_home.join(link.home),
        };
        if !login.exists() {
            return Err(format!("{} is not logged in: no {}", self.key(), login.display()));
        }
        let io = |e: std::io::Error| format!("{}: {e}", fresh.display());
        let linked = fresh.join(link.kept);
        linked.parent().map(std::fs::create_dir_all).transpose().map_err(io)?;
        std::os::unix::fs::symlink(&login, linked).map_err(io)?;
        for (file, text) in (h.files)()? {
            let path = fresh.join(file);
            path.parent().map(std::fs::create_dir_all).transpose().map_err(io)?;
            std::fs::write(path, text).map_err(io)?;
        }
        Ok(())
    }

    /// The shell line that asks a fresh session of it `ask.question`, its
    /// answer written to `ask.answer`: in an environment cleared of the
    /// asker's, so nothing of the session that asks reaches it; the session
    /// `ask.again` names, where there is one. `None`: it cannot be asked
    /// headless, or not once more.
    pub fn ask_line(self, ask: &Ask) -> Option<String> {
        let h = self.headless()?;
        let line = Line::new().fixed("exec ").launch(&self.command(ask.command)).fixed(" ").fixed(h.flags);
        let line = match ask.again {
            Some(session) => line.flag(h.again?, session),
            None => line,
        };
        let line = line.flag_if(h.model, ask.model).fixed(" < ").word(ask.question).fixed(" > ").word(ask.answer);
        let inner = match ask.secrets {
            Some(file) => crate::line::secrets_first(file, &line.done()),
            None => line.done(),
        };
        let home = ask.home.to_string_lossy();
        let mut outer = Line::new().fixed("exec env -i").set("HOME", &home).set("PATH", ask.path);
        if let Login::Machine { user_dir } = self.login() {
            outer = outer.set(user_dir, &ask.user_dir.to_string_lossy());
        }
        Some(outer.fixed(" sh -c ").launch(&crate::quoted(&inner)).done())
    }

    /// What a session asked headless on the kit's `model` answered, read
    /// from its stdout `out`; why not, when it did not answer.
    pub fn asked(self, out: &str, model: Option<&str>) -> Result<Asked, String> {
        let h = self.headless().ok_or_else(|| format!("{} is not asked headless", self.key()))?;
        (h.read)(out, model)
    }

    /// The stdout a stand-in for a session of it asked headless prints,
    /// answering `asked`.
    pub fn asked_stand_in(self, asked: &Asked) -> Option<String> {
        self.headless().map(|h| (h.stand_in)(asked))
    }
}

/// No file beside the login.
pub(crate) fn no_files() -> Result<Files, String> {
    Ok(Vec::new())
}
