//! The command line: what the binary parses, which is its manual (SPEC
//! §4), and the one [`App`] call each command makes, with the readings of
//! its arguments. A new command or flag lands here and in the module behind
//! its call; how the process runs is `main.rs`'s (docs/adr/0002).

use std::io::Read;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use factorium::{Answer, App, Caller, DB_ENV, Refusal};
use factorium_domain::issue::Blocker;
use factorium_domain::when;
use factum::Uuid;

/// Runs AI coding agents on one machine while nobody watches: issues from a
/// local tracker go to workers in their own worktrees, green work lands on a
/// proving branch, the proving branch is promoted to the base, versions are
/// cut, and a resident watch deals with what goes wrong. Every answer is JSON.
#[derive(Parser)]
#[command(name = "factorium", version)]
pub struct Cli {
    /// The database. Default: $XDG_DATA_HOME/factorium/factorium.db.
    #[arg(long, global = true, env = DB_ENV)]
    pub db: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Top,
}

#[derive(Subcommand)]
pub enum Top {
    /// Runs the daemon in the foreground until `factorium stop`; refused
    /// while another runs. At start it looks for git, orca and the kit's
    /// harnesses on its PATH; what is missing is a finding. When a cut
    /// installs the formula it runs from, it runs the new binary in its place
    /// between ticks, as the same pid. As a service, launchd runs it.
    Run,
    /// Stops the daemon; run as the service, unloads the service.
    Stop,
    /// The daemon as a launchd agent, kept running and restarted when it exits.
    #[command(subcommand)]
    Service(ServiceCmd),
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
    /// Answers what was passed to a worker's session (`--worker`) or the
    /// watch's, oldest first: who from, the line typed, and how its
    /// delivery went (typed, submitted, queued, brief, not_typed, missed).
    Outbox {
        #[arg(long)]
        worker: Option<Uuid>,
    },
    /// Answers the findings, each with its fix; the reading; what the
    /// watch's reports leave to the owner (`human`); and `asks`: every issue
    /// that waits for a person, oldest first, with its title, its question
    /// (its body's first line after any "Based on …" line), when it started
    /// waiting and who left it so (`watch`, `session`, or `added`
    /// `--waiting-for-human`). Answer one with `issue ready` (do it),
    /// `issue close` (don't), or `issue comment` then `issue ready`; and
    /// `decided`: the Decided records made since `decided --read`.
    Doctor,
    /// Answers the Decided records of the last 7 days, newest first, each
    /// with its number (`id`, D<n>), issue, who, what, options, chose, undo
    /// and when; `decided D17` (or `17`) answers that one record, whatever
    /// its age and whether it was read; `--read` marks every one made until
    /// now read, so doctor's `decided` leaves them out.
    Decided {
        /// One record's number: `D17`, or `17`.
        id: Option<String>,
        #[arg(long, conflicts_with = "id")]
        read: bool,
    },
    /// Removes a worker's worktree that no open issue's attempt owns, as
    /// doctor names it, with its terminals: what it holds that is nowhere
    /// else (commits, and changes, committed first) is kept on
    /// `<branch>-abandoned`, named in a comment on its issue. Refused for
    /// any other path.
    Discard { worktree: PathBuf },
    /// Promotes a checkout's proving branch to its base now: slow gate, then
    /// a fast-forward.
    Promote { checkout: String },
    /// Cuts a version of a checkout now, as its factorium.toml declares.
    Cut { checkout: String },
    /// Runs a checkout's idle work now (SPEC §1.16), as `promote` and `cut`
    /// move the line now: its steps, then its tasks line, in its own clone
    /// at the base's tip, without waiting for an idle host or `every`.
    /// Through the heavy queue at background priority, it yields to nothing
    /// but the hold bound; `unqueue` ends it whole. Recorded as any run is
    /// (its start begins `every` again), its tasks filed by the same rule.
    /// Waits for it and answers its record: tip, at, green, took_ms, tail,
    /// filed. Refused when the base declares no `[idle]`, or while idle work
    /// of any checkout runs or waits on the host (one at a time), naming
    /// it and its `unqueue`.
    Idle { checkout: String },
    /// Takes an entry (its id, as doctor's `heavy` lists it) off the heavy
    /// queue, waiting or running: one that does not move and holds the rest
    /// up. A running one is ended with every process it started. Its
    /// owner, if it still asks, queues again behind the rest.
    Unqueue { entry: Uuid },
    /// The host's heavy queue, for a program of yours that is not a worker.
    #[command(subcommand)]
    Heavy(HeavyCmd),
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
pub enum ServiceCmd {
    /// Writes the agent to ~/Library/LaunchAgents, with this shell's PATH
    /// and this database, and loads it (in place of one loaded before).
    /// Run it from a shell that finds git, orca and the kit's harnesses.
    Install,
    /// Unloads the agent and deletes its file.
    Remove,
    /// Answers the agent's definition as `install` would write it.
    Show,
}

#[derive(Subcommand)]
pub enum CheckoutCmd {
    /// Puts a repo on the list. It needs an `origin` holding the base; a
    /// proving branch origin lacks is made at the base.
    Add {
        name: String,
        path: PathBuf,
        #[arg(long, default_value = "main")]
        base: String,
        #[arg(long, default_value = "proving")]
        proving: String,
        /// Its default role: a directory, by its absolute path, that a
        /// worker of an issue naming no role starts with (an issue's own
        /// role wins).
        #[arg(long, value_name = "DIR")]
        role: Option<String>,
    },
    /// Names a checkout's default role (`--role`), or takes it away
    /// (`--no-role`), for the workers that start from now.
    Edit {
        name: String,
        #[arg(long, value_name = "DIR", conflicts_with = "no_role", required_unless_present = "no_role")]
        role: Option<String>,
        #[arg(long)]
        no_role: bool,
    },
    /// Takes a repo off the list.
    Remove { name: String },
    /// Answers the checkouts on the list.
    List,
}

#[derive(Subcommand)]
pub enum IssueCmd {
    /// Makes an issue, numbered one past the checkout's last. Answers it.
    Add {
        checkout: String,
        title: String,
        #[arg(long)]
        body: Option<String>,
        /// Reads the body from this file; `-` reads stdin.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<PathBuf>,
        /// Waits for a person from the start: no worker takes it until a
        /// person readies it.
        #[arg(long)]
        waiting_for_human: bool,
        /// An issue it waits for, repeat for each: `N` of its checkout;
        /// `<checkout>#N` of another, until it lands; `<checkout>#N@cut`,
        /// until a cut of that checkout holds its landing (and installs it).
        #[arg(long = "blocked-by", value_name = "[CHECKOUT#]N[@cut]")]
        blocked_by: Vec<Blocker>,
        /// No worker takes it before this time: RFC 3339 with its offset
        /// (`2026-11-23T14:34:37+09:00`, or `Z`), or ms since the epoch.
        #[arg(long, value_name = "TIME", value_parser = when::parse)]
        not_before: Option<i64>,
        /// The role its worker plays: a directory, by its absolute path,
        /// whose rules (CLAUDE.md) and skills (.claude/skills) its worker
        /// starts with beside factorium's worker rules. Read, never written.
        #[arg(long, value_name = "DIR")]
        role: Option<String>,
        /// The engine account its worker starts on (mytokens
        /// `agent-account-<NAME>`), on the kit's entry of that account's
        /// harness. While it is used up its issue waits.
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
    },
    /// Answers the open issues, of one checkout or of all.
    List {
        checkout: Option<String>,
        #[arg(long)]
        closed: bool,
    },
    /// Answers one issue, with its comments: what the owner, its workers,
    /// the watch and factorium said about it, each with who and when (ms);
    /// the role and account it names; and `spent`, what it took: each
    /// attempt's time from start to end (a live one's, to now) in ms, and the
    /// tokens its session spent (`null`: its harness's files do not say),
    /// with their sums.
    Show { checkout: String, number: u64 },
    /// Changes an issue's title, body or role.
    Edit {
        checkout: String,
        number: u64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body: Option<String>,
        /// Reads the body from this file; `-` reads stdin.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<PathBuf>,
        /// Names the role its workers play from now, checked as at `issue
        /// add --role`; a live worker keeps the role it started with. The
        /// change is kept as a comment on the issue.
        #[arg(long, value_name = "DIR", conflicts_with = "no_role")]
        role: Option<String>,
        /// Takes its role away: its workers start from now with none.
        #[arg(long)]
        no_role: bool,
    },
    /// Closes an issue; a worker on it ends.
    Close {
        checkout: String,
        number: u64,
        #[arg(long, default_value = "closed by hand")]
        reason: String,
    },
    /// Leaves an issue to a person, for a decision only a person can make:
    /// only `issue ready` moves it again. A worker on it ends, keeping its
    /// worktree.
    WaitForHuman {
        checkout: String,
        number: u64,
        #[arg(long)]
        why: String,
    },
    /// Readies an issue that waits for a person.
    Ready { checkout: String, number: u64 },
    /// Starts an issue over: removes the worktree its last attempt kept, so
    /// the next start begins from proving; the old branch is kept as
    /// `<branch>-abandoned` and named in a comment with the why. Refused
    /// while a worker is on it (leave it to a person first); ready or waiting for a person stays as it is.
    StartOver {
        checkout: String,
        number: u64,
        #[arg(long)]
        why: String,
    },
    /// Adds a comment: it reaches the live worker on the issue at once
    /// (typed at its session, or in its brief not yet handed; a waiting
    /// worker's through its watch episode), and the next worker's brief
    /// carries it.
    Comment { checkout: String, number: u64, text: String },
    /// Makes an issue wait for another, of its checkout or of another, or
    /// until a time (`--undo`: stop). Its worker is parked: once
    /// idle (waiting, or in a wait it declared), back to work as soon as all
    /// it waits for is met.
    #[command(group = clap::ArgGroup::new("what").required(true).args(["by", "until"]))]
    Block {
        checkout: String,
        number: u64,
        /// `N` of its checkout, `<checkout>#N` or `<checkout>#N@cut`, as for `issue add`.
        #[arg(long, alias = "blocked-by", value_name = "[CHECKOUT#]N[@cut]")]
        by: Option<Blocker>,
        /// No worker takes it before this time, as for `issue add
        /// --not-before`; it replaces the time it waited for. With
        /// `--undo`, give no time: it waits for none.
        #[arg(long, value_name = "TIME", num_args = 0..=1, value_parser = when::parse)]
        until: Option<Option<i64>>,
        #[arg(long)]
        undo: bool,
    },
}

#[derive(Subcommand)]
pub enum KitCmd {
    /// Answers the kit.
    Show,
    /// Sets the kit: `harness=command[@model]`, highest priority first.
    /// An unknown harness is refused, naming those there are.
    /// A command that is only a program (a path or a name) gets that
    /// harness's stock arguments; one with arguments is taken as given.
    Set { entries: Vec<String> },
    /// Pins a checkout to one harness (`--harness`), or unpins it.
    Pin {
        checkout: String,
        #[arg(long)]
        harness: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum WorkerCmd {
    /// Runs the repo's fast gate on your worktree, through the queue.
    Gate,
    /// Asks to land your branch. Then stop: you are told if it is red.
    Done,
    /// Hands you to the watch with what you need, kept as a comment on your
    /// issue. Then stop.
    Escalate { why: String },
    /// Adds a comment to your issue, as yours: for records (measurements,
    /// what you found). It changes nothing else and calls no one; to ask
    /// for a decision, escalate.
    Comment {
        /// The comment; without it, it is read from stdin.
        #[arg(long)]
        text: Option<String>,
    },
    /// Says you wait on a long command of your own (a background download),
    /// kept as a comment on your issue: you are not found silent until
    /// `--until` from now (held to 2h). Past it you are, with what you said.
    Waiting {
        /// What you wait for.
        #[arg(long)]
        why: String,
        /// How long: `90s`, `45m`, `2h`; a bare number is minutes.
        #[arg(long, value_parser = duration_ms)]
        until: i64,
    },
    /// Files an issue your issue asks for (a follow-up), as `issue add`
    /// does, on any checkout on the list; your issue keeps the record
    /// ("filed <checkout>#<n>") and the filed one names you. Answers it,
    /// with the blocks it got. On your own checkout it also waits for your
    /// issue to land, unless `--independent`. An issue that needs the
    /// owner's decision is escalated instead.
    File {
        checkout: String,
        title: String,
        /// Reads the body from this file; `-` reads stdin.
        #[arg(long)]
        body_file: PathBuf,
        /// An issue it waits for, repeat for each, as for `issue add`.
        #[arg(long = "blocked-by", value_name = "[CHECKOUT#]N[@cut]")]
        blocked_by: Vec<Blocker>,
        /// No worker takes it before this time, as for `issue add`.
        #[arg(long, value_name = "TIME", value_parser = when::parse)]
        not_before: Option<i64>,
        /// On your own checkout, does not wait for your issue: for a
        /// follow-up that does not need your change.
        #[arg(long)]
        independent: bool,
    },
    /// Has a second agent, a fresh session factorium starts, pick your
    /// change's mutants from slimfit's candidates, given slimfit's goal
    /// (`slimfit help picker`) and your diff. Answers the picks, or why
    /// none, by whom, and the commands that run and record them.
    PickMutants,
    /// Has a second agent, a fresh session factorium starts, judge whether
    /// your change could be smaller, given slimfit's goal (`slimfit help
    /// reviewer`) and your diff. Answers its verdict (`as_is` with why, or
    /// `cut`), by whom, and the command that writes the growth record.
    ReviewGrowth,
    /// Runs a heavy command once the machine has room, after those queued
    /// before it, at a gate's priority. Exits with its exit code.
    Exclusive {
        /// Runs it at background priority instead: for work whose time no
        /// gate compares (a mutation run), so gates get the CPU first.
        #[arg(long)]
        background: bool,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum HeavyCmd {
    /// Runs a heavy command once the machine has room, by the queue's
    /// rule, beside workers' and gates' (a line move past its bound goes
    /// ahead of it, as of a worker's), at a gate's priority. Exits with its
    /// exit code.
    Run {
        /// Runs it at background priority instead: gates get the CPU first.
        #[arg(long)]
        background: bool,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Answers the queue's depth: how many run and wait, and each entry.
    Depth,
}

#[derive(Subcommand)]
pub enum WatchCmd {
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
    /// Sends a waiting worker back to work, told `--text`, kept as a comment
    /// on its issue.
    Restart {
        worker: Uuid,
        #[arg(long, default_value = "The watch looked at what stopped you; go on.")]
        text: String,
        /// First stops its running turn (a hung call), so it reads this now.
        #[arg(long)]
        now: bool,
    },
    /// Types a line at a worker, kept as a comment on its issue; answers
    /// `submitted` (it started a turn) or `queued` (it waits behind the
    /// worker's running turn).
    Tell {
        worker: Uuid,
        text: String,
        /// First stops its running turn (a hung call), so it reads this now.
        #[arg(long)]
        now: bool,
    },
}

impl Top {
    /// Answers the command by its one [`App`] call, as `caller` runs it.
    /// `run` and `guard` are the process's own: `main.rs` answers them.
    pub async fn answer(self, app: &App, caller: &Caller) -> Answer {
        let worker_id = || caller.worker();
        match self {
            Top::Stop => app.stop().await,
            Top::Service(s) => match s {
                ServiceCmd::Install => app.service_install(),
                ServiceCmd::Remove => app.service_remove(),
                ServiceCmd::Show => app.service_show(),
            },
            Top::Checkout(c) => match c {
                CheckoutCmd::Add { name, path, base, proving, role } => {
                    app.checkout_add(&name, &path, &base, &proving, role.as_deref()).await
                }
                CheckoutCmd::Edit { name, role, no_role: _ } => app.checkout_name_role(&name, role.as_deref()).await,
                CheckoutCmd::Remove { name } => app.checkout_remove(&name).await,
                CheckoutCmd::List => app.checkout_list().await,
            },
            Top::Issue(i) => match i {
                IssueCmd::Add {
                    checkout,
                    title,
                    body,
                    body_file,
                    waiting_for_human,
                    mut blocked_by,
                    not_before,
                    role,
                    account,
                } => {
                    blocked_by.extend(not_before.map(Blocker::NotBefore));
                    let body = body_text(body, body_file)?.unwrap_or_default();
                    let on = factorium_domain::issue::On { role, account };
                    app.issue_add_on(&checkout, &title, &body, waiting_for_human, &blocked_by, on).await
                }
                IssueCmd::List { checkout, closed } => app.issue_list(checkout.as_deref(), closed).await,
                IssueCmd::Show { checkout, number } => app.issue_show(&checkout, number).await,
                IssueCmd::Edit { checkout, number, title, body, body_file, role, no_role } => {
                    let body = body_text(body, body_file)?;
                    if role.is_some() || no_role {
                        app.issue_name_role(&checkout, number, role.as_deref()).await?;
                    }
                    app.issue_edit(&checkout, number, title.as_deref(), body.as_deref()).await
                }
                IssueCmd::Close { checkout, number, reason } => app.issue_close(&checkout, number, &reason).await,
                IssueCmd::WaitForHuman { checkout, number, why } => {
                    app.issue_wait_for_human(&checkout, number, &why).await
                }
                IssueCmd::Ready { checkout, number } => app.issue_ready(&checkout, number).await,
                IssueCmd::StartOver { checkout, number, why } => app.issue_start_over(&checkout, number, &why).await,
                IssueCmd::Comment { checkout, number, text } => app.issue_comment(&checkout, number, &text).await,
                IssueCmd::Block { checkout, number, by, until, undo } => {
                    let by = match (by, until) {
                        (Some(by), _) => by,
                        (None, Some(Some(at))) => Blocker::NotBefore(at),
                        (None, _) if undo => Blocker::NotBefore(0),
                        (None, _) => return Err(Refusal::new("no_time", "name the time it waits until")),
                    };
                    app.issue_block(&checkout, number, by, undo).await
                }
            },
            Top::Workers { all } => app.workers(all).await,
            Top::Outbox { worker } => app.outbox_of(worker).await,
            Top::Doctor => app.doctor_here().await,
            Top::Decided { id, read } => app.decided_view(read, id.as_deref()).await,
            Top::Discard { worktree } => app.discard_orphaned(&worktree).await,
            Top::Promote { checkout } => app.promote(&checkout).await,
            Top::Cut { checkout } => app.cut(&checkout).await,
            Top::Idle { checkout } => app.idle_now(&checkout).await,
            Top::Unqueue { entry } => app.unqueue(entry).await,
            Top::Heavy(h) => match h {
                HeavyCmd::Run { background, command } => app.heavy_run(&here()?, &line(&command), background).await,
                HeavyCmd::Depth => app.heavy_depth().await,
            },
            Top::Hold { checkout, why, release } => {
                app.hold(&checkout, if release { None } else { why.as_deref() }).await
            }
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
                WorkerCmd::PickMutants => app.worker_pick_mutants(worker_id()?).await,
                WorkerCmd::ReviewGrowth => app.worker_review_growth(worker_id()?).await,
                WorkerCmd::Escalate { why } => app.worker_escalate(worker_id()?, &why).await,
                WorkerCmd::Comment { text } => {
                    let text = match text {
                        Some(t) => t,
                        None => body_text(None, Some(PathBuf::from("-")))?.unwrap_or_default(),
                    };
                    app.worker_comment(worker_id()?, &text).await
                }
                WorkerCmd::Waiting { why, until } => app.worker_waiting(worker_id()?, &why, until).await,
                WorkerCmd::File { checkout, title, body_file, mut blocked_by, not_before, independent } => {
                    blocked_by.extend(not_before.map(Blocker::NotBefore));
                    let body = body_text(None, Some(body_file))?.unwrap_or_default();
                    app.worker_file(worker_id()?, &checkout, &title, &body, &blocked_by, independent).await
                }
                WorkerCmd::Exclusive { background, command } => {
                    let (dir, line) = (here()?, line(&command));
                    if background {
                        app.worker_exclusive_background(worker_id().ok(), &dir, &line).await
                    } else {
                        app.worker_exclusive(worker_id().ok(), &dir, &line).await
                    }
                }
            },
            Top::Watch(w) => match w {
                WatchCmd::Next => app.watch_next().await,
                WatchCmd::Report { text, human } => app.watch_report(&text, human.as_deref()).await,
                WatchCmd::Restart { worker, text, now } => app.watch_restart(worker, &text, now).await,
                WatchCmd::Tell { worker, text, now } => app.watch_tell(worker, &text, now).await,
            },
            Top::Run | Top::Guard { .. } => unreachable!("main.rs answers it"),
        }
    }
}

/// A duration given as `90s`, `45m` or `2h` (a bare number is minutes), in ms.
fn duration_ms(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let units = [('s', 1_000), ('m', 60_000), ('h', 3_600_000)];
    let (n, per) = units.iter().find_map(|&(u, per)| Some((t.strip_suffix(u)?, per))).unwrap_or((t, 60_000));
    let n: i64 = n.trim().parse().map_err(|_| format!("`{s}` is not a duration such as `90s`, `45m` or `2h`"))?;
    Ok(n.saturating_mul(per))
}

/// The body given by `--body`, or read as UTF-8 text from `--body-file`
/// (`-` is stdin), as is.
fn body_text(body: Option<String>, file: Option<PathBuf>) -> Result<Option<String>, Refusal> {
    let Some(file) = file else { return Ok(body) };
    let bytes = if file.as_os_str() == "-" {
        let mut b = Vec::new();
        std::io::stdin().read_to_end(&mut b).map(|_| b)
    } else {
        std::fs::read(&file)
    };
    let bytes = bytes.map_err(|e| Refusal::new("io", format!("{}: {e}", file.display())))?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| Refusal::new("not_utf8", format!("{} is not UTF-8 text", file.display())))
}

/// The directory this process runs in: where a heavy command run by hand runs.
fn here() -> Result<PathBuf, Refusal> {
    std::env::current_dir().map_err(|e| Refusal::new("io", e.to_string()))
}

/// The line a heavy command run by hand runs: its words as typed.
fn line(command: &[String]) -> String {
    command.iter().map(|w| shell_word(w)).collect::<Vec<_>>().join(" ")
}

/// `s` as one word of the line `worker exclusive` runs: bare where a shell
/// reads it as it is, so the queue shows the command as it was typed.
fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-=:,@+".contains(c)) {
        s.into()
    } else {
        factorium_harness::quoted(s)
    }
}

#[cfg(test)]
mod tests {
    use super::duration_ms;

    #[test]
    fn a_duration_is_read_in_seconds_minutes_or_hours_and_a_bare_number_is_minutes() {
        assert_eq!(duration_ms("90s"), Ok(90_000));
        assert_eq!(duration_ms("45m"), Ok(45 * 60_000));
        assert_eq!(duration_ms("2h"), Ok(2 * 3_600_000));
        assert_eq!(duration_ms("60"), Ok(60 * 60_000));
        assert!(duration_ms("2d").is_err());
        assert!(duration_ms("soon").is_err());
    }
}
