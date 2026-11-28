//! The fits a repo configures at its root, and whether the gate factorium
//! runs for it calls each one's check: a fit configured and never run checks
//! nothing, a doctor finding until the gate calls it or the repo says why it
//! leaves it out (`[gate.left_out]` in `factorium.toml`).

use serde::{Deserialize, Serialize};

use crate::declared::{self, Declared};
use crate::watch::Found;

/// A fit: its name, its config at the repo root, and the call that checks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    pub name: &'static str,
    pub config: &'static str,
    pub call: &'static str,
}

/// Every fit factorium looks for.
pub const FITS: [Fit; 3] = [
    Fit { name: "slimfit", config: "slimfit.toml", call: "slimfit check" },
    Fit { name: "truefit", config: "truefit.toml", call: "truefit contract" },
    Fit { name: "docfit", config: "docfit.toml", call: "docfit check" },
];

/// Where the gate's own commands are written, as a finding names it.
pub const GATE: &str = "factorium.toml's [gate]";

/// A fit configured in a repo, and how its gate treats it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fitted {
    pub fit: String,
    pub config: String,
    pub call: String,
    #[serde(flatten)]
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// The gate calls it, in this file (or [`GATE`]).
    Run { r#in: String },
    /// The repo leaves it out of its gate, for this reason.
    LeftOut { why: String },
    /// Nothing the gate runs calls it: it belongs in this file.
    Unrun { add_to: String },
}

/// The files a gate command executes: each of its words that names a path
/// (holds a `/` or a `.`), without a leading `./` or quotes.
fn scripts(command: &str) -> Vec<String> {
    let words = command.split(|c: char| c.is_whitespace() || ";&|(){}".contains(c));
    let path = |w: &str| w.trim_matches(|c| c == '"' || c == '\'').trim_start_matches("./").to_string();
    let named = |w: &String| !w.is_empty() && !w.starts_with(['-', '$']) && w.contains(['/', '.']);
    words.map(path).filter(named).collect()
}

/// Whether `text` calls `call` on a line that is not a shell comment.
fn calls(text: &str, call: &str) -> bool {
    text.lines().any(|l| !l.trim_start().starts_with('#') && l.contains(call))
}

/// Each fit whose config `read` finds at the repo root, and whether the gate
/// `declared` holds calls it: its `fast` and `slow` commands, and the files
/// they execute, one level (`read` answers a file of the repo by its path).
pub fn of(declared: &Declared, read: impl Fn(&str) -> Option<String>) -> Vec<Fitted> {
    let gate = &declared.gate;
    let commands: Vec<&String> = std::iter::once(&gate.fast).chain(gate.slow.as_ref()).collect();
    let mut texts: Vec<(String, String)> = Vec::new();
    for command in &commands {
        texts.push((GATE.into(), command.to_string()));
        texts.extend(scripts(command).into_iter().filter_map(|s| Some((s.clone(), read(&s)?))));
    }
    let add_to = scripts(&gate.fast).into_iter().find(|s| texts.iter().any(|(f, _)| f == s));
    let add_to = add_to.unwrap_or_else(|| GATE.into());
    let configured = FITS.iter().filter(|f| read(f.config).is_some());
    configured
        .map(|f| {
            let found = texts.iter().find(|(_, t)| calls(t, f.call));
            let state = match (found, gate.left_out.get(f.name)) {
                (Some((file, _)), _) => State::Run { r#in: file.clone() },
                (None, Some(why)) => State::LeftOut { why: why.clone() },
                (None, None) => State::Unrun { add_to: add_to.clone() },
            };
            Fitted { fit: f.name.into(), config: f.config.into(), call: f.call.into(), state }
        })
        .collect()
}

/// The finding of each fit of `checkout` its gate never runs: doctor's, and
/// `checkout add`'s warning.
pub fn unrun<'a>(checkout: &'a str, fits: &'a [Fitted]) -> impl Iterator<Item = Found> + 'a {
    fits.iter().filter_map(move |f| {
        let State::Unrun { add_to } = &f.state else { return None };
        let (fit, config, call) = (&f.fit, &f.config, &f.call);
        Some(Found {
            key: format!("unrun:{checkout}:{fit}"),
            what: format!("{checkout}: {config} is at its root and its gate never runs `{call}`: {fit} checks nothing"),
            fix: format!(
                "add `{call}` to {add_to} in {checkout}; or, to leave it out on purpose, say why in its {}: \
                 `[gate.left_out]` `{fit} = \"<why>\"`",
                declared::FILE
            ),
            heard: Vec::new(),
        })
    })
}
