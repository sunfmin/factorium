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
fn scripts(command: &str) -> impl Iterator<Item = &str> {
    let words = command.split(|c: char| c.is_whitespace() || ";&|(){}".contains(c));
    let paths = words.map(|w| w.trim_matches(['"', '\'']).trim_start_matches("./"));
    paths.filter(|w| !w.is_empty() && !w.starts_with(['-', '$']) && w.contains(['/', '.']))
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
    let (mut texts, mut add_to) = (Vec::new(), None);
    for (fast, command) in std::iter::once((true, &gate.fast)).chain(gate.slow.iter().map(|s| (false, s))) {
        texts.push((GATE, command.clone()));
        for (script, text) in scripts(command).filter_map(|s| Some((s, read(s)?))) {
            add_to = add_to.or(fast.then_some(script));
            texts.push((script, text));
        }
    }
    let add_to = add_to.unwrap_or(GATE);
    let configured = FITS.iter().filter(|f| read(f.config).is_some());
    configured
        .map(|f| {
            let found = texts.iter().find(|(_, t)| calls(t, f.call));
            let state = match (found, gate.left_out.get(f.name)) {
                (Some((file, _)), _) => State::Run { r#in: file.to_string() },
                (None, Some(why)) => State::LeftOut { why: why.clone() },
                (None, None) => State::Unrun { add_to: add_to.into() },
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
