//! doctor: the findings computed from what the database holds and the
//! host's reading, each with how to fix it. It is the watch's look and the
//! owner's view.

use serde::{Deserialize, Serialize};

use crate::daemon::Daemon;
use crate::declared::Kind;
use crate::heavy::{LineJob, Queue};
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
    /// Workers asking to land.
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
    for WaitingSeen { worker: id, issue, why, heard } in &seen.waiting {
        f.push(Found {
            key: format!("worker:{id}"),
            what: format!("worker {id} on {issue} waits: {why}"),
            fix: "read its terminal and worktree and the comments on its issue; fix what stops it, then \
                  `factorium watch restart` or `tell` it, or `park` its issue"
                .into(),
            heard: heard.clone(),
        });
    }
    let mut add =
        |key: String, what: String, fix: &str| f.push(Found { key, what, fix: fix.into(), heard: Vec::new() });
    for LineSeen { checkout, line, stuck, .. } in &seen.lines {
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
        if let Some((version, why)) = &line.cut_failed {
            add(
                format!("cut:{checkout}:{version}"),
                format!("{checkout}: cutting {version} failed: {why}"),
                "fix the step that failed (an issue if it is the repo's); the next cut tries again",
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
/// that starts nothing, a landing no gate runs for, a promotion or a cut
/// past its bound with none running.
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
        let held = line_of(checkout).is_some_and(|l| l.line.held.is_some());
        if held || now - since <= stall || q.runs(checkout, LineJob::Gate(Kind::Fast)) {
            continue;
        }
        add(
            format!("landing:{id}"),
            format!("{checkout}#{number} has waited {} min to land and no gate runs for it{head}", min(now - since)),
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
