//! doctor: the findings computed from what the database holds and the
//! host's reading, each with how to fix it. It is the watch's look and the
//! owner's view.

use serde::{Deserialize, Serialize};

use crate::admission::{self, Reading};
use crate::daemon::Daemon;
use crate::declared::Kind;
use crate::heavy::{HOST, LineJob, Queue};
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
    /// Why stablehand, the accounts' one home, cannot be asked which account
    /// a session starts on, if it cannot: no worker starts then.
    #[serde(default)]
    pub stablehand: Option<StablehandSeen>,
    /// The kit's commands stablehand refused when they moved to it
    /// (factorium#306), of the engines it keeps no command for yet.
    #[serde(default)]
    pub unmoved: Vec<(String, crate::kit::Refused)>,
    /// The running entries holding a named resource whose holder is gone,
    /// and why: a command's waiter died, or its worker ended.
    #[serde(default)]
    pub holders_gone: Vec<(factum_core::Uuid, String)>,
}

/// stablehand, as a start asks it, failing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StablehandSeen {
    /// Whether it is on the PATH at all.
    pub found: bool,
    /// What failed: the command and what it answered.
    pub why: String,
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
    pub parked: Vec<Unmet>,
    /// Parked until its engine's limit resets, then (ms): no finding.
    #[serde(default)]
    pub limited: Option<i64>,
}

/// What a parked worker's issue waits for, not met: as the CLI names it,
/// and why it is waited for (none for its time).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Unmet {
    pub on: String,
    pub why: Option<String>,
}

/// A worker asking to land.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LandingSeen {
    pub worker: String,
    pub checkout: String,
    pub number: u64,
    /// Since when it asks, ms.
    pub since: i64,
    /// Its issue is marked urgent: it lands ahead of those that are not.
    #[serde(default)]
    pub urgent: bool,
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
    /// The mutants issue its promotion waits for, while it is open: its
    /// number, and when the wait ends (ms), `mutants_bound` after its filing.
    #[serde(default)]
    pub mutants: Option<(u64, i64)>,
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
    if !seen.queue.halt.is_empty() {
        let by = seen.queue.halt.join(", ");
        add(
            format!("halt:{by}"),
            format!(
                "urgent {by} halts every other start: no other worker, heavy command, landing, promotion or cut starts"
            ),
            "nothing to do while it works: once it lands, or is closed or `issue edit --no-urgent`, the next tick resumes \
             the rest in their order",
        );
    }
    stalls(seen, s, now, &mut add);
    resources(seen, s, now, &mut add);
    if let Some(HeldSeen { since, why }) = &seen.held
        && now - since >= s.ms(Time::HeldFor)
    {
        add(
            format!("held:{}", why.split(':').next().unwrap_or(why)),
            format!("admission held for {} min: {why}", (now - since) / 60_000),
            "find what holds the resource: a hot process, a full disk; free it",
        );
    }
    if let Some(StablehandSeen { found, why }) = &seen.stablehand {
        let fix = match found {
            false => "install it: `brew install sunfmin/tap/stablehand`; the next tick asks it again".to_string(),
            true => format!("do what its answer says ({why}); the next tick asks it again"),
        };
        add(
            "stablehand".into(),
            format!("stablehand cannot say which account a session starts on, so no worker of the kit starts: {why}"),
            &fix,
        );
    }
    for (engine, crate::kit::Refused { command, why }) in &seen.unmoved {
        add(
            format!("kit:unmoved:{engine}"),
            format!(
                "the kit's command for {engine}, `{command}`, was not moved to stablehand, which refused it: {why}; \
                 {engine} starts on its bare program"
            ),
            &format!("keep a command for it that stablehand takes: `stablehand command set '{engine}=<command>'`"),
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
    for (dir, lacking) in &seen.daemon.roles {
        let fixes: Vec<String> = lacking.fixes().iter().map(|f| format!("`{f}`")).collect();
        let fixes =
            if fixes.is_empty() { String::new() } else { format!("run what rolesmith gives: {}; ", fixes.join(", ")) };
        add(
            format!("role:{dir}"),
            format!("role {dir} lacks what its workers need, so no worker starts with it: {}", lacking.what()),
            &format!(
                "{fixes}`rolesmith doctor --project {dir}` passes once all is there; the daemon checks again before \
                 each start with it, and starts its issues then"
            ),
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
    if let Some(slow) = seen.daemon.slow.as_ref().filter(|s| s.stands()) {
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
    if let Some(r) = &seen.daemon.refused {
        add(
            format!("daemon:refused:{}", r.version),
            format!("the daemon does not swap to factorium {}, which Homebrew links: {}", r.version, r.why),
            &format!(
                "the old binary runs on; fix what the error names (the copy it names is kept for a look) and cut a \
                 newer version; the daemon swaps to that once Homebrew links it and it proves, never to {}",
                r.version
            ),
        );
    }
    if let Some(why) = seen.daemon.upgrade.as_ref().and_then(|u| u.failed.as_ref()) {
        add(
            "daemon:upgrade".into(),
            format!("the daemon could not upgrade its own formula from the tap: {why}"),
            "fix what brew says (the network, the tap, a lock); the daemon tries again each `upgrade_every`, and \
             a try that works ends this",
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

/// How long a resource given back because its holder died stays a finding, ms.
const GIVEN_BACK_SHOWN: i64 = 3_600_000;

/// A named resource whose holder is gone, whose worker ended, or that it
/// holds past the time it declared; one factorium gave back, for an hour.
fn resources(seen: &Seen, s: &Settings, now: i64, add: &mut impl FnMut(String, String, &str)) {
    let q = &seen.queue;
    let min = |ms: i64| ms / 60_000;
    let unqueue = "read its worker's terminal and issue; if it no longer holds it for anyone, take it off with \
                   `factorium unqueue <entry>`: the resource goes to the next waiting for it";
    let all = q.resources(now, s.order());
    for e in q.running() {
        let named: Vec<&str> = e.asks().into_iter().filter(|r| *r != HOST).collect();
        if named.is_empty() {
            continue;
        }
        let worker = e.ask.worker.map_or(String::new(), |w| format!(", worker {w}"));
        let by = format!("`{}` (entry {}{worker})", e.ask.what, e.ask.id);
        let waiting: usize = all.iter().filter(|r| named.contains(&r.name.as_str())).map(|r| r.waiting.len()).sum();
        let why = if let Some((_, gone)) = seen.holders_gone.iter().find(|(id, _)| *id == e.ask.id) {
            Some(format!("and {gone}; the next tick gives it back"))
        } else {
            e.until().filter(|u| now > *u).map(|u| format!("past the time it declared, by {} min", min(now - u)))
        };
        if let Some(why) = why {
            add(
                format!("resource:{}", e.ask.id),
                format!("{} held by {by}, {why}; {waiting} waiting for it", named.join(" and ")),
                unqueue,
            );
        }
    }
    for g in q.given_back.iter().filter(|g| now - g.at <= GIVEN_BACK_SHOWN) {
        let worker = g.worker.map_or(String::new(), |w| format!(" of worker {w}"));
        add(
            format!("resource:{}:{}", g.entry, g.resource),
            format!(
                "{} was given back {} min ago: its holder `{}` (entry {}){worker} held it, and {}",
                g.resource,
                min(now - g.at),
                g.what,
                g.entry,
                g.why
            ),
            "read the worker's terminal and issue: what it ran on the resource may have ended unfinished; whoever \
             needs it runs it again",
        );
    }
}

/// What the line owes and has not done within its bound: a heavy queue
/// that starts nothing, a landing no gate runs for on a line that does not
/// move, a promotion or a cut past its bound with none running.
fn stalls(seen: &Seen, s: &Settings, now: i64, add: &mut impl FnMut(String, String, &str)) {
    let q = &seen.queue;
    let stall = s.ms(Time::StallBound);
    let min = |ms: i64| ms / 60_000;
    // What waits first in the queue, named so the watch can take it off.
    let oldest = q.oldest(now);
    let head = oldest.map_or(String::new(), |e| {
        let ago = min(now - e.queued_at);
        format!("; the heavy queue's oldest waiting is `{}` (entry {}), queued {ago} min ago", e.ask.what, e.ask.id)
    });
    let unqueue = "if an entry of the heavy queue (doctor's `heavy`) does not move — its owner no longer asks for it, \
                   or it runs past the hold bound — take it off with `factorium unqueue <entry>`; an owner still \
                   alive queues again behind the rest";
    if let Some(e) = oldest {
        let waited = now - e.queued_at;
        let idle = q.running().next().is_none();
        if (idle && waited > stall) || waited > s.ms(Time::HoldBound) + stall {
            let running: Vec<String> = q.running().map(|r| format!("`{}` (entry {})", r.ask.what, r.ask.id)).collect();
            let running = if idle { "nothing runs".into() } else { format!("running: {}", running.join(", ")) };
            add(
                format!("queue:{}", e.ask.id),
                format!(
                    "the heavy queue has started nothing for {} min: {} waiting and {running}; its oldest, `{}` \
                     (entry {}), does not start",
                    min(waited),
                    q.waiting().count(),
                    e.ask.what,
                    e.ask.id
                ),
                unqueue,
            );
        }
    }
    let line_of = |c: &str| seen.lines.iter().find(|l| l.checkout == c);
    // Halted, what waits behind the urgent issues is the halt's finding, not a stall.
    let halted = !q.halt.is_empty();
    for LandingSeen { worker: id, checkout, number, since, urgent } in &seen.landing {
        let line = line_of(checkout).map(|l| &l.line);
        let held = line.is_some_and(|l| l.held.is_some()) || (halted && !urgent);
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
    for LineSeen { checkout, line, cuts, .. } in seen.lines.iter().filter(|_| !halted) {
        if let (Some(o), Some(owed)) = (line.unpromoted.first(), line.owed(Move::Promote, now, &line::bounds(s)))
            && owed.overdue()
            && line.red.is_none()
            && !q.runs(checkout, LineJob::Gate(Kind::Slow))
            && !waits_for_mutants(seen, checkout, now)
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

/// Whether `checkout`'s promotion waits for its mutants issue at `now`:
/// open, and within its bound, it holds the promotion back on purpose.
fn waits_for_mutants(seen: &Seen, checkout: &str, now: i64) -> bool {
    seen.lines.iter().any(|l| l.checkout == checkout && l.mutants.is_some_and(|(_, until)| now < until))
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
