//! factorium's one entry. Every CLI command is one method of [`App`], and
//! the daemon's tick is one too; the binary only parses arguments into a
//! call and prints the answer as JSON. Scenarios drive `App` the same way,
//! so what they see is what the CLI prints.

mod accounts;
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
mod lock;
mod moved;
mod needs;
mod outage;
mod owner;
mod pick;
mod service;
mod session;
mod swap;
mod upgrade;
mod watch;
mod work;
mod workdir;

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

pub use daemon::{Tick, handed_lock};
pub use data::{Data, LineDir};
pub use factorium_host as host;
pub use heavy::Exclusive;
pub use session::{Caller, DB_ENV};
pub(crate) use work::{Live, Want};

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
            // The store failed to read or write, or what it holds does not
            // fold: the daemon's watch hears of it outside the store.
            other => Refusal::new("store", other.to_string()),
        }
    }
}

pub type Answer<T = Value> = Result<T, Refusal>;

/// The state `state` of `stream`: `null`, a stream with no events yet, is
/// its default; one that does not read as `S` is refused (`bad_state`),
/// never answered as new.
fn read_state<S: DeserializeOwned + Default>(stream: &StreamId, state: Value) -> Answer<S> {
    if state.is_null() {
        return Ok(S::default());
    }
    serde_json::from_value(state).map_err(|e| {
        Refusal::new("bad_state", format!("the {} state of {stream} does not read: {e}", stream.aggregate))
    })
}

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
        checkout, daemon, decided, gate, health, heavy, idle, issue, kit, line, outbox, settings, stack, tracker,
        watch, worker,
    };
    Model::new()
        .aggregate::<checkout::Checkouts>()
        .command::<checkout::AddCheckout>()
        .command::<checkout::RemoveCheckout>()
        .command::<checkout::NameCheckoutRole>()
        .command::<checkout::TurnCheckoutIdle>()
        .aggregate::<issue::Issues>()
        .command::<issue::OpenIssue>()
        .command::<issue::EditIssue>()
        .command::<issue::NameIssueRole>()
        .command::<issue::MarkUrgent>()
        .command::<issue::BlockIssue>()
        .command::<issue::WaitForHuman>()
        .command::<issue::ReadyIssue>()
        .command::<issue::CloseIssue>()
        .command::<issue::TakeIssue>()
        .command::<issue::ReleaseIssue>()
        .command::<issue::ClaimIssue>()
        .command::<issue::UnclaimIssue>()
        .command::<issue::CommentIssue>()
        .command::<issue::NumberDecided>()
        .aggregate::<decided::Numbers>()
        .command::<decided::GiveNumbers>()
        .command::<decided::PlaceNumbers>()
        .aggregate::<worker::Workers>()
        .command::<worker::StartWorker>()
        .command::<worker::AskLanding>()
        .command::<worker::AskFinish>()
        .command::<worker::RedLanding>()
        .command::<worker::GreenLanding>()
        .command::<worker::Wait>()
        .command::<worker::Resume>()
        .command::<worker::Park>()
        .command::<worker::NoteMergeChecked>()
        .command::<worker::Unpark>()
        .command::<worker::EndWorker>()
        .command::<worker::NoteClosed>()
        .command::<worker::NoteSpent>()
        .command::<worker::NoteRoleTold>()
        .command::<worker::DeclareWait>()
        .command::<health::Observe>()
        .aggregate::<gate::Gates>()
        .command::<gate::RecordGate>()
        .aggregate::<line::Lines>()
        .command::<line::RecordLanding>()
        .command::<line::Promote>()
        .command::<line::RecordCut>()
        .command::<line::HoldLandings>()
        .command::<line::AwaitMutants>()
        .aggregate::<stack::Stacks>()
        .command::<stack::StackLanding>()
        .command::<stack::GreenLayer>()
        .command::<stack::RedLayer>()
        .command::<stack::Unstack>()
        .command::<stack::LandStack>()
        .command::<stack::ClearStack>()
        .aggregate::<kit::Kits>()
        .command::<kit::MoveCommands>()
        .command::<kit::PinCheckout>()
        .command::<kit::LimitHarness>()
        .aggregate::<settings::HostSettings>()
        .command::<settings::SetSetting>()
        .command::<settings::DeclareResource>()
        .aggregate::<idle::Idles>()
        .command::<idle::RecordIdle>()
        .aggregate::<heavy::Heavy>()
        .command::<heavy::EnqueueHeavy>()
        .command::<heavy::StartHeavy>()
        .command::<heavy::EndHeavy>()
        .command::<heavy::NoteRun>()
        .command::<heavy::NoteRules>()
        .command::<heavy::NoteHalt>()
        .aggregate::<daemon::Daemons>()
        .command::<daemon::StartDaemon>()
        .command::<daemon::NoteTick>()
        .command::<daemon::NoteRollback>()
        .command::<daemon::NoteSwap>()
        .command::<daemon::NoteUpgrade>()
        .command::<daemon::NoteRole>()
        .aggregate::<watch::Watches>()
        .command::<watch::StartWatch>()
        .command::<watch::EndWatch>()
        .command::<watch::LookNow>()
        .command::<watch::FileReport>()
        .command::<watch::NoteAdmission>()
        .command::<watch::SeeHuman>()
        .command::<watch::MarkDecidedRead>()
        .aggregate::<outbox::Outboxes>()
        .command::<outbox::GiveMessage>()
        .command::<outbox::TryMessage>()
        .command::<outbox::HoldTakenMessages>()
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
    /// The binary this process runs (canonical), noted before an upgrade could move what its path names.
    exe: Option<PathBuf>,
    /// The binary a rollback went back from: a link that names it is not swapped to.
    refused: Option<PathBuf>,
    /// Whether a tick of this process went well; how many failed before one did.
    proven: bool,
    failed: u64,
    /// The kept binary to exec in place of this one, whose ticks all failed.
    fallback: Option<PathBuf>,
    /// The heavy jobs running beside the ticks: idle work, one at a time, and
    /// line moves, one of a checkout at a time.
    beside: Vec<heavy::Beside>,
    /// The work dirs this process made that a job asked for since the last sweep
    /// ([`workdir`]): the sweep leaves them.
    work_dirs: std::collections::BTreeSet<PathBuf>,
    /// Whether this app ticks (the daemon's): the work dirs it makes are asked for tick by tick,
    /// not held until their job ends.
    ticks: bool,
    /// The cuts whose installed binary is proven beside the ticks, recorded once it ends.
    proving: Vec<line::Proving>,
    /// Each landing waiting its turn that merged onto the proving tip: that tip and its
    /// branch head then, not checked again until one of them moves (factorium#307). An entry
    /// is never wrong, so none is taken out: one per worker that waited, in this process.
    merges: std::collections::BTreeMap<Uuid, (String, String)>,
    /// Each checkout's proving tip slimfit found owes no record of its mutants: a promotion
    /// waiting its turn does not ask again of the same tip.
    owes_none: std::collections::BTreeMap<String, String>,
    /// The sessions whose messages are typed beside the ticks (factorium#311).
    typing: Vec<session::Typing>,
    /// The upgrade of the daemon's own formula from the tap, running beside the ticks.
    upgrading: Option<tokio::task::JoinHandle<Result<String, String>>>,
    /// The linked binary not swapped to, as its proof failed, or none; for the tick to record.
    refusal: Option<Option<factorium_domain::daemon::Refused>>,
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
    proofs: Arc<factorium_host::proof::Proofs>,
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
            proofs: Default::default(),
            rules: factorium_domain::heavy::RULES.into(),
        })
    }

    /// An app of its own over this one's store and host, its commands
    /// recorded as this one's: for work beside the ticks that writes.
    fn alike(&self) -> Answer<App> {
        let By::Principal(by) = &self.by else { unreachable!("an app is opened by a principal") };
        Ok(App { rules: self.rules.clone(), ..App::open(&self.db, by, self.host.clone())? })
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
        let submission = Submission::new(C::NAME, params, Uuid::now_v7(), self.by.clone())?;
        Ok(self.engine.submit(submission).await?.events)
    }

    /// The state of the stream `id` of aggregate `A`, as [`read_state`]
    /// reads it. A stored snapshot that does not read fails factum's fold
    /// first: refused as `store`, which the watch hears of.
    async fn state<A: Aggregate>(&self, id: Uuid) -> Answer<A::State>
    where
        A::State: DeserializeOwned + Default,
    {
        let stream = StreamId::new(A::NAME, id);
        read_state(&stream, self.engine.state(&stream).await?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use factorium_domain::checkout::Checkout;

    fn stream() -> StreamId {
        StreamId::new("checkout", Uuid::nil())
    }

    #[test]
    fn a_state_of_no_events_yet_reads_as_its_default() {
        assert_eq!(read_state::<Checkout>(&stream(), Value::Null), Ok(Checkout::default()));
    }

    #[test]
    fn a_state_that_does_not_read_is_refused_naming_its_aggregate_stream_and_why() {
        let refused = read_state::<Checkout>(&stream(), serde_json::json!({ "name": 5 })).unwrap_err();
        assert_eq!(refused.code, "bad_state");
        let m = &refused.message;
        assert!(m.contains("checkout state") && m.contains(&stream().to_string()) && m.contains("integer `5`"), "{m}");
    }

    #[test]
    fn a_state_that_reads_is_answered() {
        let listed = Checkout { name: "web".into(), listed: true, ..Checkout::default() };
        let state = serde_json::to_value(&listed).unwrap();
        assert_eq!(read_state::<Checkout>(&stream(), state), Ok(listed));
    }
}
