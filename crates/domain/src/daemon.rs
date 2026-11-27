//! The daemon as the database knows it: the programs it could not find when
//! it started, the run of ticks that failed in a row, and its last tick that
//! took past half the hang bound. A daemon that
//! cannot do its work is a finding, not a line in its log.

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use crate::needs::Lacking;
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

/// A binary Homebrew links that the daemon does not swap to, as its proof
/// failed (docs/adr/0030): its version, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Refused {
    pub version: String,
    pub why: String,
}

/// The daemon's last try to install the newest version of its own formula
/// from the tap, on a host that does not cut it (factorium#269): when it
/// ended, and why it failed, if it did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Upgrade {
    pub at: i64,
    pub failed: Option<String>,
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

/// How many quick ticks in a row after a slow one resolve it: ticks are
/// slow no longer.
pub const QUICK_TICKS_RESOLVE: u32 = 5;

/// A tick that took past half the hang bound: when it ended, how long it
/// took, ms, the step it spent most in, and the quick ticks in a row since,
/// counted up to [`QUICK_TICKS_RESOLVE`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlowTick {
    pub at: i64,
    pub took_ms: i64,
    pub longest: Step,
    #[serde(default)]
    pub quick_since: u32,
}

impl SlowTick {
    /// Whether ticks are still slow: fewer than [`QUICK_TICKS_RESOLVE`] quick ones since.
    pub fn stands(&self) -> bool {
        self.quick_since < QUICK_TICKS_RESOLVE
    }
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
    pub rolled_back: Option<RolledBack>,
    /// Its last tick past half the hang bound.
    pub slow: Option<SlowTick>,
    /// The linked binary it does not swap to, until a link names another.
    pub refused: Option<Refused>,
    /// Its last upgrade from the tap.
    pub upgrade: Option<Upgrade>,
    /// Each role directory whose last check before a start found it lacking
    /// what its workers need (factorium#277), by its path: its issues wait.
    #[serde(default)]
    pub roles: BTreeMap<String, Lacking>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DaemonEvent {
    DaemonStarted {
        at: i64,
        path: String,
        missing: Vec<String>,
    },
    TickFailed {
        at: i64,
        error: String,
    },
    TickDone,
    RolledBack {
        at: i64,
        rolled_back: RolledBack,
    },
    RollbackOver,
    TickSlow(SlowTick),
    /// A tick under half the hang bound, after a slow one that still stands.
    TickQuick,
    SwapRefused(Option<Refused>),
    UpgradeTried(Upgrade),
    /// A role directory was checked.
    RoleChecked(RoleChecked),
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
            DaemonEvent::TickQuick => s.slow.iter_mut().for_each(|t| t.quick_since += 1),
            DaemonEvent::SwapRefused(r) => s.refused = r,
            DaemonEvent::UpgradeTried(u) => s.upgrade = Some(u),
            DaemonEvent::RoleChecked(RoleChecked { dir, lacking: Some(l) }) => _ = s.roles.insert(dir, l),
            DaemonEvent::RoleChecked(RoleChecked { dir, lacking: None }) => _ = s.roles.remove(&dir),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    pub path: String,
    pub missing: Vec<String>,
}

command!(StartDaemon, "daemon.start", Daemons, Start, |_p| daemon_id(), |s, p, now| {
    Ok(vec![DaemonEvent::DaemonStarted { at: now.0, path: p.path.clone(), missing: p.missing.clone() }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ticked {
    /// Why the tick failed; `None`: it did its work.
    pub error: Option<String>,
    /// How long it took, ms, and its longest step, when that was past half the hang bound.
    pub slow: Option<(i64, Step)>,
}

// A tick's outcome: a failure adds to the run; a success ends it. A slow one is kept;
// a quick one counts toward resolving the last slow one, until it does.
command!(NoteTick, "daemon.tick", Daemons, Ticked, |_p| daemon_id(), |s, p, now| {
    let mut events = match (&p.error, &s.failing) {
        (Some(e), _) => vec![DaemonEvent::TickFailed { at: now.0, error: e.clone() }],
        (None, Some(_)) => vec![DaemonEvent::TickDone],
        (None, None) => Vec::new(),
    };
    match &p.slow {
        Some((took_ms, longest)) => events.push(DaemonEvent::TickSlow(SlowTick {
            at: now.0,
            took_ms: *took_ms,
            longest: longest.clone(),
            quick_since: 0,
        })),
        None if s.slow.as_ref().is_some_and(SlowTick::stands) => events.push(DaemonEvent::TickQuick),
        None => {}
    }
    Ok(events)
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rollback {
    /// The rollback the kept binary ticked well after; `None`: a binary
    /// Homebrew links ticked well, and the last rollback is over.
    pub rolled_back: Option<RolledBack>,
}

command!(NoteRollback, "daemon.rollback", Daemons, Rollback, |_p| daemon_id(), |s, p, now| {
    Ok(match (&p.rolled_back, &s.rolled_back) {
        (Some(r), _) => vec![DaemonEvent::RolledBack { at: now.0, rolled_back: r.clone() }],
        (None, Some(_)) => vec![DaemonEvent::RollbackOver],
        (None, None) => Vec::new(),
    })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Swap {
    /// The linked binary whose proof failed; `None`: the link names this
    /// binary's version, or one that proved.
    pub refused: Option<Refused>,
}

command!(NoteSwap, "daemon.swap", Daemons, Swap, |_p| daemon_id(), |s, p, now| {
    crate::only(p.refused != s.refused, DaemonEvent::SwapRefused(p.refused.clone()))
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Upgraded {
    /// Why the upgrade from the tap failed; `None`: brew upgraded, or had nothing newer.
    pub failed: Option<String>,
}

command!(NoteUpgrade, "daemon.upgrade", Daemons, Upgraded, |_p| daemon_id(), |s, p, now| {
    Ok(vec![DaemonEvent::UpgradeTried(Upgrade { at: now.0, failed: p.failed.clone() })])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoleChecked {
    pub dir: String,
    /// What it lacks; `None`: nothing.
    pub lacking: Option<Lacking>,
}

// A role directory's check before a start: kept only when it changes what is known of it.
command!(NoteRole, "daemon.role", Daemons, RoleChecked, |_p| daemon_id(), |s, p, _now| {
    let changed = s.roles.get(&p.dir) != p.lacking.as_ref();
    crate::only(changed, DaemonEvent::RoleChecked(p.clone()))
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
