//! The `factorium` binary: arguments in, one [`App`] call, JSON out. A
//! refusal prints `{"error": {"code", "message"}}` and exits 1.
//!
//! Who runs a command is known by the environment its session was started
//! with: a worker's holds `FACTORIUM_WORKER`, the watch's `FACTORIUM_WATCH`.
//! A worker may run only `factorium worker …`; only the watch may run
//! `factorium watch …`.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand};
use factorium::host::Machine;
use factorium::{Answer, App, Refusal, default_db};
use factorium_harness::{Call, Harness};
use factum::{SystemClock, Uuid};
use serde_json::json;

/// Runs AI coding agents on one machine while nobody watches: issues from a
/// local tracker go to workers in their own worktrees, green work lands on a
/// proving branch, the proving branch is promoted to the base, versions are
/// cut, and a resident watch deals with what goes wrong. Every answer is JSON.
#[derive(Parser)]
#[command(name = "factorium", version)]
struct Cli {
    /// The database. Default: $XDG_DATA_HOME/factorium/factorium.db.
    #[arg(long, global = true, env = "FACTORIUM_DB")]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Top,
}

#[derive(Subcommand)]
enum Top {
    /// Runs the daemon in the foreground until `factorium stop`.
    /// When a cut installs the formula it runs from, it runs the new binary
    /// in its place between ticks, as the same pid.
    Run,
    /// Stops the daemon.
    Stop,
    /// Runs one pass of the daemon and answers what it did.
    Tick,
    /// The repos on the list.
    #[command(subcommand)]
    Checkout(CheckoutCmd),
    /// The host's issues.
    #[command(subcommand)]
    Issue(IssueCmd),
    /// Answers the live workers (`--all`: the ended ones too).
    Workers {
        #[arg(long)]
        all: bool,
    },
    /// Answers the findings, each with its fix; the reading; and what the
    /// watch's reports leave to the owner.
    Doctor,
    /// Promotes a checkout's proving branch to its base now: slow gate, then
    /// a fast-forward.
    Promote { checkout: String },
    /// Cuts a version of a checkout now, as its factorium.toml declares.
    Cut { checkout: String },
    /// Holds a checkout's landings (`--why`), or releases them (`--release`).
    Hold {
        checkout: String,
        #[arg(long, required_unless_present = "release")]
        why: Option<String>,
        #[arg(long)]
        release: bool,
    },
    /// The kit: the harnesses this host starts, in priority order.
    #[command(subcommand)]
    Kit(KitCmd),
    /// Answers the engine accounts mytokens holds (`agent-account-<name>`),
    /// as this machine can use them, with what each has left.
    Accounts,
    /// Answers the settings; with a name, one; with a value, sets it.
    Settings { name: Option<String>, value: Option<u64> },
    /// A worker's commands.
    #[command(subcommand)]
    Worker(WorkerCmd),
    /// The watch's commands.
    #[command(subcommand)]
    Watch(WatchCmd),
    /// The hook a session runs before each tool call: refuses what its
    /// session may not run.
    #[command(hide = true)]
    Guard { harness: String },
}

#[derive(Subcommand)]
enum CheckoutCmd {
    /// Puts a repo on the list. It needs an `origin` holding the base; a
    /// proving branch origin lacks is made at the base.
    Add {
        name: String,
        path: PathBuf,
        #[arg(long, default_value = "main")]
        base: String,
        #[arg(long, default_value = "proving")]
        proving: String,
    },
    /// Takes a repo off the list.
    Remove { name: String },
    /// Answers the checkouts on the list.
    List,
}

#[derive(Subcommand)]
enum IssueCmd {
    /// Makes an issue, numbered one past the checkout's last. Answers it.
    Add {
        checkout: String,
        title: String,
        #[arg(long, default_value = "")]
        body: String,
        /// Parked: no worker takes it until it is readied.
        #[arg(long)]
        parked: bool,
    },
    /// Answers the open issues, of one checkout or of all.
    List {
        checkout: Option<String>,
        #[arg(long)]
        closed: bool,
    },
    /// Answers one issue.
    Show { checkout: String, number: u64 },
    /// Changes an issue's title or body.
    Edit {
        checkout: String,
        number: u64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body: Option<String>,
    },
    /// Closes an issue; a worker on it ends.
    Close {
        checkout: String,
        number: u64,
        #[arg(long, default_value = "closed by hand")]
        reason: String,
    },
    /// Parks an issue; a worker on it ends, keeping its worktree.
    Park { checkout: String, number: u64, reason: String },
    /// Readies a parked issue.
    Ready { checkout: String, number: u64 },
    /// Adds a comment, which the next worker's brief carries.
    Comment { checkout: String, number: u64, text: String },
    /// Makes an issue wait for another of its checkout (`--undo`: stop).
    Block {
        checkout: String,
        number: u64,
        #[arg(long)]
        by: u64,
        #[arg(long)]
        undo: bool,
    },
}

#[derive(Subcommand)]
enum KitCmd {
    /// Answers the kit.
    Show,
    /// Sets the kit: `harness=command[@model]`, highest priority first.
    /// Harnesses: claude-code, codex, pi, dsh, kimi, grok.
    Set { entries: Vec<String> },
    /// Pins a checkout to one harness (`--harness`), or unpins it.
    Pin {
        checkout: String,
        #[arg(long)]
        harness: Option<String>,
    },
}

#[derive(Subcommand)]
enum WorkerCmd {
    /// Runs the repo's fast gate on your worktree, through the queue.
    Gate,
    /// Asks to land your branch. Then stop: you are told if it is red.
    Done,
    /// Hands you to the watch with what you need. Then stop.
    Escalate { why: String },
    /// Runs a heavy command once the machine has room, after those queued
    /// before it. Exits with its exit code.
    Exclusive {
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
}

#[derive(Subcommand)]
enum WatchCmd {
    /// Answers the round running now and every open episode.
    Next,
    /// Ends the round with what you found and did.
    Report {
        #[arg(long)]
        text: String,
        /// What only the owner can do (spend money).
        #[arg(long)]
        human: Option<String>,
    },
    /// Sends a waiting worker back to work, told `--text`.
    Restart {
        worker: Uuid,
        #[arg(long, default_value = "The watch looked at what stopped you; go on.")]
        text: String,
    },
    /// Types a line at a worker.
    Tell { worker: Uuid, text: String },
    /// Looks now: findings to episodes, a round if none runs.
    Look,
    /// Answers the reports filed, newest last.
    Reports,
}

fn worker_id() -> Answer<Uuid> {
    std::env::var("FACTORIUM_WORKER")
        .ok()
        .and_then(|w| w.parse().ok())
        .ok_or_else(|| Refusal::new("not_a_worker", "only a worker's session runs this: FACTORIUM_WORKER is not set"))
}

/// Refuses a command its caller may not run.
fn guard(command: &Top) -> Answer<()> {
    let worker = std::env::var_os("FACTORIUM_WORKER").is_some();
    let watch = std::env::var_os("FACTORIUM_WATCH").is_some();
    match command {
        Top::Worker(_) | Top::Guard { .. } => Ok(()),
        _ if worker => Err(Refusal::new("guarded", "a worker runs only `factorium worker …`")),
        Top::Watch(_) if !watch => Err(Refusal::new("guarded", "only the watch runs `factorium watch …`")),
        _ => Ok(()),
    }
}

async fn run(cli: Cli) -> Answer {
    guard(&cli.command)?;
    let db = cli.db.unwrap_or_else(default_db);
    let by = if std::env::var_os("FACTORIUM_WORKER").is_some() {
        "worker"
    } else if std::env::var_os("FACTORIUM_WATCH").is_some() {
        "watch"
    } else {
        "owner"
    };
    let data = db.parent().map(PathBuf::from).unwrap_or_default();
    let app = App::open(&db, Arc::new(SystemClock), by, Arc::new(Machine::new(data)))?;
    match cli.command {
        Top::Run => {
            let stop = Arc::new(AtomicBool::new(false));
            app.run(&stop).await
        }
        Top::Stop => app.stop(),
        Top::Tick => app.tick().await.map(|t| json!(t)),
        Top::Checkout(c) => match c {
            CheckoutCmd::Add { name, path, base, proving } => app.checkout_add(&name, &path, &base, &proving).await,
            CheckoutCmd::Remove { name } => app.checkout_remove(&name).await,
            CheckoutCmd::List => app.checkout_list().await,
        },
        Top::Issue(i) => match i {
            IssueCmd::Add { checkout, title, body, parked } => app.issue_add(&checkout, &title, &body, parked).await,
            IssueCmd::List { checkout, closed } => app.issue_list(checkout.as_deref(), closed).await,
            IssueCmd::Show { checkout, number } => app.issue_show(&checkout, number).await,
            IssueCmd::Edit { checkout, number, title, body } => {
                app.issue_edit(&checkout, number, title.as_deref(), body.as_deref()).await
            }
            IssueCmd::Close { checkout, number, reason } => app.issue_close(&checkout, number, &reason).await,
            IssueCmd::Park { checkout, number, reason } => app.issue_park(&checkout, number, &reason).await,
            IssueCmd::Ready { checkout, number } => app.issue_ready(&checkout, number).await,
            IssueCmd::Comment { checkout, number, text } => app.issue_comment(&checkout, number, &text).await,
            IssueCmd::Block { checkout, number, by, undo } => app.issue_block(&checkout, number, by, undo).await,
        },
        Top::Workers { all } => app.workers(all).await,
        Top::Doctor => app.doctor().await,
        Top::Promote { checkout } => app.promote(&checkout).await,
        Top::Cut { checkout } => app.cut(&checkout).await,
        Top::Hold { checkout, why, release } => app.hold(&checkout, if release { None } else { why.as_deref() }).await,
        Top::Kit(k) => match k {
            KitCmd::Show => app.kit().await,
            KitCmd::Set { entries } => app.kit_set(&entries).await,
            KitCmd::Pin { checkout, harness } => app.kit_pin(&checkout, harness.as_deref()).await,
        },
        Top::Accounts => app.accounts().await,
        Top::Settings { name, value } => app.settings_set(name.as_deref(), value).await,
        Top::Worker(w) => match w {
            WorkerCmd::Gate => app.worker_gate(worker_id()?).await,
            WorkerCmd::Done => app.worker_done(worker_id()?).await,
            WorkerCmd::Escalate { why } => app.worker_escalate(worker_id()?, &why).await,
            WorkerCmd::Exclusive { command } => {
                let dir = std::env::current_dir().map_err(|e| Refusal::new("io", e.to_string()))?;
                let line = command.iter().map(|w| shell_word(w)).collect::<Vec<_>>().join(" ");
                app.worker_exclusive(worker_id().ok(), &dir, &line).await
            }
        },
        Top::Watch(w) => match w {
            WatchCmd::Next => app.watch_next().await,
            WatchCmd::Report { text, human } => app.watch_report(&text, human.as_deref()).await,
            WatchCmd::Restart { worker, text } => app.watch_restart(worker, &text).await,
            WatchCmd::Tell { worker, text } => app.watch_tell(worker, &text).await,
            WatchCmd::Look => app.look_now().await,
            WatchCmd::Reports => app.watch_reports().await,
        },
        Top::Guard { .. } => unreachable!("answered before the database opens"),
    }
}

fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-=:,@+".contains(c)) {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The guard hook: what a worker's session may not run, refused.
fn hook(harness: &str) -> ExitCode {
    let Some(h) = Harness::read(harness) else { return ExitCode::SUCCESS };
    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);
    let mut de = serde_json::Deserializer::from_str(&payload);
    let Ok(Call::Shell(line)) = h.call(&mut de) else { return ExitCode::SUCCESS };
    let worker = std::env::var_os("FACTORIUM_WORKER").is_some();
    if let Some(why) = factorium::refused(&line, worker) {
        eprintln!("factorium refuses this: {why}");
        return ExitCode::from(h.refused());
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Top::Guard { harness } = &cli.command {
        return hook(harness);
    }
    let exclusive = matches!(cli.command, Top::Worker(WorkerCmd::Exclusive { .. }));
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a tokio runtime");
    match runtime.block_on(run(cli)) {
        Ok(answer) => {
            println!("{answer:#}");
            if exclusive {
                return ExitCode::from(answer["code"].as_i64().unwrap_or(1).clamp(0, 255) as u8);
            }
            ExitCode::SUCCESS
        }
        Err(refusal) => {
            println!("{:#}", json!({ "error": refusal }));
            ExitCode::FAILURE
        }
    }
}
