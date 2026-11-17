//! The `factorium` binary: arguments in, one [`App`] call, JSON out. A
//! refusal prints `{"error": {"code", "message"}}` and exits 1.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use factorium::{Answer, App, default_db};
use factum::SystemClock;
use serde_json::json;

/// Runs AI coding agents on one machine while nobody watches.
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
    /// The repos on the list.
    #[command(subcommand)]
    Checkout(CheckoutCmd),
    /// The host's issues.
    #[command(subcommand)]
    Issue(IssueCmd),
}

#[derive(Subcommand)]
enum CheckoutCmd {
    /// Puts a repo on the list. Answers the checkout as recorded.
    Add {
        name: String,
        path: PathBuf,
        /// Where promotions land.
        #[arg(long, default_value = "main")]
        base: String,
        /// Where landings land.
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
        /// Make it parked: no worker takes it until it is readied.
        #[arg(long)]
        parked: bool,
    },
    /// Answers the open issues, of one checkout or of all.
    List {
        checkout: Option<String>,
        /// Answer the closed ones too.
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
    /// Closes an issue.
    Close {
        checkout: String,
        number: u64,
        #[arg(long, default_value = "closed by the owner")]
        reason: String,
    },
    /// Parks an issue: no worker takes it until it is readied.
    Park {
        checkout: String,
        number: u64,
        reason: String,
    },
    /// Readies a parked issue.
    Ready { checkout: String, number: u64 },
    /// Makes an issue wait for another of its checkout.
    Block {
        checkout: String,
        number: u64,
        /// The number of the issue it waits for.
        #[arg(long)]
        by: u64,
        /// Stop waiting for it instead.
        #[arg(long)]
        undo: bool,
    },
}

async fn run(cli: Cli) -> Answer {
    let db = cli.db.unwrap_or_else(default_db);
    let app = App::open(&db, Arc::new(SystemClock), "owner")?;
    match cli.command {
        Top::Checkout(c) => match c {
            CheckoutCmd::Add {
                name,
                path,
                base,
                proving,
            } => app.checkout_add(&name, &path, &base, &proving).await,
            CheckoutCmd::Remove { name } => app.checkout_remove(&name).await,
            CheckoutCmd::List => app.checkout_list().await,
        },
        Top::Issue(i) => match i {
            IssueCmd::Add {
                checkout,
                title,
                body,
                parked,
            } => app.issue_add(&checkout, &title, &body, parked).await,
            IssueCmd::List { checkout, closed } => {
                app.issue_list(checkout.as_deref(), closed).await
            }
            IssueCmd::Show { checkout, number } => app.issue_show(&checkout, number).await,
            IssueCmd::Edit {
                checkout,
                number,
                title,
                body,
            } => {
                app.issue_edit(&checkout, number, title.as_deref(), body.as_deref())
                    .await
            }
            IssueCmd::Close {
                checkout,
                number,
                reason,
            } => app.issue_close(&checkout, number, &reason).await,
            IssueCmd::Park {
                checkout,
                number,
                reason,
            } => app.issue_park(&checkout, number, &reason).await,
            IssueCmd::Ready { checkout, number } => app.issue_ready(&checkout, number).await,
            IssueCmd::Block {
                checkout,
                number,
                by,
                undo,
            } => app.issue_block(&checkout, number, by, undo).await,
        },
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a tokio runtime");
    match runtime.block_on(run(cli)) {
        Ok(answer) => {
            println!("{answer:#}");
            ExitCode::SUCCESS
        }
        Err(refusal) => {
            println!("{:#}", json!({ "error": refusal }));
            ExitCode::FAILURE
        }
    }
}
