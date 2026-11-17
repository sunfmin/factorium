//! doctor: the findings computed from what the database holds and the
//! host's reading, each with how to fix it. It is the watch's look and the
//! owner's view.

use serde::{Deserialize, Serialize};

use crate::daemon::Daemon;
use crate::heavy::Queue;
use crate::line::Line;
use crate::settings::{Setting, Settings, Time};
use crate::watch::Found;

/// What doctor reads.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Seen {
    /// Workers that wait for the watch: (worker id, checkout#number, why,
    /// the comments on its issue since it started waiting).
    pub waiting: Vec<(String, String, String, Vec<String>)>,
    /// Each listed checkout's proving line.
    pub lines: Vec<LineSeen>,
    /// Workers asking to land: (worker id, checkout, number, since when, ms).
    pub landing: Vec<(String, String, u64, i64)>,
    /// The heavy-command queue.
    pub queue: Queue,
    /// Admission held since then for this reason.
    pub held: Option<(i64, String)>,
    /// Projections with dead letters: (projection, count).
    pub dead_letters: Vec<(String, usize)>,
    /// The owner's part of the watch's reports not yet seen.
    pub human: Vec<String>,
    /// The daemon: what it could not find at start, its failing ticks.
    #[serde(default)]
    pub daemon: Daemon,
    /// Orca terminals titled as factorium's that no live worker or the
    /// watch holds: (handle, title).
    pub orphans: Vec<(String, String)>,
}

/// A checkout's proving line as doctor reads it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LineSeen {
    pub checkout: String,
    pub line: Line,
    /// Whether its base declares a cut.
    pub cuts: bool,
}

pub fn findings(seen: &Seen, s: &Settings, now: i64) -> Vec<Found> {
    let mut f = Vec::new();
    for (id, issue, why, heard) in &seen.waiting {
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
    for LineSeen { checkout, line, .. } in &seen.lines {
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
    if let Some((since, why)) = &seen.held
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
    for (p, n) in &seen.dead_letters {
        add(format!("dead:{p}"), format!("projection {p} has {n} dead letters"), "a code fix, then retry them");
    }
    for (handle, title) in &seen.orphans {
        add(
            format!("orphan:{handle}"),
            format!("Orca terminal {handle} ({title}) is factorium's, but no live worker or the watch holds it"),
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
    for (id, checkout, number, since) in &seen.landing {
        let held = line_of(checkout).is_some_and(|l| l.line.held.is_some());
        if held || now - since <= stall || q.runs(checkout, "fast gate") {
            continue;
        }
        add(
            format!("landing:{id}"),
            format!("{checkout}#{number} has waited {} min to land and no gate runs for it{head}", min(now - since)),
            &format!("{unqueue}; the next tick gates the landing. If the daemon does not tick, see its daemon.log"),
        );
    }
    for LineSeen { checkout, line, cuts } in &seen.lines {
        if let Some(o) = line.unpromoted.first()
            && line.red.is_none()
            && now - o.at > s.ms(Time::PromotionBound)
            && !q.runs(checkout, "slow gate")
        {
            add(
                format!("unpromoted:{checkout}"),
                format!(
                    "{checkout}#{} landed {} min ago and is not promoted, past the promotion bound of {} min, and \
                     no promotion runs{head}",
                    o.number,
                    min(now - o.at),
                    min(s.ms(Time::PromotionBound))
                ),
                &format!("{unqueue}; then the next tick promotes, or `factorium promote {checkout}` promotes now"),
            );
        }
        if let Some(at) = line.uncut.first()
            && *cuts
            && line.cut_failed.is_none()
            && now - at > s.ms(Time::CutBound)
            && !q.runs(checkout, "cut")
        {
            add(
                format!("uncut:{checkout}"),
                format!(
                    "{checkout}: what landed {} min ago is promoted and not cut, past the cut bound of {} min, and \
                     no cut runs{head}",
                    min(now - at),
                    min(s.ms(Time::CutBound))
                ),
                &format!("{unqueue}; then the next tick cuts, or `factorium cut {checkout}` cuts now"),
            );
        }
    }
}
