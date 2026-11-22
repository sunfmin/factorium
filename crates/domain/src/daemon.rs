//! The daemon as the database knows it: the programs it could not find when
//! it started, the run of ticks that failed in a row, and its last tick that
//! took past half the hang bound. A daemon that
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

/// A tick's longest step: its name and how long it took, ms.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub step: String,
    pub ms: i64,
}

/// A tick's steps as each ends, on the app's clock: the longest so far, the
/// first of those that took as long.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Steps {
    last: i64,
    pub longest: Step,
}

impl Steps {
    /// A tick that starts `now`.
    pub fn at(now: i64) -> Self {
        Steps { last: now, longest: Step::default() }
    }

    /// `step` ended `now`.
    pub fn ended(&mut self, step: &str, now: i64) {
        let ms = now - self.last;
        if ms > self.longest.ms || self.longest.step.is_empty() {
            self.longest = Step { step: step.into(), ms };
        }
        self.last = now;
    }
}

/// A tick that took past half the hang bound: when it ended, how long it
/// took, ms, and the step it spent most in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlowTick {
    pub at: i64,
    pub took_ms: i64,
    pub longest: Step,
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
    /// Its last tick past half the hang bound.
    #[serde(default)]
    pub slow: Option<SlowTick>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DaemonEvent {
    DaemonStarted { at: i64, path: String, missing: Vec<String> },
    TickFailed { at: i64, error: String },
    TickDone,
    RolledBack { at: i64, rolled_back: RolledBack },
    RollbackOver,
    TickSlow(SlowTick),
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
            DaemonEvent::TickSlow(slow) => s.slow = Some(slow),
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
    /// How long it took, ms, and its longest step, when that was past half the hang bound.
    #[serde(default)]
    pub slow: Option<(i64, Step)>,
}

command!(
    /// A tick's outcome: a failure adds to the run; a success ends it. A slow one is kept.
    NoteTick,
    "daemon.tick",
    Daemons,
    Ticked,
    |_p| daemon_id(),
    |s, p, now| {
        let mut events = match (&p.error, &s.failing) {
            (Some(e), _) => vec![DaemonEvent::TickFailed { at: now.0, error: e.clone() }],
            (None, Some(_)) => vec![DaemonEvent::TickDone],
            (None, None) => Vec::new(),
        };
        if let Some((took_ms, longest)) = &p.slow {
            events.push(DaemonEvent::TickSlow(SlowTick { at: now.0, took_ms: *took_ms, longest: longest.clone() }));
        }
        Ok::<_, factum_core::Rejection>(events)
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
/// the live pid its pid file names, whether its lock is held, and when it
/// last ticked (ms on its clock).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub pid: Option<u32>,
    pub locked: bool,
    pub ticked: Option<i64>,
}

/// How long a daemon may go without a tick before it is taken to hang, ms:
/// five of its ticks, never under two minutes, as one tick fetches every checkout.
pub fn hang_bound(tick_every: i64) -> i64 {
    (5 * tick_every).max(120_000)
}

/// Whether a tick that took `took_ms` is slow: past half the hang bound, so
/// one twice as slow would read as a hung daemon.
pub fn slow(took_ms: i64, tick_every: i64) -> bool {
    took_ms > hang_bound(tick_every) / 2
}

/// Why no daemon runs, if none does: the pid file names no live process
/// that holds its lock, or the one that does has not ticked within the hang
/// bound (hung). One that has written no tick at all is a binary older than the
/// tick file (rolled back to, or not yet swapped): it runs.
pub fn down(p: &Probe, tick_every: i64, now: i64) -> Option<String> {
    let Some(pid) = p.pid else { return Some("its pid file names no live process".into()) };
    if !p.locked {
        return Some(format!("pid {pid}, which its pid file names, does not hold its lock"));
    }
    let silent = now - p.ticked?;
    (silent > hang_bound(tick_every))
        .then(|| format!("pid {pid} holds its lock and has not ticked for {} s: it hangs", silent / 1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_is_down_without_a_live_pid_its_lock_or_a_tick_within_the_bound() {
        let running = Probe { pid: Some(7), locked: true, ticked: Some(0) };
        assert_eq!(down(&running, 10_000, 120_000), None, "the two-minute floor");
        assert!(down(&running, 10_000, 120_001).unwrap().contains("not ticked for 120 s: it hangs"));
        let later = Probe { ticked: Some(5_000), ..running.clone() };
        assert_eq!(down(&later, 10_000, 125_000), None, "silence counts from its last tick");
        assert!(down(&later, 10_000, 125_001).is_some());
        assert_eq!(down(&running, 60_000, 300_000), None, "five ticks, past the floor");
        assert!(down(&Probe::default(), 10_000, 0).unwrap().contains("names no live process"));
        assert!(down(&Probe { locked: false, ..running.clone() }, 10_000, 0).unwrap().contains("its lock"));
        assert_eq!(down(&Probe { ticked: None, ..running }, 10_000, i64::MAX / 2), None, "an older binary runs");
    }

    #[test]
    fn a_ticks_longest_step_is_the_first_that_took_longest() {
        let mut s = Steps::at(100);
        s.ended("a", 100);
        assert_eq!(s.longest, Step { step: "a".into(), ms: 0 }, "a first step, however short");
        s.ended("b", 130);
        s.ended("c", 160);
        s.ended("d", 170);
        assert_eq!(s.longest, Step { step: "b".into(), ms: 30 }, "the first of two as long");
    }

    #[test]
    fn a_tick_past_half_the_hang_bound_is_slow_and_kept_until_a_slower_one() {
        assert!(!slow(60_000, 10_000) && slow(60_001, 10_000), "half the two-minute floor");
        assert!(!slow(150_000, 60_000) && slow(150_001, 60_000), "half of five ticks");
        let mut s = Daemon::default();
        let step = Step { step: "lines".into(), ms: 90_000 };
        let slow = SlowTick { at: 5, took_ms: 95_000, longest: step.clone() };
        Daemons::evolve(&mut s, DaemonEvent::TickSlow(slow.clone()));
        Daemons::evolve(&mut s, DaemonEvent::TickDone);
        assert_eq!(s.slow, Some(slow), "a later tick does not clear it");
    }
}
