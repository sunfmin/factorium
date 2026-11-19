//! Every place under the data directory, named once: the App puts things
//! there and the scenarios look for them there, both through [`Data`].

use std::path::{Path, PathBuf};

/// The data directory: the one that holds the database.
#[derive(Clone, Debug)]
pub struct Data(PathBuf);

/// The clones the line and idle work run in, one of each per checkout (SPEC §1.12–14, §1.16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineClone {
    Landing,
    Promotion,
    Cut,
    /// Where idle work runs, on the base tip.
    Idle,
}

impl LineClone {
    pub const ALL: [LineClone; 4] = [LineClone::Landing, LineClone::Promotion, LineClone::Cut, LineClone::Idle];

    fn name(self) -> &'static str {
        match self {
            LineClone::Landing => "landing",
            LineClone::Promotion => "promotion",
            LineClone::Cut => "cut",
            LineClone::Idle => "idle",
        }
    }
}

impl Data {
    /// The data directory of the database at `db`.
    pub fn of(db: &Path) -> Data {
        Data(db.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")))
    }

    pub fn root(&self) -> &Path {
        &self.0
    }

    /// The brief the worker session `name` was handed.
    pub fn brief(&self, name: &str) -> PathBuf {
        self.0.join("briefs").join(format!("{name}.md"))
    }

    /// Where every worker session starts: its rules and skills.
    pub fn worker_role(&self) -> PathBuf {
        self.0.join("role").join("worker")
    }

    /// The watch's home.
    pub fn watch(&self) -> PathBuf {
        self.0.join("watch")
    }

    /// The brief the watch's session is handed, in its home.
    pub fn watch_brief(&self) -> PathBuf {
        self.watch().join("BRIEF.md")
    }

    /// The home a harness keeps for its sessions, by its key.
    pub fn kept(&self, harness: &str) -> PathBuf {
        self.0.join("kept").join(harness)
    }

    /// The clones of `kind`, one directory per checkout.
    pub fn clones(&self, kind: LineClone) -> PathBuf {
        self.0.join(kind.name())
    }

    /// The clone of `kind` for `checkout`.
    pub fn clone_of(&self, kind: LineClone, checkout: &str) -> PathBuf {
        self.clones(kind).join(checkout)
    }

    /// The lock a promotion of `checkout` holds while it runs, beside its
    /// clone: it names who holds it.
    pub fn promotion_lock(&self, checkout: &str) -> PathBuf {
        self.clones(LineClone::Promotion).join(format!("{checkout}.lock"))
    }

    /// Where secrets wait for the start line that reads them.
    pub fn secrets(&self) -> PathBuf {
        self.0.join("secrets")
    }

    /// What was seen of `worker` at `at`.
    pub fn evidence(&self, worker: impl std::fmt::Display, at: i64) -> PathBuf {
        self.0.join("evidence").join(format!("{worker}-{at}.txt"))
    }

    /// The running daemon's pid.
    pub fn pid(&self) -> PathBuf {
        self.0.join("daemon.pid")
    }

    /// The binary kept to roll back to, as `<version>/bin/<name>` in it.
    pub fn binary(&self) -> PathBuf {
        self.0.join("binary")
    }

    /// What a rollback hands the kept binary it execs.
    pub fn rollback(&self) -> PathBuf {
        self.0.join("rollback.json")
    }

    /// A copy of the database, made to prove a binary on (`id` its own),
    /// kept where it failed for a look.
    pub fn proof(&self, id: impl std::fmt::Display) -> PathBuf {
        self.0.join("proofs").join(id.to_string()).join("factorium.db")
    }

    /// What the daemon prints, as the service keeps it.
    pub fn log(&self) -> PathBuf {
        self.0.join("daemon.log")
    }
}
