//! The command line: what the binary parses, which is its manual
//! (docs/adr/0026), assembled by agent-help from the doc comments here ([`manual`]), and the one [`App`] call each command makes, with the readings of
//! its arguments. A new command or flag lands here and in the module behind
//! its call; how the process runs is `main.rs`'s (docs/adr/0002).

use std::io::Read;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use factorium::{Answer, App, Caller, DB_ENV, Refusal};
use factorium_domain::issue::{Blocker, Wait};
use factorium_domain::wait::Until;
use factorium_domain::when::{self, Since};
use factum::Uuid;

/// Who runs a command, its group in the manual: the person who owns the
/// host runs every command but `worker`, `watch` and the hook's `guard`.
const OWNER: &str = "Run by the owner";

/// The manual: clap's command tree, each command's text read from its doc
/// comment as written, a group's (`issue`) ended by its [`commands`].
pub fn manual() -> agent_help::Help {
    let cli = Cli::command();
    let name = cli.get_name().to_owned();
    let groups = cli.get_subcommands().map(|c| c.get_name().to_owned()).collect::<Vec<_>>();
    let cli = groups.iter().fold(cli, |cli, group| cli.mut_subcommand(group, |c| commands(c, &name)));
    agent_help::Help::new(cli)
}

/// `command`, under `parent`, with each group in it, itself among them,
/// its text ended by a `## Commands` list of its visible subcommands, each
/// with its summary, the line the top level's index gives it: agent-help
/// lists only the top level's, so `issue --help` would name none of its own.
fn commands(command: clap::Command, parent: &str) -> clap::Command {
    let title = format!("{parent} {}", command.get_name());
    let subs = command.get_subcommands().map(|c| c.get_name().to_owned()).collect::<Vec<_>>();
    let mut list = String::new();
    for sub in command.get_subcommands().filter(|c| !c.is_hide_set()) {
        let about = sub.get_about().or(sub.get_long_about()).map(|a| a.to_string()).unwrap_or_default();
        let summary = about.lines().next().unwrap_or_default().trim().to_owned();
        list.push_str(&format!("- `{title} {}`: {summary}\n", sub.get_name()));
    }
    let mut command = subs.iter().fold(command, |c, sub| c.mut_subcommand(sub, |s| commands(s, &title)));
    if !list.is_empty() {
        let text = command.get_long_about().or(command.get_about()).map(|a| a.to_string()).unwrap_or_default();
        command = command.long_about(format!("{}\n\n## Commands\n\n{list}", text.trim_end()));
    }
    command
}

/// Runs AI coding agents on one machine while nobody watches: issues from a local tracker go to workers in their own worktrees, green work lands on a proving branch, the proving branch is promoted to the base, versions are cut, and a resident watch deals with what goes wrong. Every answer is JSON.
#[derive(Parser)]
#[command(name = "factorium", version, verbatim_doc_comment)]
pub struct Cli {
    /// The database. Default: $XDG_DATA_HOME/factorium/factorium.db.
    #[arg(long, global = true, env = DB_ENV, verbatim_doc_comment)]
    pub db: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Top,
}

#[derive(Subcommand)]
pub enum Top {
    /// Runs the daemon in the foreground until `factorium stop`; refused while another runs. At start it moves the kit's commands to stablehand, once, and looks for git, orca and the programs of the commands stablehand keeps on its PATH; what is missing is a finding. A failing tick is recorded, and `tick_failures_max` of them in a row is a finding; the watch still looks when a tick fails. When a cut installs the formula it runs from, it runs the new binary in its place between ticks, as the same pid. As a service, launchd runs it.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Run,
    /// Stops the daemon; run as the service, unloads the service, and no command loads it back until `service install`.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Stop,
    /// The daemon as a launchd agent, kept running and restarted when it exits.
    #[command(subcommand, next_help_heading = OWNER, verbatim_doc_comment)]
    Service(ServiceCmd),
    /// The repos on the list.
    #[command(subcommand, next_help_heading = OWNER, verbatim_doc_comment)]
    Checkout(CheckoutCmd),
    /// The host's issues.
    #[command(subcommand, next_help_heading = OWNER, verbatim_doc_comment)]
    Issue(IssueCmd),
    /// Answers `{"workers": [...]}`, always an object, whatever the number of workers: the live workers (`--all`: the ended ones too), each with its `worker` id, the `checkout` and issue `number` it works, its `status` (`working`, `landing`, `finishing`, `waiting`, `ended`), `urgent`, its issue's mark, and the resources it `holds` and `waits_for`: each one's rule, who holds it, since, until, and who waits in line, in the order they start, each with its declared hold, its window, `urgent` when its issue is, and why it is not next (`behind `hold of FMGG`, a 5 min hold`, `waits for its window from 22:00`, `behind `fast gate of web#2`, an urgent issue's`). doctor's `resources` shows the same of every resource.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Workers {
        #[arg(long)]
        all: bool,
    },
    /// Answers what was passed to a worker's session (`--worker`) or the watch's, oldest first: who from, the line typed, and how its delivery went: `typed` (a brief), `submitted` (it started a turn), `queued` (it waits behind the running turn; found taken in the session file on a later look, it was submitted), `brief` (written into a brief still to be typed), `not_typed` (the harness was not up, the terminal gone, Orca refused), or `missed` (typed, it started no turn and none runs: a dialog took it; or still unread 10 min after it was given).
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Outbox {
        #[arg(long)]
        worker: Option<Uuid>,
    },
    /// Answers the findings, each with its fix; the reading; what the watch's reports leave to the owner (`human`); and `asks`: every issue that waits for a person, oldest first, with its title, its question (its body's first line after any "Based on …" line), when it started waiting and who left it so (`watch`, `session`, or `added` `--waiting-for-human`). Answer one with `issue ready` (do it), `issue close` (don't), or `issue comment` then `issue ready`; `decided`: the Decided records made since `decided --read`; and `mutants`: each promotion that waits for its mutants issue (where slimfit holds the record of mutants at the base), with the issue and when the wait ends (`until`, `utc`, `left_s`), `mutants_bound` after its filing; `landing`: each landing waiting, with its place in its checkout's line (the urgent first, then each as it asked) and `stack`: where it is in its checkout's stack (`depth`, 1 merged onto the proving tip; `on`, the commit it merged onto; `merge`; and its `gate`'s state), none while it waits to be stacked. A checkout's landings stack, each merged onto the one below and their fast gates run at once as the heavy queue has room; proving moves to the deepest merge whose gate and every gate below it are green. Its promotion and cut run apart, each one at a time: its landings go on while its promotion's gate or its cut runs, as the three write different refs (proving, the base, a tag) from work dirs of their own, each push requiring the tip it read.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Doctor,
    /// Answers the Decided records of the last 7 days, newest first, each with its number (`id`, D<n>), issue, who, what, options, chose, undo and when; `decided D17` (or `17`) answers that one record, whatever its age and whether it was read; `--read` marks every one made until now read, so doctor's `decided` leaves them out.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Decided {
        /// One record's number: `D17`, or `17`.
        #[arg(verbatim_doc_comment)]
        id: Option<String>,
        #[arg(long, conflicts_with = "id")]
        read: bool,
    },
    /// Answers where a checkout's issues spent their time, read from each attempt's steps (`issue show --steps`): all its issues, open and closed, or one (`12`), or a range (`198..222`); with `--since`, only each attempt's time from then on, one that ended before it left out. Each attempt's time is cut into parts that do not overlap, summing to its time: a moment two steps share goes to the first of waiting for a landing, a heavy command running, one queued, a park, a declared wait; the rest is the session's own (the engine and its tools). It answers `checkout`, `since`; `totals`: `issues`, `attempts`, `ms` (the attempts' time), `tokens` (`in`, `out`, summed over attempts whose harness says; all of an attempt's, whatever `--since`), `first_start`, `last_landing` and `wall_ms` between them (null: nothing landed); `split`: `heavy_queued` and `heavy_ran` (`ms`, and `by` command: `gate`, `exclusive`, `hold`), `landing` (`ms`, `green`, `red` with `ms` and `by` kind: `no_merge`, `gate_red`, `past_limit`, `regenerated`, `unknown` for one before its kind was kept; `other`: one that ended otherwise, or still lands), `waits` and `parks` (`ms`, `by` why), `rest`; `reds`: `count`, `by_kind`, and `files` that conflicted (`file`, `landings` it broke), most first; `landings`: how fast its issues landed, `issues_landed` (those with a landing that landed), `reds` and `reds_by_kind`, `to_land_median_ms` and `to_land_max_ms` (a landed issue's `split.landing.green`, its time from `done` to its landing; null: none landed), `heavy_queued_ms` (`split.heavy_queued.ms`), `issue_median_ms` (the median of the issues' `ms`; of an even count, the mean of the middle two); and `issues`: one row each, `number`, `title`, `attempts`, `ms`, `tokens`, and its own `split` and `reds`. All times are ms. With no checkout, it answers every checkout: `since`, and `checkouts`, a row each, its `checkout` and the fields of its `landings`, the same numbers its own `spent` gives for that `--since`.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Spent {
        /// The checkout; none: every checkout, its `landings` alone.
        checkout: Option<String>,
        /// One issue's number (`12`), or a range of them, both ends in (`198..222`).
        #[arg(value_name = "N|FROM..TO", value_parser = numbers, requires = "checkout", verbatim_doc_comment)]
        numbers: Option<(u64, u64)>,
        /// Only the time from then: RFC 3339 with its offset (`2026-11-25T00:00:00+09:00`, or `Z`), ms since the epoch, or a span back from now (`24h`, `90m`, `30s`).
        #[arg(long, value_name = "TIME|SPAN", value_parser = Since::parse, verbatim_doc_comment)]
        since: Option<Since>,
    },
    /// Removes a worker's worktree that no open issue's attempt owns, as doctor names it, with its terminals: what it holds that is nowhere else (commits, and changes, committed first) is kept on `<branch>-abandoned`, named in a comment on its issue. Refused for any other path.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Discard { worktree: PathBuf },
    /// Promotes a checkout's proving branch to its base now: its gate, then a fast-forward. A tip a green landing pushed runs the slow steps alone (none when `slow` is absent or is `fast`); any other, the fast gate then the slow steps. Answers which ran in `gated`. Whatever the bound or a red tip says; but while a promotion of that checkout runs, the daemon's or another `promote`, it is refused, its answer naming the running one, which is left be.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Promote { checkout: String },
    /// Cuts a version of a checkout now, as its factorium.toml declares. A checkout has one cut at a time: while one of it, the daemon's or another by hand, waits in the heavy queue or runs, it cuts nothing and answers `cut` null and `why` naming who holds it and its pid; doctor's heavy shows it as `cut of <checkout>`. Once that cut is recorded, run it again if a cut is still owed.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Cut { checkout: String },
    /// Runs a checkout's idle work now (docs/adr/0014-idle-work.md), as `promote` and `cut` move the line now: its steps, then its tasks line, in its own clone at the base's tip, without waiting for an idle host or `every`. Through the heavy queue at background priority, it yields to nothing but the hold bound; `unqueue` ends it whole. Recorded as any run is (its start begins `every` again), its tasks filed by the same rule. Waits for it and answers its record: tip, at, green, took_ms, tail, filed. Refused when the base declares no `[idle]`, or while idle work of any checkout runs or waits on the host (one at a time), naming it and its `unqueue`.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Idle { checkout: String },
    /// Takes an entry (its id, as doctor's `heavy` lists it) off the heavy queue, waiting or running: one that does not move and holds the rest up. A running one is ended with every process it started. Its owner, if it still asks, queues again behind the rest.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Unqueue { entry: Uuid },
    /// The host's heavy queue, for a program of yours that is not a worker.
    #[command(subcommand, next_help_heading = OWNER, verbatim_doc_comment)]
    Heavy(HeavyCmd),
    /// Holds a checkout's landings (`--why`), or releases them (`--release`).
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Hold {
        checkout: String,
        #[arg(long, required_unless_present = "release")]
        why: Option<String>,
        #[arg(long)]
        release: bool,
    },
    /// The kit: the checkouts pinned to one engine. The engine, model and account a session starts on are stablehand's pick (`stablehand pick`), walking its order (`stablehand order show`), and so is the command each engine starts with (`stablehand command set|show`), followed by factorium's own arguments: an engine stablehand keeps none for starts on its bare program with its stock arguments. With stablehand missing or failing, claude-code alone starts, by `claude --dangerously-skip-permissions`, on the host user's own login.
    #[command(subcommand, next_help_heading = OWNER, verbatim_doc_comment)]
    Kit(KitCmd),
    /// Answers the engine accounts as stablehand's books say now: the `accounts` of `stablehand status`, each with its windows and pace. stablehand is their one home: none is kept here.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Accounts,
    /// Answers the settings; with a name, one; with a value, sets it.
    ///
    /// With `--resource <name>`, the host's named resources instead (a device every checkout's workers share): with `--slots`, declares it (how many hold it at once; 1 is a mutex) and `--note` (what it is); with `--remove`, takes it away. Answers every resource declared. A change of slots applies from the next ask for it. `host`, the machine's load by the room rule, is never declared.
    #[command(next_help_heading = OWNER, verbatim_doc_comment)]
    Settings {
        name: Option<String>,
        value: Option<u64>,
        #[arg(long, value_name = "NAME", conflicts_with_all = ["name", "value"])]
        resource: Option<String>,
        #[arg(long, requires = "resource", conflicts_with = "remove")]
        slots: Option<u64>,
        #[arg(long, requires = "slots")]
        note: Option<String>,
        #[arg(long, requires = "resource")]
        remove: bool,
    },
    /// A worker's commands.
    #[command(subcommand, next_help_heading = "Run by a worker", verbatim_doc_comment)]
    Worker(WorkerCmd),
    /// The watch's commands. The watch runs every owner command too, but `stop`.
    #[command(subcommand, next_help_heading = "Run by the watch", verbatim_doc_comment)]
    Watch(WatchCmd),
    /// The hook a session runs before each tool call: refuses what its session may not run.
    #[command(hide = true, next_help_heading = "Run by a session's hook", verbatim_doc_comment)]
    Guard { harness: String },
}

#[derive(Subcommand)]
pub enum ServiceCmd {
    /// Writes the agent to ~/Library/LaunchAgents, with this shell's PATH and this database, and loads it (in place of one loaded before). Run it from a shell that finds git, orca, stablehand and the engines' programs. A boot loads no Background agent, so after one the first other command (doctor among them) loads it again, unless `stop` stopped it.
    #[command(verbatim_doc_comment)]
    Install,
    /// Unloads the agent and deletes its file.
    #[command(verbatim_doc_comment)]
    Remove,
    /// Answers the agent's definition as `install` would write it.
    #[command(verbatim_doc_comment)]
    Show,
}

#[derive(Subcommand)]
pub enum CheckoutCmd {
    /// Puts a repo on the list. It needs an `origin` holding the base; a proving branch origin lacks is made at the base. Answers it, with `warnings`: each fit configured at the base's root that its gate never runs, as doctor finds it (`checkout show`).
    #[command(verbatim_doc_comment)]
    Add {
        name: String,
        path: PathBuf,
        #[arg(long, default_value = "main")]
        base: String,
        #[arg(long, default_value = "proving")]
        proving: String,
        /// Its default role: a directory, by its absolute path, that a worker of an issue naming no role starts with (an issue's own role wins).
        #[arg(long, value_name = "DIR", verbatim_doc_comment)]
        role: Option<String>,
    },
    /// Names a checkout's default role (`--role`), or takes it away (`--no-role`), for the workers that start from now; turns its idle work off on this host (`--no-idle`), or back on (`--idle`).
    #[command(verbatim_doc_comment, group(clap::ArgGroup::new("edit").required(true).multiple(true)
        .args(["role", "no_role", "idle", "no_idle"])))]
    Edit {
        name: String,
        #[arg(long, value_name = "DIR", conflicts_with = "no_role")]
        role: Option<String>,
        #[arg(long)]
        no_role: bool,
        /// Its idle work runs on this host, as the repo's `[idle]` declares.
        #[arg(long, conflicts_with = "no_idle", verbatim_doc_comment)]
        idle: bool,
        /// Its idle work runs not on this host: no idle run, no `tasks_bound` run, no task filed; `factorium idle` by hand still runs it. `list` shows it as `idle_off`.
        #[arg(long, verbatim_doc_comment)]
        no_idle: bool,
    },
    /// Takes a repo off the list.
    #[command(verbatim_doc_comment)]
    Remove { name: String },
    /// Answers the checkouts on the list.
    #[command(verbatim_doc_comment)]
    List,
    /// Answers one checkout, and each fit configured at its base's root as last fetched (`slimfit.toml`, `truefit.toml`, `docfit.toml`): its `state` is `run` (`in`: the file of the gate that calls its check), `left_out` (`why`: the reason `[gate.left_out]` in its factorium.toml gives) or `unrun` (`add_to`: the file its check belongs in; doctor finds it). Refused `bad_declaration` where its factorium.toml does not read: fix what the message names.
    #[command(verbatim_doc_comment)]
    Show { name: String },
}

#[derive(Subcommand)]
pub enum IssueCmd {
    /// Makes an issue, numbered one past the checkout's last. Answers it.
    #[command(group = clap::ArgGroup::new("waits").multiple(true).args(["waiting_for_human", "blocked_by"]), verbatim_doc_comment)]
    Add {
        checkout: String,
        title: String,
        #[arg(long)]
        body: Option<String>,
        /// Reads the body from this file; `-` reads stdin.
        #[arg(long, conflicts_with = "body", verbatim_doc_comment)]
        body_file: Option<PathBuf>,
        /// Waits for a person from the start: no worker takes it until a person readies it.
        #[arg(long, verbatim_doc_comment)]
        waiting_for_human: bool,
        /// Why it waits, repeat for each wait. After a `--blocked-by`, that block's why, required with each: kept with it and shown in `issue show`'s `waits` and doctor's `parked` (`--blocked-by 3 --why "its schema" --blocked-by 5 --why "its API"`). Before any `--blocked-by`, the person's, as `wait-for-human --why` gives one, shown in doctor's `asks`. One `--why` with one `--blocked-by`, before or after it, is the block's, and the person's too with `--waiting-for-human`. Refused: a second `--why` for the same wait (`why_twice`), one before any `--blocked-by` without `--waiting-for-human` (`why_unbound`).
        #[arg(long, requires = "waits", verbatim_doc_comment)]
        why: Vec<String>,
        /// Where each `--why` stands: how many `--blocked-by` come before it.
        #[arg(skip)]
        after: Vec<usize>,
        /// An issue it waits for, repeat for each: `N` of its checkout; `<checkout>#N` of another, until it lands; `<checkout>#N@cut`, until a cut of that checkout holds its landing (and installs it). Refused without a `--why` after it: the code it needs from that issue. Two issues that only use the same device or host do not block each other: that is a resource (`settings --resource`).
        #[arg(long = "blocked-by", value_name = "[CHECKOUT#]N[@cut]", verbatim_doc_comment)]
        blocked_by: Vec<Blocker>,
        /// No worker takes it before this time: RFC 3339 with its offset (`2026-11-23T14:34:37+09:00`, or `Z`), or ms since the epoch.
        #[arg(long, value_name = "TIME", value_parser = when::parse, verbatim_doc_comment)]
        not_before: Option<i64>,
        /// The role its worker plays: a directory, by its absolute path, whose rules (CLAUDE.md) and skills (.claude/skills) its worker starts with beside factorium's worker rules. Read, never written. One holding a rolesmith.toml is checked before each start (`rolesmith doctor --project DIR`), a program it lacks installed; while it lacks anything its issues wait, and doctor names what.
        #[arg(long, value_name = "DIR", verbatim_doc_comment)]
        role: Option<String>,
        /// The engine account its worker starts on, by its name in stablehand, by the command stablehand keeps for its engine. Refused unless `stablehand account show NAME` knows it and a session may start on it here. While stablehand's pick passes it over (at its limit, or full) its issue waits.
        #[arg(long, value_name = "NAME", verbatim_doc_comment)]
        account: Option<String>,
        /// The model its worker starts on, by the id its engine starts it with (`agy models`: `claude-opus-5-5-high`): stablehand's pick is asked for it (`stablehand pick --model`), tried after the order's own where the order does not list it.
        #[arg(long, value_name = "MODEL", verbatim_doc_comment)]
        model: Option<String>,
        /// Marks it urgent, the owner's mark (`worker file` cannot set it). Once nothing blocks it, it starts before every other ready issue, on the first tick the memory and disk floors allow (the CPU, the workers' share and `queue_wait_bound` do not hold it); its worker's heavy commands and its landing's gate go ahead of other workers' and programs' waiting entries, behind a line move past its bound; and its landing takes the next turn of its checkout's line. Among urgent issues the ordinary order holds. Blocks still hold it. From its first worker's start until it closes or `--no-urgent`, it halts the host for its heavy commands: a worker's or program's heavy command not urgent that runs is cut short once an urgent one waits with no room for a resource it holds (the host's reading finds no room, or a named resource's slots are all held: `ended: it was cut short: an urgent issue's command waits with no room for it`); with room the urgent one starts next and nothing is cut short; a `worker hold` is not cut short. The halt holds no command back: while no urgent one waits (its worker reading, writing code, or running a command of its own), heavy commands not urgent start by the ordinary order and room, and one waits `behind `fast gate of web#3`, an urgent issue's` only while an urgent one is next ahead of it. Line moves are not halted: other landings, promotions and cuts, on every checkout, go on through the queue as usual and are never cut short by it, since a line move delivers work already done and its gate is short; the urgent issue's own heavy commands still go ahead of a landing's waiting gate. The halt holds no worker's start: an issue not urgent starts as admission and the order allow at any other time, after the urgent ones, since holding starts kept every other checkout from work it could do without the CPU (reading, writing code, waiting its turn in the queue), while the queue already keeps the CPU for the urgent issue. doctor shows the halt as a finding naming the urgent issues. `issue list`, `issue show`, `workers` and doctor's `landing` and `heavy` show `urgent: true`; a tick's start or landing that went ahead for it says so (`urgent`). The mark stays once it closes.
        #[arg(long, verbatim_doc_comment)]
        urgent: bool,
    },
    /// Answers the open issues (with `--closed`, the closed too), of one checkout or, with none named, of every checkout, by checkout then number: what every checkout is doing and waiting on, in one call. Each row answers `issue show --brief`'s fields (among them `waits`, only those not met; `issue show` answers them all), and `ready_at` (ms: filed, or readied from waiting for a person) and `ahead` (a promotion's mutants issue, started before the ordinary ones). Its `status` (as `issue show`'s, with or without `--brief`) says what it is doing now: `ready` (no live worker holds it; it starts when what it waits for is met), `working` (a live worker is on it, waiting for the watch among it), `landing` (its worker ran `done`: its landing waits in line or its gate runs), `parked` (its worker is parked, behind what the issue waits for or until its engine's limit resets), `waiting_for_human` (it waits for the owner's answer: `issue ready` or `issue close`), `closed` (landed, or closed with its `reason`).
    #[command(verbatim_doc_comment)]
    List {
        checkout: Option<String>,
        #[arg(long)]
        closed: bool,
    },
    /// Answers one issue, with `waits`, each thing it waits for (`on`), why (a block made before whys were asked for: "(no reason given)"; its time: null) and how it was met (`met`, null while it is not); with its comments: what the owner, its workers, the watch and factorium said about it, each with who and when (ms); the role and account it names; and `spent`, what it took: each attempt's time from start to end (a live one's, to now) in ms, and the tokens its session spent (`null`: its harness's files do not say), with their sums. With `--steps`, each attempt answers too `steps`: where its time went, each step with its start and end in ms (null: still open). `heavy`: each heavy command it ran, `asked_at`, `started_at` (the queue granted it), `ended_at`, `queued_ms` and `ran_ms` apart, and `outcome` (`green`, `red`, `timed_out`; null: a hold, or one that has not ended). `landings`: each `done`, its `asked_at`, each check that its branch merges while it waited its turn (`checked`), each fast gate on its merge (`gates`, as `heavy`: asked at its turn in the line), `ended_at`, `outcome` (`landed`, `red`, or the status it left landing for), and on a red its `kind` (`no_merge`, `gate_red`, `past_limit`, `regenerated`: red on a merge that took proving's copy of regenerated records), the `files` it names (those that conflict, or the records taken) and `told`, the message that told it, with when it reached the session (`reached_at`). `waits`: each wait it declared (`worker waiting`); `parks`: each park, behind what its issue waits for or on its engine's limit, with why. `messages`: each message passed to its session, by its number in `outbox --worker <id>`, which holds its text.
    #[command(verbatim_doc_comment)]
    Show {
        checkout: String,
        number: u64,
        #[arg(long)]
        steps: bool,
        /// Answers only where it stands: `checkout`, `number`, `title`, `status`, `urgent`, `reason`, `worker`, as the full answer has them; `doing`, its live worker's `status` (`working`, `landing`, `waiting`…), `why` and `since` (ms), or null; `waits`, only those not met, each `on` and `why`; `comments`, only the last, its `text` cut to its first 300 characters (then `cut`: true); and `spent`, its `ms` alone. No body, no steps. `issue list` answers each row with these fields too.
        #[arg(long, conflicts_with = "steps", verbatim_doc_comment)]
        brief: bool,
    },
    /// Waits until each issue named has ended, then answers: it landed, it closed, or (`--until any`, the default) it waits for a person; or until `--timeout`. Watches the database, looking at each issue once a second; no other work. Answers `outcome` and `code` (its exit code), and `issues`: each one's `checkout`, `number`, `title`, `status`, `reason` (why it closed, or why it waits for a person), `commit` (the commit it landed as; null: it did not land) and `outcome`: `landed` (exit 0), `closed` without landing (3), `waiting_for_human` (4), or `timeout` (5: it had not ended then; its status is as it was). With several issues it ends once all have, and `outcome` and `code` are the last of theirs in that order. One that has ended already answers at once; one not there is refused (`no_issue`).
    #[command(verbatim_doc_comment)]
    Wait {
        checkout: String,
        #[arg(required = true)]
        numbers: Vec<u64>,
        /// `any`: it lands, closes or waits for a person. `closed`: it lands or closes; a wait for a person is waited past (a closed issue never opens again, so this is also "until it lands").
        #[arg(long, default_value = "any", value_parser = until(), verbatim_doc_comment)]
        until: Until,
        /// Ends the wait then, answering each issue as it is: `45m`, `2h`.
        #[arg(long, value_name = "DURATION", value_parser = when::duration_ms, verbatim_doc_comment)]
        timeout: Option<i64>,
    },
    /// Changes an issue's title, body, role or urgent mark. A live worker on it gets the edit at once: its brief is rewritten to the title and body as they are now, and a body changed is typed at its session as a comment is, telling it to read its brief again (a waiting worker's brief is rewritten, and it is not typed at). The answer's `delivery` says how a changed body reached it (`brief`: only its brief), `undelivered` why it did not.
    #[command(verbatim_doc_comment)]
    Edit {
        checkout: String,
        number: u64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body: Option<String>,
        /// Reads the body from this file; `-` reads stdin.
        #[arg(long, conflicts_with = "body", verbatim_doc_comment)]
        body_file: Option<PathBuf>,
        /// Names the role its workers play from now, checked as at `issue add --role`; a live worker keeps the role it started with. The change is kept as a comment on the issue.
        #[arg(long, value_name = "DIR", conflicts_with = "no_role", verbatim_doc_comment)]
        role: Option<String>,
        /// Takes its role away: its workers start from now with none.
        #[arg(long, verbatim_doc_comment)]
        no_role: bool,
        /// Marks it urgent, as `issue add --urgent` says; a live worker's heavy commands and landing go ahead from their next ask.
        #[arg(long, conflicts_with = "no_urgent", verbatim_doc_comment)]
        urgent: bool,
        /// Takes the urgent mark away: it goes back to the ordinary order.
        #[arg(long, verbatim_doc_comment)]
        no_urgent: bool,
    },
    /// Closes an issue; a worker on it ends.
    #[command(verbatim_doc_comment)]
    Close {
        checkout: String,
        number: u64,
        #[arg(long, default_value = "closed by hand")]
        reason: String,
    },
    /// Leaves an issue to a person, for a decision only a person can make: only `issue ready` moves it again. A worker on it ends, keeping its worktree.
    #[command(verbatim_doc_comment)]
    WaitForHuman {
        checkout: String,
        number: u64,
        #[arg(long)]
        why: String,
    },
    /// Readies an issue that waits for a person.
    #[command(verbatim_doc_comment)]
    Ready { checkout: String, number: u64 },
    /// Moves an issue to another host's factorium, reached by ssh (`--to`, as ssh names it; run there in a login shell). Checks first that its factorium lists a checkout of this name with this one's origin, changing nothing here until it does. Then: its worker here ends, its worktree kept, as `wait-for-human`; what that worktree holds that origin does not (its commits, and its changes not committed as one commit on top) is pushed to origin's `factorium/move/<checkout>-<n>`; it is filed there with `issue add` (its body, where it came from, the branch to merge first, then its comments with who and when; its role, account and blockers, this host's, are named, not carried; one that waited for a person here is filed `--waiting-for-human`, its `--why` naming where it waited and why, so no worker there takes it before a person decides; a ready one arrives ready); and it is closed here with a comment naming the issue there. Answers the issue there (`moved_to`) and the branch. A step that fails answers what was done before it; the issue here stays open.
    #[command(verbatim_doc_comment)]
    Move {
        checkout: String,
        number: u64,
        #[arg(long, value_name = "SSH_TARGET")]
        to: String,
    },
    /// Starts an issue over: removes the worktree its last attempt kept, so the next start begins from proving; the old branch is kept as `<branch>-abandoned` and named in a comment with the why. Refused while a worker is on it (leave it to a person first); ready or waiting for a person stays as it is.
    #[command(verbatim_doc_comment)]
    StartOver {
        checkout: String,
        number: u64,
        #[arg(long)]
        why: String,
    },
    /// Adds a comment: it reaches the live worker on the issue at once (typed at its session, or in its brief not yet handed; a waiting worker's through its watch episode, and typed with the line that sends it back to work), never stopping its running turn, and the next worker's brief carries it. Answers `delivery`: `submitted`, `queued` or `missed` (typed at its session), `brief`, `watch`, or `none` (no worker, or delivery failed: `undelivered` says why).
    #[command(verbatim_doc_comment)]
    Comment { checkout: String, number: u64, text: String },
    /// Makes an issue wait for another, of its checkout or of another, for `--why`, or until a time (`--undo`: stop). Its worker is parked: once idle (waiting, or in a wait it declared), back to work as soon as all it waits for is met.
    #[command(group = clap::ArgGroup::new("what").required(true).args(["by", "until"]), verbatim_doc_comment)]
    Block {
        checkout: String,
        number: u64,
        /// `N` of its checkout, `<checkout>#N` or `<checkout>#N@cut`, as for `issue add`.
        #[arg(long, alias = "blocked-by", value_name = "[CHECKOUT#]N[@cut]", verbatim_doc_comment)]
        by: Option<Blocker>,
        /// Why it waits for `--by`, required with it unless `--undo`, as for `issue add`: blocking it again with another why replaces it.
        #[arg(long, requires = "by", verbatim_doc_comment)]
        why: Option<String>,
        /// No worker takes it before this time, as for `issue add --not-before`; it replaces the time it waited for. With `--undo`, give no time: it waits for none.
        #[arg(long, value_name = "TIME", num_args = 0..=1, value_parser = when::parse, verbatim_doc_comment)]
        until: Option<Option<i64>>,
        #[arg(long)]
        undo: bool,
    },
}

#[derive(Subcommand)]
pub enum KitCmd {
    /// Answers the kit: `{"pins": {"<checkout>": "<engine>"}}`. Each engine's command is stablehand's (`stablehand command show`).
    #[command(verbatim_doc_comment)]
    Show,
    /// Pins a checkout to one engine (`--harness`), or unpins it: its starts ask stablehand's pick of that engine alone (`--engine`).
    #[command(verbatim_doc_comment)]
    Pin {
        checkout: String,
        #[arg(long)]
        harness: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum WorkerCmd {
    /// Runs the repo's fast gate on your worktree, through the queue.
    #[command(verbatim_doc_comment)]
    Gate,
    /// Asks to land your branch. Then stop: you are told if it is red.
    #[command(verbatim_doc_comment)]
    Done {
        /// Finishes without a landing, for an issue whose work changes no code (a publish, an upload, a measurement): your issue closes with this evidence (what you did, how you verified it) as its reason, no gate runs, and your session ends. Refused while your branch has commits of its own: those land.
        #[arg(long, value_name = "EVIDENCE", verbatim_doc_comment)]
        no_landing: Option<String>,
    },
    /// Hands you to the watch with what you need, kept as a comment on your issue. Then stop.
    #[command(verbatim_doc_comment)]
    Escalate { why: String },
    /// Adds a comment to your issue, as yours: for records (measurements, what you found). It changes nothing else and calls no one; to ask for a decision, escalate.
    #[command(verbatim_doc_comment)]
    Comment {
        /// The comment; without it, it is read from stdin.
        #[arg(long, verbatim_doc_comment)]
        text: Option<String>,
    },
    /// Says you wait on a long command of your own (a background download), kept as a comment on your issue: you are not found silent until `--until` from now (held to 2h). Past it you are, with what you said.
    #[command(verbatim_doc_comment)]
    Waiting {
        /// What you wait for.
        #[arg(long, verbatim_doc_comment)]
        why: String,
        /// How long: `90s`, `45m`, `2h`; a bare number is minutes.
        #[arg(long, value_parser = when::duration_ms, verbatim_doc_comment)]
        until: i64,
    },
    /// Files an issue your issue asks for (a follow-up), as `issue add` does, on any checkout on the list; your issue keeps the record ("filed <checkout>#<n>") and the filed one names you. Answers it, with the blocks it got. On your own checkout it also waits for your issue to land, unless `--independent`. An issue that needs the owner's decision is escalated instead.
    #[command(verbatim_doc_comment)]
    File {
        checkout: String,
        title: String,
        /// Reads the body from this file; `-` reads stdin.
        #[arg(long, verbatim_doc_comment)]
        body_file: PathBuf,
        /// An issue it waits for, repeat for each, as for `issue add`.
        #[arg(long = "blocked-by", value_name = "[CHECKOUT#]N[@cut]", verbatim_doc_comment)]
        blocked_by: Vec<Blocker>,
        /// Why it waits for the `--blocked-by` before it, one after each, as for `issue add`. The wait for your own issue says why on its own.
        #[arg(long, requires = "blocked_by", verbatim_doc_comment)]
        why: Vec<String>,
        /// Where each `--why` stands: how many `--blocked-by` come before it.
        #[arg(skip)]
        after: Vec<usize>,
        /// No worker takes it before this time, as for `issue add`.
        #[arg(long, value_name = "TIME", value_parser = when::parse, verbatim_doc_comment)]
        not_before: Option<i64>,
        /// On your own checkout, does not wait for your issue: for a follow-up that does not need your change.
        #[arg(long, verbatim_doc_comment)]
        independent: bool,
    },
    /// Has a second agent, a fresh session factorium starts, pick your change's mutants from slimfit's candidates, given slimfit's goal (`slimfit help picker`) and your diff. Answers the picks, or why none, by whom, and the commands that run and record them.
    #[command(verbatim_doc_comment)]
    PickMutants {
        /// Takes the change against this ref, as `slimfit check --target` does: everything your HEAD adds since its merge base with it, its diff and its candidates; the commands answered take it too. A promotion's mutants issue names it: the range's base.
        #[arg(long, value_name = "REF", verbatim_doc_comment)]
        target: Option<String>,
    },
    /// Has a second agent, a fresh session factorium starts, judge whether your change could be smaller, given slimfit's goal (`slimfit help reviewer`) and your diff. Answers its verdict (`as_is` with why, or `cut`), by whom, and the command that writes the growth record.
    #[command(verbatim_doc_comment)]
    ReviewGrowth,
    /// Runs a heavy command once the machine has room, after those queued before it, at a gate's priority. Exits with its exit code.
    ///
    /// A resource is a named thing with a capacity rule, each with its own queue: `host`, the machine's load (the room rule: what is above), and those the owner declares in the host's settings (a device, a build host: `slots`, how many hold it at once). With `--resource` (repeat it), it holds those while it runs, granted all at once or not at all, and not `host` unless named: it waits for them alone, and commands asking for others run beside it. Only a command holding `host` is ended by the hold bound. Use a resource for a device or a host, never `issue block` on another issue only because both use the same device: only your command waits, not your issue. `doctor` and `workers` show who holds each resource and who waits for it.
    #[command(verbatim_doc_comment)]
    Exclusive {
        /// Runs it at background priority instead: for work whose time no gate compares (a mutation run), so gates get the CPU first.
        #[arg(long, verbatim_doc_comment)]
        background: bool,
        /// A resource it holds while it runs, repeat for each: `host`, or one the host's settings declare (refused `unknown_resource` otherwise). None: `host`.
        #[arg(long = "resource", value_name = "NAME", verbatim_doc_comment)]
        resources: Vec<String>,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Holds named resources across several commands (a soak on a device), all at once, once each has a free slot: waits for them, and answers the hold. Each resource's queue starts the shortest declared hold first (a command, which declares none, is read as `hold_default`), so declare what you will take, not more; one waiting past `starve_bound` goes ahead of all others. Use this order, never `issue block` between issues that share a device. It stays yours after this answers, until `worker release` or your worker ends (factorium gives it back). It is never cut short before `--for`; past it, it is a finding for the watch. Your own `worker exclusive --resource` of a resource you hold runs inside your hold. Not `host`: the host is held by a command alone.
    #[command(verbatim_doc_comment)]
    Hold {
        /// The resources, as the host's settings declare them.
        #[arg(required = true, value_name = "NAME", verbatim_doc_comment)]
        resources: Vec<String>,
        /// How long you hold them: `90s`, `160m`, `2h`; a bare number is minutes.
        #[arg(long = "for", value_name = "DURATION", value_parser = when::duration_ms, verbatim_doc_comment)]
        hold_for: i64,
        /// When it may start, the host's local time: `22:00-07:00` (an overnight run on the charger). It joins the line only inside this window, every day; outside it, doctor shows it waiting for its window. Once started, it holds until released.
        #[arg(long, value_name = "HH:MM-HH:MM", value_parser = when::window, verbatim_doc_comment)]
        between: Option<(u32, u32)>,
    },
    /// Gives back your hold of a resource (`worker hold`).
    #[command(verbatim_doc_comment)]
    Release {
        #[arg(value_name = "NAME")]
        resource: String,
    },
}

#[derive(Subcommand)]
pub enum HeavyCmd {
    /// Runs a heavy command once the machine has room, by the queue's rule, beside workers' and gates' (a line move past its bound, and a landing's fast gate, go ahead of it, as of a worker's), at a gate's priority. Exits with its exit code. With `--resource`, it holds those resources instead, all at once, as `worker exclusive --resource` does.
    #[command(verbatim_doc_comment)]
    Run {
        /// Runs it at background priority instead: gates get the CPU first.
        #[arg(long, verbatim_doc_comment)]
        background: bool,
        /// A resource it holds while it runs, repeat for each (none: `host`).
        #[arg(long = "resource", value_name = "NAME", verbatim_doc_comment)]
        resources: Vec<String>,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Answers the queue's depth: how many run and wait (`running`, `waiting`), each entry, and each resource.
    #[command(verbatim_doc_comment)]
    Depth,
}

#[derive(Subcommand)]
pub enum WatchCmd {
    /// Answers the round running now and every open episode.
    #[command(verbatim_doc_comment)]
    Next,
    /// Ends the round with what you found and did.
    #[command(verbatim_doc_comment)]
    Report {
        #[arg(long)]
        text: String,
        /// What only the owner can do (spend money).
        #[arg(long, verbatim_doc_comment)]
        human: Option<String>,
    },
    /// Sends a waiting worker back to work, told `--text`, kept as a comment on its issue. The owner's comments made while it waited go with it.
    #[command(verbatim_doc_comment)]
    Restart {
        worker: Uuid,
        #[arg(long, default_value = "The watch looked at what stopped you; go on.")]
        text: String,
        /// First stops its running turn (a hung call), so it reads this now.
        #[arg(long, verbatim_doc_comment)]
        now: bool,
    },
    /// Types a line at a worker, kept as a comment on its issue; answers `submitted` (it started a turn) or `queued` (it waits behind the worker's running turn). To a waiting worker it is `restart`.
    #[command(verbatim_doc_comment)]
    Tell {
        worker: Uuid,
        text: String,
        /// First stops its running turn (a hung call), so it reads this now.
        #[arg(long, verbatim_doc_comment)]
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
                CheckoutCmd::Edit { name, role, no_role, idle, no_idle } => {
                    let role = (role.is_some() || no_role).then_some(role.as_deref());
                    app.checkout_edit(&name, role, (idle || no_idle).then_some(idle)).await
                }
                CheckoutCmd::Remove { name } => app.checkout_remove(&name).await,
                CheckoutCmd::List => app.checkout_list().await,
                CheckoutCmd::Show { name } => app.checkout_show(&name).await,
            },
            Top::Issue(i) => match i {
                IssueCmd::Add {
                    checkout,
                    title,
                    body,
                    body_file,
                    waiting_for_human,
                    why,
                    after,
                    blocked_by,
                    not_before,
                    role,
                    account,
                    model,
                    urgent,
                } => {
                    let body = body_text(body, body_file)?.unwrap_or_default();
                    let on = factorium_domain::issue::On { role, account, model };
                    let (mut blocked_by, why) = waits(blocked_by, why, &after, waiting_for_human)?;
                    blocked_by.extend(not_before.map(|at| Wait { by: Blocker::NotBefore(at), why: None }));
                    let open = factorium_domain::issue::Open {
                        checkout,
                        title,
                        body,
                        waiting_for_human,
                        why,
                        blocked_by,
                        on,
                        urgent,
                        ..Default::default()
                    };
                    app.issue_file(open).await
                }
                IssueCmd::List { checkout, closed } => app.issue_list(checkout.as_deref(), closed).await,
                IssueCmd::Show { checkout, number, brief: true, .. } => app.issue_show_brief(&checkout, number).await,
                IssueCmd::Show { checkout, number, steps: false, .. } => app.issue_show(&checkout, number).await,
                IssueCmd::Show { checkout, number, steps: true, .. } => app.issue_show_steps(&checkout, number).await,
                IssueCmd::Wait { checkout, numbers, until, timeout } => {
                    app.issue_wait(&checkout, &numbers, until, timeout).await
                }
                IssueCmd::Edit { checkout, number, title, body, body_file, role, no_role, urgent, no_urgent } => {
                    let body = body_text(body, body_file)?;
                    if role.is_some() || no_role {
                        app.issue_name_role(&checkout, number, role.as_deref()).await?;
                    }
                    if urgent || no_urgent {
                        app.issue_mark_urgent(&checkout, number, urgent).await?;
                    }
                    app.issue_edit(&checkout, number, title.as_deref(), body.as_deref()).await
                }
                IssueCmd::Close { checkout, number, reason } => app.issue_close(&checkout, number, &reason).await,
                IssueCmd::WaitForHuman { checkout, number, why } => {
                    app.issue_wait_for_human(&checkout, number, &why).await
                }
                IssueCmd::Ready { checkout, number } => app.issue_ready(&checkout, number).await,
                IssueCmd::Move { checkout, number, to } => app.issue_move(&checkout, number, &to).await,
                IssueCmd::StartOver { checkout, number, why } => app.issue_start_over(&checkout, number, &why).await,
                IssueCmd::Comment { checkout, number, text } => app.issue_comment(&checkout, number, &text).await,
                IssueCmd::Block { checkout, number, by, why, until, undo } => {
                    let by = match (by, until) {
                        (Some(by), _) => by,
                        (None, Some(Some(at))) => Blocker::NotBefore(at),
                        (None, _) if undo => Blocker::NotBefore(0),
                        (None, _) => return Err(Refusal::new("no_time", "name the time it waits until")),
                    };
                    app.issue_block(&checkout, number, by, why.as_deref(), undo).await
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
            Top::Spent { checkout, numbers, since } => app.spent_of(checkout.as_deref(), numbers, since).await,
            Top::Unqueue { entry } => app.unqueue(entry).await,
            Top::Heavy(h) => match h {
                HeavyCmd::Run { background, resources, command } => {
                    let how = factorium::Exclusive { background, resources };
                    app.heavy_run_as(&here()?, &line(&command), how).await
                }
                HeavyCmd::Depth => app.heavy_depth().await,
            },
            Top::Hold { checkout, why, release } => {
                app.hold(&checkout, if release { None } else { why.as_deref() }).await
            }
            Top::Kit(k) => match k {
                KitCmd::Show => app.kit().await,
                KitCmd::Pin { checkout, harness } => app.kit_pin(&checkout, harness.as_deref()).await,
            },
            Top::Accounts => app.accounts().await,
            Top::Settings { resource: Some(resource), slots, note, remove, .. } => {
                let declared = slots.map(|slots| factorium_domain::settings::Resource { slots, note });
                app.resource_declare(&resource, declared, remove).await
            }
            Top::Settings { name, value, .. } => app.settings_set(name.as_deref(), value).await,
            Top::Worker(w) => match w {
                WorkerCmd::Gate => app.worker_gate(worker_id()?).await,
                WorkerCmd::Done { no_landing } => app.worker_done(worker_id()?, no_landing.as_deref()).await,
                WorkerCmd::PickMutants { target } => app.worker_pick_mutants(worker_id()?, target.as_deref()).await,
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
                WorkerCmd::File { checkout, title, body_file, blocked_by, why, after, not_before, independent } => {
                    let body = body_text(None, Some(body_file))?.unwrap_or_default();
                    let (mut blocked_by, _) = waits(blocked_by, why, &after, false)?;
                    blocked_by.extend(not_before.map(|at| Wait { by: Blocker::NotBefore(at), why: None }));
                    app.worker_file(worker_id()?, &checkout, &title, &body, &blocked_by, independent).await
                }
                WorkerCmd::Exclusive { background, resources, command } => {
                    let (dir, line) = (here()?, line(&command));
                    let how = factorium::Exclusive { background, resources };
                    app.worker_exclusive_as(worker_id().ok(), &dir, &line, how).await
                }
                WorkerCmd::Hold { resources, hold_for, between } => {
                    app.worker_hold(worker_id()?, &resources, hold_for, between).await
                }
                WorkerCmd::Release { resource } => app.worker_release(worker_id()?, &resource).await,
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

/// What `issue wait --until` reads: `any` or `closed`.
fn until() -> impl clap::builder::TypedValueParser<Value = Until> {
    use clap::builder::TypedValueParser;
    clap::builder::PossibleValuesParser::new(["any", "closed"])
        .map(|s| if s == "closed" { Until::Closed } else { Until::Any })
}

/// An issue's number, `12`, or a range of them, `198..222`, as its ends.
fn numbers(s: &str) -> Result<(u64, u64), String> {
    let n =
        |t: &str| t.trim().parse::<u64>().map_err(|_| format!("`{s}` is no issue number, nor a range as `198..222`"));
    let (from, to) = match s.split_once("..") {
        Some((from, to)) => (n(from)?, n(to)?),
        None => (n(s)?, n(s)?),
    };
    if from > to {
        return Err(format!("`{s}` is a range that ends before it starts: name it as `{to}..{from}`"));
    }
    Ok((from, to))
}

/// Each of `blocked_by` with the `--why` after it (`after`: how many stand
/// before each), and the person's, the one before them all. One `--why` with
/// one `--blocked-by` is both. A block left without one is the domain's to refuse.
fn waits(
    blocked_by: Vec<Blocker>,
    why: Vec<String>,
    after: &[usize],
    waiting_for_human: bool,
) -> Result<(Vec<Wait>, Option<String>), Refusal> {
    if let ([why], 0 | 1) = (&why[..], blocked_by.len()) {
        let waits = blocked_by.into_iter().map(|by| Wait { by, why: Some(why.clone()) }).collect();
        return Ok((waits, waiting_for_human.then_some(why.clone())));
    }
    let (mut person, mut of_block) = (None, vec![None; blocked_by.len()]);
    for (why, &k) in why.into_iter().zip(after) {
        let slot = if k == 0 { &mut person } else { &mut of_block[k - 1] };
        if slot.replace(why).is_some() {
            return Err(Refusal::new("why_twice", "two `--why` for one wait: a block takes the one after it"));
        }
    }
    if person.is_some() && !waiting_for_human {
        return Err(Refusal::new("why_unbound", "a `--why` before any `--blocked-by` needs `--waiting-for-human`"));
    }
    Ok((blocked_by.into_iter().zip(of_block).map(|(by, why)| Wait { by, why }).collect(), person))
}

/// The command `matches` holds, each `--why` placed among the
/// `--blocked-by` before it (`after`), which derive cannot see.
pub fn parsed(matches: &clap::ArgMatches) -> Result<Cli, clap::Error> {
    use clap::FromArgMatches;
    let mut cli = Cli::from_arg_matches(matches)?;
    let placed = |m: &clap::ArgMatches| {
        let blocks: Vec<usize> = m.indices_of("blocked_by").map(Iterator::collect).unwrap_or_default();
        let whys = m.indices_of("why").into_iter().flatten();
        whys.map(|w| blocks.iter().filter(|&&b| b < w).count()).collect::<Vec<_>>()
    };
    let sub = |m: &clap::ArgMatches, a: &str, b: &str| {
        m.subcommand_matches(a).and_then(|m| m.subcommand_matches(b)).map(placed).unwrap_or_default()
    };
    match &mut cli.command {
        Top::Issue(IssueCmd::Add { after, .. }) => *after = sub(matches, "issue", "add"),
        Top::Worker(WorkerCmd::File { after, .. }) => *after = sub(matches, "worker", "file"),
        _ => {}
    }
    Ok(cli)
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
