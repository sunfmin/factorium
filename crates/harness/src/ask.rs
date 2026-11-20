//! Asking a fresh session of a harness one question, headless: no
//! terminal, no tools, no rules or settings of anyone's, nothing but the
//! question it reads on stdin; and reading what it answered on stdout, with
//! the session's id, model and tokens. Whoever asks says what the question
//! is; nothing here knows why.

use std::path::Path;

use crate::Harness;
use crate::line::Line;

/// What a harness that can be asked headless says of it (`Kind::HEADLESS`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Headless {
    /// The flags, after its launch command, that ask one question read from
    /// stdin and answer on stdout, with no tools and none of a host's rules,
    /// settings or servers loaded.
    pub(crate) flags: &'static str,
    /// Its flag for the model.
    pub(crate) model: &'static str,
    /// What its stdout `out` says it answered, or why it did not answer.
    pub(crate) read: fn(out: &str) -> Result<Asked, String>,
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
    /// gets, beside those its secrets file exports.
    pub home: &'a Path,
    pub path: &'a str,
    /// The file the question is read from, and the one the answer goes to.
    pub question: &'a str,
    pub answer: &'a str,
    /// The secrets file the line reads first and removes (`secrets_file`).
    pub secrets: Option<&'a str>,
}

impl Harness {
    /// Whether a session of it can be asked headless.
    pub fn asks(self) -> bool {
        self.headless().is_some()
    }

    /// The shell line that asks a fresh session of it `ask.question`, its
    /// answer written to `ask.answer`: in an environment cleared of the
    /// asker's, so nothing of the session that asks reaches it. `None`: it
    /// cannot be asked headless.
    pub fn ask_line(self, ask: &Ask) -> Option<String> {
        let h = self.headless()?;
        let line = Line::new().fixed("exec ").launch(&self.command(ask.command)).fixed(" ").fixed(h.flags);
        let line = line.flag_if(h.model, ask.model).fixed(" < ").word(ask.question).fixed(" > ").word(ask.answer);
        let inner = match ask.secrets {
            Some(file) => crate::line::secrets_first(file, &line.done()),
            None => line.done(),
        };
        let home = ask.home.to_string_lossy();
        let outer = Line::new().fixed("exec env -i").set("HOME", &home).set("PATH", ask.path).fixed(" sh -c ");
        Some(outer.launch(&crate::quoted(&inner)).done())
    }

    /// What a session asked headless answered, read from its stdout `out`;
    /// why not, when it did not answer.
    pub fn asked(self, out: &str) -> Result<Asked, String> {
        let h = self.headless().ok_or_else(|| format!("{} is not asked headless", self.key()))?;
        (h.read)(out)
    }

    /// The stdout a stand-in for a session of it asked headless prints,
    /// answering `asked`.
    pub fn asked_stand_in(self, asked: &Asked) -> Option<String> {
        self.headless().map(|h| (h.stand_in)(asked))
    }
}
