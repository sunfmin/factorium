//! A shell line the tool writes for a shell to run: the one that starts a
//! session, and those a brief tells a session to run. Whatever differs from one
//! line to the next goes in as a word the shell reads as that text and nothing
//! more — but for the launch command, which is a shell string already
//! and goes in as it was given, once.

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

    /// Adds the launch command `command` as it was given: the tool
    /// never parses it. A line holds it once.
    pub(crate) fn launch(self, command: &str) -> Line {
        let mut text = self.text;
        text.push_str(command);
        Line { text, launched: true }
    }

    /// The line, of one that starts the launch command.
    pub(crate) fn done(self) -> String {
        debug_assert!(self.launched);
        self.text
    }
}

impl Default for Line {
    fn default() -> Line {
        Line::new()
    }
}

/// `text` as one word of a shell line, whatever it holds.
pub(crate) fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
