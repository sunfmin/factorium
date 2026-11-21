//! The daemon as the database knows it: the programs it could not find when
//! it started, and the run of ticks that failed in a row. A daemon that
//! cannot do its work is a finding, not a line in its log.

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{command, id};

pub fn daemon_id() -> factum_core::Uuid {
    id("daemon")
}

/// Ticks that failed in a row: how many, and the last error.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Failing {
    pub ticks: u64,
    pub error: String,
}

/// A new binary whose ticks all failed, and the kept one the daemon went
/// back to: their versions, and the last error.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RolledBack {
    pub from: String,
    pub to: String,
    pub error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Daemon {
    /// When it last started.
    pub started_at: i64,
    /// The PATH it started with.
    pub path: String,
    /// The programs it needs and did not find on that PATH.
    pub missing: Vec<String>,
    pub failing: Option<Failing>,
    /// Its last rollback, until a binary Homebrew links ticks well.
    #[serde(default)]
    pub rolled_back: Option<RolledBack>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DaemonEvent {
    DaemonStarted { at: i64, path: String, missing: Vec<String> },
    TickFailed { at: i64, error: String },
    TickDone,
    RolledBack { at: i64, rolled_back: RolledBack },
    RollbackOver,
}

pub struct Daemons;

impl Aggregate for Daemons {
    const NAME: &'static str = "daemon";
    type State = Daemon;
    type Event = DaemonEvent;

    fn evolve(s: &mut Daemon, event: DaemonEvent) {
        match event {
            DaemonEvent::DaemonStarted { at, path, missing } => {
                (s.started_at, s.path, s.missing) = (at, path, missing);
            }
            DaemonEvent::TickFailed { error, .. } => {
                let f = s.failing.get_or_insert_with(Failing::default);
                f.ticks += 1;
                f.error = error;
            }
            DaemonEvent::TickDone => s.failing = None,
            DaemonEvent::RolledBack { rolled_back, .. } => s.rolled_back = Some(rolled_back),
            DaemonEvent::RollbackOver => s.rolled_back = None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    pub path: String,
    pub missing: Vec<String>,
}

command!(StartDaemon, "daemon.start", Daemons, Start, |_p| daemon_id(), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![DaemonEvent::DaemonStarted {
        at: now.0,
        path: p.path.clone(),
        missing: p.missing.clone(),
    }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ticked {
    /// Why the tick failed; `None`: it did its work.
    pub error: Option<String>,
}

command!(
    /// A tick's outcome: a failure adds to the run; a success ends it.
    NoteTick,
    "daemon.tick",
    Daemons,
    Ticked,
    |_p| daemon_id(),
    |s, p, now| {
        Ok::<_, factum_core::Rejection>(match (&p.error, &s.failing) {
            (Some(e), _) => vec![DaemonEvent::TickFailed { at: now.0, error: e.clone() }],
            (None, Some(_)) => vec![DaemonEvent::TickDone],
            (None, None) => Vec::new(),
        })
    }
);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rollback {
    /// The rollback the kept binary ticked well after; `None`: a binary
    /// Homebrew links ticked well, and the last rollback is over.
    pub rolled_back: Option<RolledBack>,
}

command!(NoteRollback, "daemon.rollback", Daemons, Rollback, |_p| daemon_id(), |s, p, now| {
    Ok::<_, factum_core::Rejection>(match (&p.rolled_back, &s.rolled_back) {
        (Some(r), _) => vec![DaemonEvent::RolledBack { at: now.0, rolled_back: r.clone() }],
        (None, Some(_)) => vec![DaemonEvent::RollbackOver],
        (None, None) => Vec::new(),
    })
});

/// What the daemon's own files say of it, as the process edge reads them:
/// the pid its pid file names, whether that process lives, whether its lock
/// is held, and when it last ticked (ms on its clock).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub pid: Option<u32>,
    pub alive: bool,
    pub locked: bool,
    pub ticked: Option<i64>,
}

/// How long a running daemon may go without a tick: five of its ticks, and
/// never under two minutes, as one tick fetches every checkout and merges
/// its landings.
pub fn silent_bound(tick_every: i64) -> i64 {
    (5 * tick_every).max(120_000)
}

/// Why no daemon runs, if none does: the pid file names no live process
/// that holds its lock, or the one that does has not ticked within
/// [`silent_bound`] (hung). One that holds its lock and has written no tick
/// at all is a binary older than the tick file (one rolled back to, or not
/// yet swapped): it runs.
pub fn down(p: &Probe, tick_every: i64, now: i64) -> Option<String> {
    let Some(pid) = p.pid else { return Some("its pid file names no daemon".into()) };
    if !p.alive {
        return Some(format!("its pid file names pid {pid}, which is gone"));
    }
    if !p.locked {
        return Some(format!("pid {pid}, which its pid file names, does not hold its lock"));
    }
    let silent = now - p.ticked?;
    (silent > silent_bound(tick_every))
        .then(|| format!("pid {pid} holds its lock and has not ticked for {} s: it hangs", silent / 1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY: i64 = 10_000;

    fn running(ticked: i64) -> Probe {
        Probe { pid: Some(7), alive: true, locked: true, ticked: Some(ticked) }
    }

    #[test]
    fn a_live_locked_daemon_that_ticked_lately_runs() {
        assert_eq!(down(&running(1_000_000), EVERY, 1_000_000 + 120_000), None);
    }

    #[test]
    fn no_pid_a_dead_one_or_one_without_the_lock_is_down() {
        assert!(down(&Probe::default(), EVERY, 0).unwrap().contains("names no daemon"));
        let dead = Probe { alive: false, ..running(0) };
        assert!(down(&dead, EVERY, 0).unwrap().contains("pid 7, which is gone"));
        let unlocked = Probe { locked: false, ..running(0) };
        assert!(down(&unlocked, EVERY, 0).unwrap().contains("does not hold its lock"));
    }

    #[test]
    fn a_daemon_silent_past_the_bound_hangs() {
        let said = down(&running(0), EVERY, 120_001).unwrap();
        assert!(said.contains("has not ticked for 120 s: it hangs"), "{said}");
        let older = Probe { ticked: None, ..running(0) };
        assert_eq!(down(&older, EVERY, i64::MAX / 2), None, "a binary that writes no tick file runs");
        assert_eq!(silent_bound(60_000), 300_000, "five ticks, past the floor");
    }
}
