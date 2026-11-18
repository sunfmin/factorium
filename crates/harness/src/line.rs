//! Every shell line the tool writes for a shell to run: the one that starts a
//! session (its secrets read first from a file), those a brief tells a
//! session to run, the guard hook a session runs before each tool call, the
//! line that logs an account in, and the secrets file itself. Whatever
//! differs from one line to the next goes in as a word the shell reads as
//! that text and nothing more — but for the launch command, which is a shell
//! string already and goes in as it was given, once.

/// A shell line as it is put together. Text reaches it in no other way than
/// those below, so nothing that was read at run time is in it unquoted but
/// the launch command — and that at most once.
#[derive(Debug, Clone)]
pub(crate) struct Line {
    text: String,
    /// Whether the launch command is in it.
    launched: bool,
}

impl Line {
    pub(crate) fn new() -> Line {
        Line { text: String::new(), launched: false }
    }

    /// Adds text that every such line holds: it is written in this tool's
    /// source, so nothing read at run time can be passed for it.
    pub(crate) fn fixed(mut self, text: &'static str) -> Line {
        self.text.push_str(text);
        self
    }

    /// Adds `text` as one word, whatever it holds.
    pub(crate) fn word(mut self, text: &str) -> Line {
        self.text.push_str(&quoted(text));
        self
    }

    /// Adds ` text`: a word of this tool's source, after a space.
    pub(crate) fn arg(self, text: &'static str) -> Line {
        self.fixed(" ").fixed(text)
    }

    /// Adds the flag `name` and `value`, one word.
    pub(crate) fn flag(self, name: &'static str, value: &str) -> Line {
        self.arg(name).fixed(" ").word(value)
    }

    /// Adds the flag `name` and `value`, where there is one.
    pub(crate) fn flag_if(self, name: &'static str, value: Option<&str>) -> Line {
        match value {
            Some(value) => self.flag(name, value),
            None => self,
        }
    }

    /// Adds the variable `name` set to `value`, one word.
    pub(crate) fn set(self, name: &'static str, value: &str) -> Line {
        self.arg(name).fixed("=").word(value)
    }

    /// Adds the number `n`: digits, which a shell reads as they are.
    fn number(mut self, n: u8) -> Line {
        self.text.push_str(&n.to_string());
        self
    }

    /// Adds the launch command `command` as it was given: the tool
    /// never parses it. A line holds it once.
    pub(crate) fn launch(self, command: &str) -> Line {
        debug_assert!(!self.launched);
        let mut text = self.text;
        text.push_str(command);
        Line { text, launched: true }
    }

    /// The line, of one that starts the launch command.
    pub(crate) fn done(self) -> String {
        debug_assert!(self.launched);
        self.text
    }

    /// The line, of one that starts no launch command: every word of it quoted.
    fn quoted_only(self) -> String {
        debug_assert!(!self.launched);
        self.text
    }
}

/// The start `line`, reading its secrets first from `file`, which it removes.
pub(crate) fn secrets_first(file: &str, line: &str) -> String {
    Line::new().fixed(". ").word(file).fixed(" && rm -f ").word(file).fixed(" && ").launch(line).done()
}

/// The text of a secrets file a start line reads first: each of `secrets`
/// exported, its value one word.
pub fn secrets_file(secrets: &[(&'static str, String)]) -> String {
    let lines = secrets.iter().map(|(name, value)| Line::new().fixed("export ").fixed(name).fixed("=").word(value));
    lines.map(|line| line.fixed("\n").quoted_only()).collect()
}

/// The guard hook of a session of the harness `harness`: the factorium
/// `program` asked whether a tool call may be made.
pub(crate) fn guard(program: &str, harness: &'static str) -> String {
    guarded(Line::new(), program, harness).quoted_only()
}

/// The guard hook for a harness whose hooks fail open on an unreadable or
/// failed hook: every such outcome is turned into its refusal code
/// `refused`, with the guard's reason on stderr.
pub(crate) fn guard_asked(program: &str, harness: &'static str, refused: u8) -> String {
    guarded(Line::new().fixed("said=$("), program, harness)
        .fixed(
            " 2>&1 >/dev/null) && exit 0; \
             printf '%s\\n' \"${said:-factorium guard: it could not be asked}\" >&2; exit ",
        )
        .number(refused)
        .quoted_only()
}

fn guarded(line: Line, program: &str, harness: &'static str) -> Line {
    line.word(program).fixed(" guard ").fixed(harness)
}

/// The line a person runs to log an account of the harness `harness` in
/// once: it told, by the variable `user_dir`, that its home is `home`.
pub(crate) fn login(user_dir: &'static str, home: &str, harness: &'static str) -> String {
    Line::new().fixed(user_dir).fixed("=").word(home).fixed(" ").fixed(harness).quoted_only()
}

/// `text` as one word of a shell line, whatever it holds: the one way
/// factorium quotes a word for a shell.
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
