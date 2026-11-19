//! factorium's one entry. Every CLI command is one method of [`App`], and
//! the daemon's tick is one too; the binary only parses arguments into a
//! call and prints the answer as JSON. Scenarios drive `App` the same way,
//! so what they see is what the CLI prints.

mod daemon;
mod data;
mod doctor;
mod gate;
mod heavy;
mod idle;
mod issues;
mod kept;
mod life;
mod line;
mod load;
mod owner;
mod service;
mod session;
mod swap;
mod watch;
mod work;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::{self, Declared};
use factorium_domain::settings::{HostSettings, Settings, settings_id};
use factorium_domain::tracker::{Rows, TRACKER};
use factorium_domain::{checkout::Checkouts, checkout_id};
use factorium_host::{Git, Host};
use factum::{Aggregate, By, Command, Engine, Model, StreamId, Submission, Uuid, Value};
use factum_sqlite::SqliteStore;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use daemon::Tick;
pub use data::{Data, LineClone};
pub use factorium_host as host;
pub use session::{Caller, DB_ENV};

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

/// Where the database lives when nothing says otherwise, as `var` reads the
/// environment (`std::env::var_os`): under `XDG_DATA_HOME`, else under
/// `HOME`'s `.local/share`, else in the working directory.
pub fn default_db(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
    let data = var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("factorium").join("factorium.db")
}

/// Every aggregate, command and projection the engine runs: a new domain
/// command is registered here, beside the engine (docs/adr/0001).
fn model() -> Model {
    use factorium_domain::{
        checkout, daemon, gate, health, heavy, idle, issue, kit, line, outbox, settings, tracker, watch, worker,
    };
    Model::new()
        .aggregate::<checkout::Checkouts>()
        .command::<checkout::AddCheckout>()
        .command::<checkout::RemoveCheckout>()
        .aggregate::<issue::Issues>()
        .command::<issue::OpenIssue>()
        .command::<issue::EditIssue>()
        .command::<issue::BlockIssue>()
        .command::<issue::UnblockIssue>()
        .command::<issue::BlockIssueAcross>()
        .command::<issue::UnblockIssueAcross>()
        .command::<issue::WaitForHuman>()
        .command::<issue::ReadyIssue>()
        .command::<issue::CloseIssue>()
        .command::<issue::TakeIssue>()
        .command::<issue::ReleaseIssue>()
        .command::<issue::CommentIssue>()
        .aggregate::<worker::Workers>()
        .command::<worker::StartWorker>()
        .command::<worker::AskLanding>()
        .command::<worker::RedLanding>()
        .command::<worker::GreenLanding>()
        .command::<worker::Wait>()
        .command::<worker::Resume>()
        .command::<worker::Park>()
        .command::<worker::Unpark>()
        .command::<worker::Hear>()
        .command::<worker::EndWorker>()
        .command::<worker::NoteClosed>()
        .command::<health::Observe>()
        .aggregate::<gate::Gates>()
        .command::<gate::RecordGate>()
        .aggregate::<line::Lines>()
        .command::<line::RecordLanding>()
        .command::<line::Promote>()
        .command::<line::RecordCut>()
        .command::<line::HoldLandings>()
        .aggregate::<kit::Kits>()
        .command::<kit::SetTheKit>()
        .command::<kit::PinCheckout>()
        .command::<kit::LimitAccount>()
        .aggregate::<settings::HostSettings>()
        .command::<settings::SetSetting>()
        .aggregate::<idle::Idles>()
        .command::<idle::RecordIdle>()
        .aggregate::<heavy::Heavy>()
        .command::<heavy::EnqueueHeavy>()
        .command::<heavy::StartHeavy>()
        .command::<heavy::EndHeavy>()
        .command::<heavy::NoteRun>()
        .command::<heavy::NoteRules>()
        .aggregate::<daemon::Daemons>()
        .command::<daemon::StartDaemon>()
        .command::<daemon::NoteTick>()
        .aggregate::<watch::Watches>()
        .command::<watch::StartWatch>()
        .command::<watch::EndWatch>()
        .command::<watch::LookNow>()
        .command::<watch::FileReport>()
        .command::<watch::NoteAdmission>()
        .command::<watch::SeeHuman>()
        .aggregate::<outbox::Outboxes>()
        .command::<outbox::GiveMessage>()
        .command::<outbox::TryMessage>()
        .projection(tracker::Tracker)
}

/// What the daemon keeps between ticks and nowhere else: which of its heavy
/// jobs wait in the queue, and which this tick asked for.
#[derive(Default)]
struct Memory {
    queued: std::collections::BTreeSet<Uuid>,
    asked: std::collections::BTreeSet<Uuid>,
    /// The binary this process runs, and the Homebrew link to its formula.
    running: Option<(PathBuf, PathBuf)>,
    /// Each binary (canonical) proven on a copy of the database: opens it, or why not.
    proven: std::collections::BTreeMap<PathBuf, Result<(), String>>,
    /// The idle work running beside the ticks, and its heavy job's run.
    idle: Option<idle::Running>,
    beside: Option<heavy::Running>,
}

/// The host's clock, as the engine reads it.
struct HostClock(Arc<dyn Host>);

impl factum::Clock for HostClock {
    fn now(&self) -> factum::Instant {
        factum::Instant(self.0.clock().now())
    }
}

pub struct App {
    engine: Engine<SqliteStore>,
    /// The host's clock, the app's one: events, waits, idle work's limit, commits.
    clock: Arc<dyn factorium_host::Clock>,
    host: Arc<dyn Host>,
    /// The database it was opened on. Its directory holds the rest: briefs,
    /// worktrees of the lines, the watch's home, evidence.
    db: PathBuf,
    by: By,
    memory: std::sync::Mutex<Memory>,
    /// The heavy queue's rules it asks under: [`factorium_domain::heavy::RULES`], unless a
    /// scenario stands in for another version.
    rules: String,
}

impl App {
    /// Opens the database at `db`, making it if it is not there. `by` names
    /// who runs the commands, as the events record it.
    pub fn open(db: &Path, by: &str, host: Arc<dyn Host>) -> Answer<App> {
        // Absolute: sessions are told it, and they run in other directories.
        let db = std::path::absolute(db)
            .map_err(|e| Refusal::new("no_database", format!("cannot place {}: {e}", db.display())))?;
        let data = db.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&data)
            .map_err(|e| Refusal::new("no_database", format!("cannot make {}: {e}", data.display())))?;
        let store = SqliteStore::open(&db)
            .map_err(|e| Refusal::new("no_database", format!("cannot open {}: {e}", db.display())))?;
        let engine = Engine::new(Arc::new(store), Arc::new(model()), Arc::new(HostClock(host.clone())), by);
        Ok(App {
            engine,
            clock: host.clock(),
            host,
            db,
            by: By::Principal(by.into()),
            memory: Default::default(),
            rules: factorium_domain::heavy::RULES.into(),
        })
    }

    /// The same app, asking the heavy queue under the rules of `version`: a
    /// binary of another version, as a scenario has one.
    pub fn under_rules(mut self, version: &str) -> App {
        self.rules = version.into();
        self
    }

    fn now(&self) -> i64 {
        self.engine.now().0
    }

    async fn submit<C: Command>(&self, params: impl Serialize) -> Answer<()> {
        self.decide::<C>(params).await.map(drop)
    }

    /// Submits command `C`; answers the events it recorded.
    async fn decide<C: Command>(&self, params: impl Serialize) -> Answer<Vec<factum::Event>> {
        let submission = Submission::new(C::NAME, params, Uuid::now_v7(), self.by.clone());
        Ok(self.engine.submit(submission).await?.events)
    }

    /// The state of the stream `id` of aggregate `A`.
    async fn state<A: Aggregate>(&self, id: Uuid) -> Answer<A::State>
    where
        A::State: DeserializeOwned + Default,
    {
        let state = self.engine.state(&StreamId::new(A::NAME, id)).await?;
        Ok(serde_json::from_value(state).unwrap_or_default())
    }

    async fn rows<R: DeserializeOwned>(&self, rows: Rows<R>) -> Answer<Vec<R>> {
        self.engine.settle().await?;
        let found = self.engine.query(TRACKER, &rows.query).await?.into_iter().map(|(_, row)| row).collect();
        Rows::read(found).map_err(|e| Refusal::new("bad_row", e))
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
        let text = git.shown(rev, declared::FILE);
        declared::read(text.as_deref()).map_err(fail("bad_declaration"))
    }

    fn data(&self) -> Data {
        Data::of(&self.db)
    }
}
