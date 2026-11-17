//! doctor: the findings computed from what the database holds and the
//! host's reading, each with how to fix it. It is the watch's look and the
//! owner's view.

use serde::{Deserialize, Serialize};

use crate::daemon::Daemon;
use crate::settings::{Setting, Settings, Time};
use crate::watch::Found;

/// What doctor reads.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Seen {
    /// Workers that wait for the watch: (worker id, checkout#number, why).
    pub waiting: Vec<(String, String, String)>,
    /// Checkouts whose proving tip's slow gate is red: (checkout, tip, why).
    pub red_promotions: Vec<(String, String, String)>,
    /// Checkouts whose last cut failed: (checkout, version, why).
    pub failed_cuts: Vec<(String, String, String)>,
    /// Admission held since then for this reason.
    pub held: Option<(i64, String)>,
    /// Projections with dead letters: (projection, count).
    pub dead_letters: Vec<(String, usize)>,
    /// Checkouts whose landings are held: (checkout, why).
    pub holds: Vec<(String, String)>,
    /// The owner's part of the watch's reports not yet seen.
    pub human: Vec<String>,
    /// The daemon: what it could not find at start, its failing ticks.
    #[serde(default)]
    pub daemon: Daemon,
}

pub fn findings(seen: &Seen, s: &Settings, now: i64) -> Vec<Found> {
    let mut f = Vec::new();
    let mut add = |key: String, what: String, fix: &str| f.push(Found { key, what, fix: fix.into() });
    for (id, issue, why) in &seen.waiting {
        add(
            format!("worker:{id}"),
            format!("worker {id} on {issue} waits: {why}"),
            "read its terminal and worktree; fix what stops it, then `factorium watch restart` or `tell` it, \
             or `park` its issue",
        );
    }
    for (checkout, tip, why) in &seen.red_promotions {
        add(
            format!("promotion:{checkout}:{tip}"),
            format!("{checkout}: the slow gate is red on proving tip {tip}: {why}"),
            "find the landing that broke it; open an issue to fix it, or hold landings with `factorium hold`",
        );
    }
    for (checkout, version, why) in &seen.failed_cuts {
        add(
            format!("cut:{checkout}:{version}"),
            format!("{checkout}: cutting {version} failed: {why}"),
            "fix the step that failed (an issue if it is the repo's); the next cut tries again",
        );
    }
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
    f
}
