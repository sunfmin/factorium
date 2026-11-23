//! doctor: the findings computed from what the database holds and the
//! host's reading, each with how to fix it. It is the watch's look and the
//! owner's view.

use serde::{Deserialize, Serialize};

use crate::admission::{self, Reading};
use crate::daemon::Daemon;
use crate::declared::Kind;
use crate::heavy::{LineJob, Queue};
use crate::idle::{Idle, Run};
use crate::line::{self, Line, Move};
use crate::settings::{Setting, Settings, Time};
use crate::watch::Found;

/// What doctor reads.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Seen {
    /// Workers that wait for the watch.
    pub waiting: Vec<WaitingSeen>,
    /// Each listed checkout's proving line.
    pub lines: Vec<LineSeen>,
    /// Workers asking to land, oldest ask first, as the lander takes them.
    pub landing: Vec<LandingSeen>,
    /// The heavy-command queue.
    pub queue: Queue,
    /// Admission held, since when and why.
    pub held: Option<HeldSeen>,
    /// Projections with dead letters.
    pub dead_letters: Vec<DeadLetters>,
    /// The owner's part of the watch's reports not yet seen.
    pub human: Vec<String>,
    /// The daemon: what it could not find at start, its failing ticks.
    #[serde(default)]
    pub daemon: Daemon,
    /// Orca terminals titled as factorium's that no live worker or the
    /// watch holds.
    pub orphans: Vec<Terminal>,
    /// Terminals factorium closed that Orca refused to close or lists
    /// still.
    #[serde(default)]
    pub unclosed: Vec<Unclosed>,
    /// Workers' worktrees that no open issue's attempt owns.
    #[serde(default)]
    pub orphaned: Vec<Orphaned>,
    /// Ready issues no worker holds, of listed checkouts, oldest first.
    #[serde(default)]
    pub ready: Vec<ReadySeen>,
    /// Admission's reading, as the tick reads it.
    #[serde(default)]
    pub reading: Reading,
    /// Whether the kit has no entry: nothing starts until it has one.
    #[serde(default)]
    pub kit_empty: bool,
}

/// A ready issue no worker holds, as admission would take it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReadySeen {
    /// checkout#number.
    pub issue: String,
    /// What it waits for before it may start (an open blocker, its time, a
    /// claim): admission does not consider it yet.
    #[serde(default)]
    pub waits: bool,
    /// Why the kit starts nothing for it now (no harness, its accounts used
    /// up), if it cannot.
    #[serde(default)]
    pub kit: Option<String>,
}

/// Why admission starts none of the ready issues now: each reason it holds
/// (the host's readings beside their settings, the workers' share, the
/// heavy queue's wait) and, when no issue's harness or account can start,
/// the kit's. Empty when no issue may start yet, or one could start now.
pub fn unstarted(seen: &Seen, s: &Settings, now: i64) -> Vec<String> {
    let mut ready = seen.ready.iter().filter(|r| !r.waits).peekable();
    if ready.peek().is_none() {
        return Vec::new();
    }
    let mut why = admission::holds(&seen.reading, &seen.queue, now, s);
    // Each issue's kit answer, or none once one issue's kit can start.
    let kit: Option<Vec<&String>> = ready.map(|r| r.kit.as_ref()).collect();
    for kit in kit.into_iter().flatten().map(|k| format!("kit: {k}")) {
        if !why.contains(&kit) {
            why.push(kit);
        }
    }
    why
}

/// A worker's worktree that no open issue's attempt owns, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Orphaned {
    pub checkout: String,
    /// Its issue's number and its attempt, as its branch names them.
    pub number: u64,
    pub attempt: u32,
    pub path: String,
    pub branch: String,
    /// The branch removing it keeps what it holds on.
    pub kept: String,
    pub why: String,
    /// The git worktrees nested in it that discard cannot remove, each as
    /// `<path>: <why>`: they hold work, which is kept where it is.
    #[serde(default)]
    pub nested: Vec<String>,
}

/// A worker that waits for the watch.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WaitingSeen {
    pub worker: String,
    /// checkout#number.
    pub issue: String,
    pub why: String,
    /// The comments on its issue since it started waiting.
    pub heard: Vec<String>,
    /// Parked behind what its issue waits for: what of that is not met.
    /// While any is not, it is no finding: the watch decided already.
    #[serde(default)]
    pub parked: Vec<String>,
    /// Parked until its engine's limit resets, then (ms): no finding.
    #[serde(default)]
    pub limited: Option<i64>,
}

/// A worker asking to land.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LandingSeen {
    pub worker: String,
    pub checkout: String,
    pub number: u64,
    /// Since when it asks, ms.
    pub since: i64,
}

/// Admission held since `since`, ms, for `why`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HeldSeen {
    pub since: i64,
    pub why: String,
}

/// A projection's dead letters.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeadLetters {
    pub projection: String,
    pub count: usize,
}

/// An Orca terminal: its handle and title.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Terminal {
    pub handle: String,
    pub title: String,
}

/// A terminal factorium closed that is not closed, and why.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Unclosed {
    pub handle: String,
    pub title: String,
    pub why: String,
}

/// A checkout's proving line as doctor reads it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LineSeen {
    pub checkout: String,
    pub line: Line,
    /// Whether its base declares a cut.
    pub cuts: bool,
    /// Its local base, where it cannot follow origin's.
    #[serde(default)]
    pub stuck: Option<Stuck>,
    /// Its idle work's last run.
    #[serde(default)]
    pub idle: Idle,
    /// Its idle work is off on this host: a red last run is no finding.
    #[serde(default)]
    pub idle_off: bool,
}

/// A checkout's local base behind origin's that is not fast-forwarded,
/// because something of it would be lost.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stuck {
    pub base: String,
    /// The checkout's path.
    pub path: String,
    /// Commits of origin's base it lacks.
    pub behind: u64,
    /// Its own commits, not on origin's base.
    pub ours: u64,
    /// Whether it is checked out over changes not committed.
    pub dirty: bool,
}

/// How long a slow tick stays a finding, ms: long enough for the watch's
/// looks and the owner to see it.
const SLOW_TICK_SHOWN: i64 = 3_600_000;

pub fn findings(seen: &Seen, s: &Settings, now: i64) -> Vec<Found> {
    let mut f = Vec::new();
    for WaitingSeen { worker: id, issue, why, heard, .. } in
        seen.waiting.iter().filter(|w| w.parked.is_empty() && w.limited.is_none())
    {
        f.push(Found {
            key: format!("worker:{id}"),
            what: format!("worker {id} on {issue} waits: {why}"),
            fix: "read its terminal and worktree and the comments on its issue; fix what stops it, then \
                  `factorium watch restart` or `tell` it, or leave its issue to a person (`issue wait-for-human`)"
                .into(),
            heard: heard.clone(),
        });
    }
    let mut add =
        |key: String, what: String, fix: &str| f.push(Found { key, what, fix: fix.into(), heard: Vec::new() });
    for LineSeen { checkout, line, stuck, idle, idle_off, .. } in &seen.lines {
        if let Some(Stuck { base, path, behind, ours, dirty }) = stuck {
            let why = if *dirty {
                "it is checked out over changes not committed".to_string()
            } else {
                format!("it has {ours} commits of its own")
            };
            add(
                format!("base:{checkout}"),
                format!("{checkout}: local {base} in {path} is {behind} commits behind origin/{base}, and {why}"),
                "commit or put aside the changes, or merge or rebase its own commits onto origin's; the next tick \
                 fast-forwards it. factorium never resets it",
            );
        }
        if let Some((tip, why)) = &line.red {
            add(
                format!("promotion:{checkout}:{tip}"),
                format!("{checkout}: the slow gate is red on proving tip {tip}: {why}"),
                "find the landing that broke it; open an issue to fix it, or hold landings with `factorium hold`",
            );
        }
        if let Some(Run { tip, tail, .. }) = idle.red().filter(|_| !idle_off) {
            add(
                format!("idle:{checkout}:{tip}"),
                format!("{checkout}: its idle work is red on base tip {tip}: {tail}"),
                "find what on the base broke it; open an issue to fix it. The next idle run, once `every` has \
                 passed, tries again",
            );
        }
        if let Some(line::CutFailure { version, why, .. }) = &line.cut_failed {
            add(
                format!("cut:{checkout}:{version}"),
                format!("{checkout}: cutting {version} failed: {why}"),
                "fix the step that failed (an issue if it is the repo's); each tick reads it back and clears this once it \
                 holds; the next promotion of a newer commit owes the cut again, or a cut by hand tries it now",
            );
        }
    }
    stalls(seen, s, now, &mut add);
    if let Some(HeldSeen { since, why }) = &seen.held
        && now - since >= s.ms(Time::HeldFor)
    {
        add(
            format!("held:{}", why.split(':').next().unwrap_or(why)),
            format!("admission held for {} min: {why}", (now - since) / 60_000),
            "find what holds the resource: a hot process, a full disk; free it",
        );
    }
    if seen.kit_empty && !seen.ready.is_empty() {
        let issues: Vec<&str> = seen.ready.iter().map(|r| r.issue.as_str()).collect();
        add(
            "kit:empty".into(),
            format!("the kit is empty, so no worker can start: {} ready ({})", issues.len(), issues.join(", ")),
            "set the harnesses this host starts, highest priority first: `factorium kit set \
             \"<harness>=<command>[@<model>]\"…`, e.g. `factorium kit set \"claude-code=claude \
             --dangerously-skip-permissions@opus\"`",
        );
    }
    for program in &seen.daemon.missing {
        add(
            format!("missing:{program}"),
            format!("the daemon cannot find `{program}` on its PATH: {}", seen.daemon.path),
            "install it, or put its directory on the service's PATH (run `factorium service install` from a shell \
             that finds it); the daemon checks again when it starts",
        );
    }
    if let Some(failing) = &seen.daemon.failing
        && failing.ticks >= s.get(Setting::TickFailuresMax)
    {
        add(
            "daemon:ticks".into(),
            format!("the daemon's last {} ticks failed: {}", failing.ticks, failing.error),
            "find what the error names (a program, a repo, the database) and fix it; daemon.log has every tick",
        );
    }
    if let Some(slow) = seen.daemon.slow.as_ref().filter(|s| now - s.at < SLOW_TICK_SHOWN) {
        add(
            format!("daemon:slow-tick:{}", slow.longest.step),
            format!(
                "a daemon tick took {} s, past half the hang bound, {} min ago; it spent {} s in {}",
                slow.took_ms / 1000,
                (now - slow.at) / 60_000,
                slow.longest.ms / 1000,
                slow.longest.step
            ),
            "find what that step waits for (a child process, the network, a lock: `sample` the daemon while it \
             runs) and move it beside the ticks, as the lines' gates are; twice as slow reads as a hung daemon",
        );
    }
    if let Some(r) = &seen.daemon.rolled_back {
        add(
            "daemon:rollback".into(),
            format!("the daemon went back from factorium {} to {}: its ticks failed: {}", r.from, r.to, r.error),
            &format!(
                "{} cannot run here: fix what the error names and cut a newer version; the daemon swaps to that \
                 once Homebrew links it, never to {} again",
                r.from, r.from
            ),
        );
    }
    for DeadLetters { projection: p, count: n } in &seen.dead_letters {
        add(format!("dead:{p}"), format!("projection {p} has {n} dead letters"), "a code fix, then retry them");
    }
    for Terminal { handle, title } in &seen.orphans {
        add(
            format!("orphan:{handle}"),
            format!("Orca terminal {handle} ({title}) is factorium's, but no live worker or the watch holds it"),
            &format!("close it: `orca terminal close --terminal {handle}`"),
        );
    }
    for Orphaned { path, branch, kept, why, nested, .. } in &seen.orphaned {
        let (held, first) = match nested.is_empty() {
            true => (String::new(), ""),
            false => (
                format!("; a worktree nested in it holds work: {}", nested.join("; ")),
                "first move or remove that, then ",
            ),
        };
        add(
            format!("worktree:{path}"),
            format!("worktree {path} ({branch}) is a worker's, but no open issue's attempt owns it: {why}{held}"),
            &format!(
                "{first}remove it: `factorium discard {path}`. What it holds that is nowhere else is kept on \
                 `{kept}`"
            ),
        );
    }
    for Unclosed { handle, title, why } in &seen.unclosed {
        add(
            format!("unclosed:{handle}"),
            format!("factorium closed Orca terminal {handle} ({title}), and it is not closed: {why}"),
            &format!("close it: `orca terminal close --terminal {handle}`"),
        );
    }
    f
}

/// What the line owes and has not done within its bound: a heavy queue
/// that starts nothing, a landing no gate runs for on a line that does not
/// move, a promotion or a cut past its bound with none running.
fn stalls(seen: &Seen, s: &Settings, now: i64, add: &mut impl FnMut(String, String, &str)) {
    let q = &seen.queue;
    let stall = s.ms(Time::StallBound);
    let min = |ms: i64| ms / 60_000;
    // What waits first in the queue, named so the watch can take it off.
    let head = q.oldest().map_or(String::new(), |e| {
        let ago = min(now - e.queued_at);
        format!("; the heavy queue's oldest waiting is `{}` (entry {}), queued {ago} min ago", e.what, e.id)
    });
    let unqueue = "if an entry of the heavy queue (doctor's `heavy`) does not move — its owner no longer asks for it, \
                   or it runs past the hold bound — take it off with `factorium unqueue <entry>`; an owner still \
                   alive queues again behind the rest";
    if let Some(e) = q.oldest() {
        let waited = now - e.queued_at;
        let idle = q.running().next().is_none();
        if (idle && waited > stall) || waited > s.ms(Time::HoldBound) + stall {
            let running: Vec<String> = q.running().map(|r| format!("`{}` (entry {})", r.what, r.id)).collect();
            let running = if idle { "nothing runs".into() } else { format!("running: {}", running.join(", ")) };
            add(
                format!("queue:{}", e.id),
                format!(
                    "the heavy queue has started nothing for {} min: {} waiting and {running}; its oldest, `{}` \
                     (entry {}), does not start",
                    min(waited),
                    q.waiting().count(),
                    e.what,
                    e.id
                ),
                unqueue,
            );
        }
    }
    let line_of = |c: &str| seen.lines.iter().find(|l| l.checkout == c);
    for LandingSeen { worker: id, checkout, number, since } in &seen.landing {
        let line = line_of(checkout).map(|l| &l.line);
        let held = line.is_some_and(|l| l.held.is_some());
        // Behind others on a line that moves (a landing, promotion, cut or job start) it waits its turn.
        let moved = line.and_then(|l| l.moved).max(q.started(checkout)).unwrap_or(*since).max(*since);
        if held || now - moved <= stall || q.runs(checkout, LineJob::Gate(Kind::Fast)) {
            continue;
        }
        add(
            format!("landing:{id}"),
            format!(
                "{checkout}#{number} has waited {} min to land, no gate runs for it, and its line has not moved for \
                 {} min{head}",
                min(now - since),
                min(now - moved)
            ),
            &format!("{unqueue}; the next tick gates the landing. If the daemon does not tick, see its daemon.log"),
        );
    }
    for LineSeen { checkout, line, cuts, .. } in &seen.lines {
        if let (Some(o), Some(owed)) = (line.unpromoted.first(), line.owed(Move::Promote, now, &line::bounds(s)))
            && owed.overdue()
            && line.red.is_none()
            && !q.runs(checkout, LineJob::Gate(Kind::Slow))
        {
            add(
                format!("unpromoted:{checkout}"),
                format!(
                    "{checkout}#{} landed {} min ago and is not promoted, past the promotion bound of {} min, and \
                     no promotion runs{head}",
                    o.number,
                    min(now - owed.since),
                    min(owed.bound)
                ),
                &format!("{unqueue}; then the next tick promotes, or `factorium promote {checkout}` promotes now"),
            );
        }
        if let Some(owed) = line.owed(Move::Cut, now, &line::bounds(s))
            && owed.overdue()
            && *cuts
            && line.cut_failed.is_none()
            && !q.runs(checkout, LineJob::Cut)
        {
            add(
                format!("uncut:{checkout}"),
                format!(
                    "{checkout}: what landed {} min ago is promoted and not cut, past the cut bound of {} min, and \
                     no cut runs{head}",
                    min(now - owed.since),
                    min(owed.bound)
                ),
                &format!("{unqueue}; then the next tick cuts, or `factorium cut {checkout}` cuts now"),
            );
        }
    }
}

/// No daemon runs, for `why` ([`crate::daemon::down`]): nothing lands,
/// starts or is watched until one does, so it is doctor's first finding.
/// `service` is launchd's target when it has the service loaded; `log` the
/// daemon's log.
pub fn no_daemon(why: &str, service: Option<&str>, log: &str) -> Found {
    let fix = match service {
        Some(target) => format!(
            "launchd has the service loaded and no daemon runs: read `launchctl print {target}` (its last exit) and \
             {log}; a hung one: `sample` its pid, then `factorium stop` and `factorium service install`"
        ),
        None => format!(
            "`factorium service install` from a shell whose PATH finds its programs; if launchd lists it \
             (`launchctl list | grep factorium`: its pid and last exit), {log} says how the last daemon ended"
        ),
    };
    Found { key: "daemon:none".into(), what: format!("no daemon runs: {why}"), fix, heard: Vec::new() }
}
