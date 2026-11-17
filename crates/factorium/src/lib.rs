//! factorium's one entry. Every CLI command is one method of [`App`], and
//! the daemon's tick is one too; the binary only parses arguments into a
//! call and prints the answer as JSON. Scenarios drive `App` the same way,
//! so what they see is what the CLI prints.

mod daemon;
mod gate;
mod heavy;
mod line;
mod owner;
mod session;
mod start;
mod watch;
mod work;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::{self, Declared};
use factorium_domain::settings::{HostSettings, Settings, settings_id};
use factorium_domain::tracker::TRACKER;
use factorium_domain::{checkout::Checkouts, checkout_id};
use factorium_host::{Git, Host};
use factum::{Aggregate, By, Clock, Command, Engine, Query, StreamId, Submission, Uuid, Value};
use factum_sqlite::SqliteStore;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use daemon::Tick;
pub use factorium_host as host;

/// What a command answers instead of its answer: a code a script can match
/// and a sentence a person can read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

impl Refusal {
    pub fn new(code: &str, message: impl Into<String>) -> Refusal {
        Refusal { code: code.into(), message: message.into() }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl From<factum::Error> for Refusal {
    fn from(e: factum::Error) -> Refusal {
        match e {
            factum::Error::Rejected(r) => Refusal { code: r.code, message: r.message },
            other => Refusal::new("internal", other.to_string()),
        }
    }
}

pub type Answer<T = Value> = Result<T, Refusal>;

fn fail(code: &str) -> impl Fn(String) -> Refusal + '_ {
    move |m| Refusal::new(code, m)
}

/// Where the database lives when nothing says otherwise.
pub fn default_db() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("factorium").join("factorium.db")
}

/// What the daemon remembers between ticks and keeps nowhere else: when a
/// worker went over its share, and which of its heavy jobs wait in the
/// queue and which this tick asked for.
#[derive(Default)]
struct Memory {
    over: std::collections::BTreeMap<Uuid, i64>,
    queued: std::collections::BTreeSet<Uuid>,
    asked: std::collections::BTreeSet<Uuid>,
}

pub struct App {
    engine: Engine<SqliteStore>,
    host: Arc<dyn Host>,
    /// The directory beside the database: briefs, worktrees of the lines,
    /// the watch's home, evidence.
    data: PathBuf,
    by: By,
    memory: Mutex<Memory>,
}

impl App {
    /// Opens the database at `db`, making it if it is not there. `by` names
    /// who runs the commands, as the events record it.
    pub fn open(db: &Path, clock: Arc<dyn Clock>, by: &str, host: Arc<dyn Host>) -> Answer<App> {
        let data = db.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&data)
            .map_err(|e| Refusal::new("no_database", format!("cannot make {}: {e}", data.display())))?;
        let store = SqliteStore::open(db)
            .map_err(|e| Refusal::new("no_database", format!("cannot open {}: {e}", db.display())))?;
        let engine = Engine::new(Arc::new(store), Arc::new(owner::model()), clock, by);
        Ok(App { engine, host, data, by: By::Principal(by.into()), memory: Default::default() })
    }

    fn now(&self) -> i64 {
        self.engine.now().0
    }

    async fn submit<C: Command>(&self, params: impl Serialize) -> Answer<()> {
        let submission = Submission::new(C::NAME, params, Uuid::now_v7(), self.by.clone());
        self.engine.submit(submission).await?;
        Ok(())
    }

    /// The state of the stream `id` of aggregate `A`.
    async fn state<A: Aggregate>(&self, id: Uuid) -> Answer<A::State>
    where
        A::State: DeserializeOwned + Default,
    {
        let state = self.engine.state(&StreamId::new(A::NAME, id)).await?;
        Ok(serde_json::from_value(state).unwrap_or_default())
    }

    async fn rows(&self, query: Query) -> Answer<Vec<Value>> {
        self.engine.settle().await?;
        Ok(self.engine.query(TRACKER, &query).await?.into_iter().map(|(_, row)| row).collect())
    }

    async fn settings(&self) -> Answer<Settings> {
        self.state::<HostSettings>(settings_id()).await
    }

    async fn checkout(&self, name: &str) -> Answer<Checkout> {
        let c = self.state::<Checkouts>(checkout_id(name)).await?;
        if c.listed { Ok(c) } else { Err(Refusal::new("no_checkout", format!("{name} is not on the list"))) }
    }

    /// What the repo declares at `rev`.
    fn declared(&self, git: &Git, rev: &str) -> Answer<Declared> {
        let text = git.run(&["show", &format!("{rev}:{}", declared::FILE)]).ok();
        declared::read(text.as_deref()).map_err(fail("bad_declaration"))
    }

    fn dir(&self, parts: &[&str]) -> PathBuf {
        parts.iter().fold(self.data.clone(), |p, s| p.join(s))
    }
}

/// Why a session may not run the shell `line`, if it may not: a worker runs
/// only `factorium worker …` and never pushes.
pub fn refused(line: &str, worker: bool) -> Option<&'static str> {
    if !worker {
        return None;
    }
    // Each command the line runs: what follows a separator, past variable
    // assignments and the words that run the next one (`env`, `exec`, …).
    for segment in line.split(|c: char| ";&|()\n`".contains(c) || c == '$') {
        let mut words = segment.split_whitespace().peekable();
        while let Some(w) = words.peek() {
            let assigns = w.split_once('=').is_some_and(|(n, _)| !n.is_empty() && !n.starts_with('-'));
            let runs_next = ["env", "exec", "command", "nohup", "time", "nice", "xargs"].contains(w);
            if !(assigns || runs_next || (w.starts_with('-') && segment.trim_start().starts_with("env"))) {
                break;
            }
            words.next();
        }
        let Some(command) = words.next() else { continue };
        if command.rsplit('/').next() == Some("factorium") && words.next() != Some("worker") {
            return Some("a worker runs only `factorium worker …`");
        }
        if command.rsplit('/').next() == Some("git") {
            // The subcommand: past git's own options, `-C dir` and `-c x=y` taking a word.
            while let Some(w) = words.next() {
                if w == "-C" || w == "-c" {
                    words.next();
                } else if !w.starts_with('-') {
                    if w == "push" {
                        return Some("a worker does not push: `factorium worker done` lands your branch");
                    }
                    break;
                }
            }
        }
    }
    None
}
