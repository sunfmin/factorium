//! Every place under the data directory, named once: the App puts things
//! there and the scenarios look for them there, both through [`Data`].

use std::path::{Path, PathBuf};

/// The data directory: the one that holds the database.
#[derive(Clone, Debug)]
pub struct Data(PathBuf);

/// The kinds of job of the line and idle work, each run in a work dir of its own
/// ([`crate::workdir`]; docs/adr/0017-the-line-s-jobs-run-in-work-dirs.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineDir {
    Landing,
    Promotion,
    Cut,
    /// Where idle work runs, on the base tip.
    Idle,
}

impl LineDir {
    pub const ALL: [LineDir; 4] = [LineDir::Landing, LineDir::Promotion, LineDir::Cut, LineDir::Idle];

    /// The word its work dirs are named by.
    pub fn name(self) -> &'static str {
        match self {
            LineDir::Landing => "landing",
            LineDir::Promotion => "promotion",
            LineDir::Cut => "cut",
            LineDir::Idle => "idle",
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

    /// Where the session `name` of a worker whose issue names a role starts:
    /// the worker's rules and that role's, and the role's skills.
    pub fn issue_role(&self, name: &str) -> PathBuf {
        self.0.join("role").join("issues").join(name)
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

    /// The home a harness keeps for sessions on the bound login of
    /// `account` (a name stablehand keeps, one word), by the harness's key.
    pub fn kept_on(&self, harness: &str, account: &str) -> PathBuf {
        self.0.join("kept").join(format!("{harness}@{account}"))
    }

    /// Where the jobs of `kind` keep their files, one or more per checkout:
    /// once a clone per checkout each job ran in.
    pub fn line(&self, kind: LineDir) -> PathBuf {
        self.0.join(kind.name())
    }

    /// The build caches the line's jobs keep across their work dirs.
    pub fn caches(&self) -> PathBuf {
        self.0.join("cache")
    }

    /// The build cache of `checkout`'s jobs of `kind`: `CARGO_TARGET_DIR`,
    /// and the dirs its `factorium.toml` declares `cache`, under `dirs/`.
    pub fn cache(&self, kind: LineDir, checkout: &str) -> PathBuf {
        self.caches().join(kind.name()).join(checkout)
    }

    /// The lock a promotion of `checkout` holds while it runs: it names who
    /// holds it.
    pub fn promotion_lock(&self, checkout: &str) -> PathBuf {
        self.line(LineDir::Promotion).join(format!("{checkout}.lock"))
    }

    /// What the idle work of `checkout`'s tasks line printed, last run.
    pub fn idle_tasks(&self, checkout: &str) -> PathBuf {
        self.line(LineDir::Idle).join(format!("{checkout}.tasks"))
    }

    /// Where secrets wait for the start line that reads them.
    pub fn secrets(&self) -> PathBuf {
        self.0.join("secrets")
    }

    /// Where one mutant picker is asked, alone: its question and answer.
    pub fn pick(&self, id: impl std::fmt::Display) -> PathBuf {
        self.0.join("picks").join(id.to_string())
    }

    /// What was seen of `worker` at `at`.
    pub fn evidence(&self, worker: impl std::fmt::Display, at: i64) -> PathBuf {
        self.0.join("evidence").join(format!("{worker}-{at}.txt"))
    }

    /// Where the watch's session is reached when the store cannot be read.
    pub fn watch_reach(&self) -> PathBuf {
        self.0.join("watch-reach.json")
    }

    /// The looks failing on the store now, and when the watch heard of it.
    pub fn outage(&self) -> PathBuf {
        self.0.join("outage.json")
    }

    /// The running daemon's pid.
    pub fn pid(&self) -> PathBuf {
        self.0.join("daemon.pid")
    }

    /// There from the time `stop` unloads the service until `service
    /// install` loads it again; while it is there, no command loads it back.
    pub fn stopped(&self) -> PathBuf {
        self.0.join("service.stopped")
    }

    /// When the running daemon last ticked, ms on its clock: as it started,
    /// then after each tick.
    pub fn ticked(&self) -> PathBuf {
        self.0.join("daemon.tick")
    }

    /// The binary kept to roll back to, as `<version>/bin/<name>` in it.
    pub fn binary(&self) -> PathBuf {
        self.0.join("binary")
    }

    /// What a rollback hands the kept binary it execs.
    pub fn rollback(&self) -> PathBuf {
        self.0.join("rollback.json")
    }

    /// Where binaries are proven: a copy of the database for each, kept
    /// where it failed for a look, and how each was proven.
    pub fn proofs(&self) -> PathBuf {
        self.0.join("proofs")
    }

    /// The whole outputs of `checkout`'s gate runs, one file each, named
    /// so that their names sort oldest first.
    pub fn gate_outputs(&self, checkout: &str) -> PathBuf {
        self.0.join("gates").join(checkout)
    }

    /// What the daemon prints, as the service keeps it.
    pub fn log(&self) -> PathBuf {
        self.0.join("daemon.log")
    }
}
