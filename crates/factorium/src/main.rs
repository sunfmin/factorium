//! The `factorium` binary, the process edge: the command line ([`cli`]) in,
//! the guard hook answered before any database, the caller's permission,
//! the daemon's signals, the one [`App`] call's answer out as JSON. A
//! refusal prints `{"error": {"code", "message"}}` and exits 1.
//!
//! Who runs a command, and what each may run, is [`Caller`]'s to say.

mod cli;

agent_help::binary!("factorium");

use std::io::Read;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cli::{Cli, HeavyCmd, Top, WorkerCmd};
use factorium::host::Machine;
use factorium::{Answer, App, Caller, Refusal, default_db};
use serde_json::json;

async fn run(cli: Cli, caller: Caller) -> Answer {
    let db = cli.db.unwrap_or_else(|| default_db(|name| std::env::var_os(name)));
    let data = factorium::Data::of(&db).root().to_path_buf();
    let app = App::open(&db, caller.by(), Arc::new(Machine::new(data)))?;
    // Whoever runs factorium first after a boot loads the service launchd
    // did not (factorium#287), but the daemon and what ends or starts it.
    let revived = !matches!(cli.command, Top::Run | Top::Stop | Top::Service(_))
        && match app.service_revive() {
            Ok(Some(target)) => {
                eprintln!("factorium: launchd did not hold the service and no daemon ran; loaded {target}");
                true
            }
            Ok(None) => false,
            Err(e) => {
                eprintln!("factorium: the service is installed and not loaded, and loading it failed: {e}");
                false
            }
        };
    // Whoever runs factorium hears that no daemon runs, but the daemon,
    // what ends or starts it, and doctor, which names it as a finding.
    if !revived
        && !matches!(cli.command, Top::Run | Top::Stop | Top::Service(_) | Top::Doctor)
        && let Some(why) = app.daemon_down().await
    {
        eprintln!("factorium: no daemon runs ({why}): nothing lands, starts or is watched; see `factorium doctor`");
    }
    match cli.command {
        Top::Run => app.run(&*stopped_by_signals()?, factorium::handed_lock()).await,
        command => command.answer(&app, &caller).await,
    }
}

/// A flag set when SIGTERM (stop, launchd) or SIGINT arrives, so the daemon
/// ends in order, not mid-write: the handlers are in place before its first
/// tick.
fn stopped_by_signals() -> Answer<Arc<AtomicBool>> {
    use tokio::signal::unix::{SignalKind, signal};
    let fail = |e: std::io::Error| Refusal::new("io", e.to_string());
    let (mut term, mut int) =
        (signal(SignalKind::terminate()).map_err(fail)?, signal(SignalKind::interrupt()).map_err(fail)?);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    tokio::spawn(async move {
        tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    Ok(stop)
}

fn main() -> ExitCode {
    // Every `--help` answers from the manual (`cli::manual`); one that
    // cannot be assembled is refused, as `agent_help::check` refuses it in
    // the tests.
    let command = match cli::manual().clap() {
        Ok(command) => command,
        Err(broken) => {
            println!("{:#}", json!({ "error": Refusal::new("manual", broken.to_string()) }));
            return ExitCode::FAILURE;
        }
    };
    let matches = command.get_matches();
    let cli = cli::parsed(&matches).unwrap_or_else(|e| e.exit());
    if let Top::Guard { harness } = &cli.command {
        // The guard hook: the tool call on stdin, refused with the code its harness reads.
        let mut payload = String::new();
        let _ = std::io::stdin().read_to_string(&mut payload);
        let Err((code, why)) = Caller::here().guard(harness, &payload) else { return ExitCode::SUCCESS };
        eprintln!("factorium refuses this: {why}");
        return ExitCode::from(code);
    }
    let exclusive = matches!(cli.command, Top::Worker(WorkerCmd::Exclusive { .. }) | Top::Heavy(HeavyCmd::Run { .. }));
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a tokio runtime");
    let caller = Caller::here();
    let answer = caller.may_run(matches.subcommand_name().unwrap_or(""));
    match answer.and_then(|()| runtime.block_on(run(cli, caller))) {
        Ok(answer) => {
            println!("{answer:#}");
            if exclusive {
                return ExitCode::from(answer["code"].as_i64().unwrap_or(1).clamp(0, 255) as u8);
            }
            // Doctor that cannot read the store answers that finding, and fails.
            if answer.get("unreadable").is_some() {
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(refusal) => {
            println!("{:#}", json!({ "error": refusal }));
            ExitCode::FAILURE
        }
    }
}
