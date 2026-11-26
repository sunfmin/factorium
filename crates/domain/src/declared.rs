//! What a repo declares about itself, in `factorium.toml` at its root: its
//! gate, its cut and its heavy commands.
//!
//! ```toml
//! [gate]
//! fast = "cargo test"     # run on every landing and by `factorium worker gate`
//! fast_limit = 600        # seconds
//! slow = "./slow.sh"      # what the fast gate lacks, on the proving tip before a promotion
//! slow_limit = 3600
//! regenerated = [".truefit/models/*.json"]  # records the gate rewrites (git globs)
//!
//! [cut]                   # how a version is made from the base branch
//! steps = ["./release.sh $FACTORIUM_VERSION"]
//! formula = "factorium"   # the Homebrew formula to upgrade once cut
//!
//! [idle]                  # run on the base tip only while the host is idle
//! steps = ["./soak.sh"]
//! tasks = "slimfit --version && slimfit next --all"  # filed as issues
//! every = 86400           # at most once per this many seconds
//! limit = 14400           # seconds, for all the steps together
//! ```

use serde::{Deserialize, Serialize};

pub const FILE: &str = "factorium.toml";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declared {
    #[serde(default)]
    pub gate: Gate,
    pub cut: Option<Cut>,
    pub idle: Option<Idle>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    #[serde(default = "nothing")]
    pub fast: String,
    #[serde(default = "fast_limit")]
    pub fast_limit: u64,
    pub slow: Option<String>,
    #[serde(default = "slow_limit")]
    pub slow_limit: u64,
    /// The records the gate regenerates, as git glob pathspecs: a landing
    /// whose only conflicts are among them takes proving's copy of each and
    /// commits them as its fast gate rewrote them on the merge (factorium#225).
    #[serde(default)]
    pub regenerated: Vec<String>,
}

impl Default for Gate {
    fn default() -> Gate {
        Gate {
            fast: nothing(),
            fast_limit: fast_limit(),
            slow: None,
            slow_limit: slow_limit(),
            regenerated: Vec::new(),
        }
    }
}

fn nothing() -> String {
    "true".into()
}

fn fast_limit() -> u64 {
    600
}

fn slow_limit() -> u64 {
    3600
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cut {
    /// Shell lines run in order on the base tip, with `FACTORIUM_VERSION`
    /// set. Any red one fails the cut.
    pub steps: Vec<String>,
    /// The Homebrew formula upgraded once the cut is tagged.
    pub formula: Option<String>,
    #[serde(default = "slow_limit")]
    pub limit: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Idle {
    /// Shell lines run in order on the base tip while the host is idle.
    /// Any red one makes the run red.
    #[serde(default)]
    pub steps: Vec<String>,
    /// A shell line run after them that prints tasks: what read them, then
    /// slimfit's `next --all` answer. Each kind's first is filed as an issue
    /// (`idle::to_file`).
    pub tasks: Option<String>,
    /// At most one run per this many seconds, from the last one's start.
    #[serde(default = "every")]
    pub every: u64,
    /// Seconds all the steps may take together; past it the run is red.
    #[serde(default = "slow_limit")]
    pub limit: u64,
}

fn every() -> u64 {
    86400
}

/// Reads a `factorium.toml`; a repo without one declares the defaults.
pub fn read(text: Option<&str>) -> Result<Declared, String> {
    match text {
        None => Ok(Declared::default()),
        Some(t) => toml::from_str(t).map_err(|e| format!("{FILE}: {e}")),
    }
}

/// Which gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Fast,
    Slow,
}

/// What a gate runs on: a tree a landing or a worker gates, or the proving
/// tip a promotion promotes, as [`Promoting`] says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gating {
    Fast,
    Promotion(Promoting),
}

impl Gating {
    /// The gate whose record it is.
    pub fn kind(&self) -> Kind {
        match self {
            Gating::Fast => Kind::Fast,
            Gating::Promotion(_) => Kind::Slow,
        }
    }
}

/// How a promotion gates the proving tip (SPEC §1.13). Proving moves only by
/// landings, each gated by the fast gate on the very commit it pushes; so a
/// tip a landing gated green needs only what the fast gate lacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Promoting {
    /// The tip is the commit of `by`'s landing, whose fast gate was green:
    /// the slow steps alone, none when the checkout declares none.
    SlowOnly { by: String },
    /// No green landing's record of the tip: the fast gate, then the slow steps.
    Full,
}

impl Promoting {
    /// Which case ran, in the words doctor and the promotion's record use.
    pub fn said(&self) -> String {
        match self {
            Promoting::SlowOnly { by } => format!("slow steps only, tip gated green by {by}'s landing"),
            Promoting::Full => "fast gate and slow steps".into(),
        }
    }
}

impl Gate {
    /// What the slow gate holds that the fast gate lacks: none when `slow`
    /// is absent or is the fast gate's command.
    pub fn slow_steps(&self) -> Option<&str> {
        self.slow.as_deref().filter(|s| s.trim() != self.fast.trim())
    }

    /// The command and limit `gating` runs; `None` when it runs nothing (a
    /// promotion's slow steps alone, of a checkout that declares none).
    pub fn of(&self, gating: &Gating) -> Option<(String, u64)> {
        let fast = (self.fast.clone(), self.fast_limit);
        let slow = self.slow_steps().map(|s| (s.to_string(), self.slow_limit));
        match gating {
            Gating::Fast => Some(fast),
            Gating::Promotion(Promoting::SlowOnly { .. }) => slow,
            // Each in braces of its own, ended by a newline: a line of its own
            // may end in a comment, or join its commands by `;` or `||`.
            Gating::Promotion(Promoting::Full) => Some(match slow {
                None => fast,
                Some((s, l)) => (format!("{{ {}\n}} && {{ {s}\n}}", fast.0), fast.1 + l),
            }),
        }
    }
}
