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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Daemon {
    /// When it last started.
    pub started_at: i64,
    /// The PATH it started with.
    pub path: String,
    /// The programs it needs and did not find on that PATH.
    pub missing: Vec<String>,
    pub failing: Option<Failing>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DaemonEvent {
    DaemonStarted { at: i64, path: String, missing: Vec<String> },
    TickFailed { at: i64, error: String },
    TickDone,
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
