//! The heavy-command queue: one host-wide line of the commands that load
//! the machine — every gate run, and what a worker runs through
//! `factorium worker exclusive`. The oldest waiting starts when the host
//! has room, or when nothing runs, so it never starves; one start per
//! reading; and one that holds others up past the hold bound is ended.

use factum_core::{Aggregate, Uuid};
use serde::{Deserialize, Serialize};

use crate::{command, id, no};

pub fn queue_id() -> Uuid {
    id("heavy")
}

/// What a proving line's heavy command is called: `job` (`fast gate`,
/// `slow gate`, `cut`) of `checkout`. Doctor knows a line's move runs by it.
pub fn line_job(checkout: &str, job: &str) -> String {
    format!("{job} of {checkout}")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: Uuid,
    pub what: String,
    /// The process that waits for it, to forget it when that dies.
    pub pid: u32,
    pub worker: Option<Uuid>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Queue {
    pub entries: Vec<Entry>,
    /// When the last one started: one start per reading.
    pub last_start: i64,
}

impl Queue {
    pub fn running(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.started_at.is_some())
    }

    pub fn waiting(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.started_at.is_none())
    }

    /// The waiting entry that starts next.
    pub fn oldest(&self) -> Option<&Entry> {
        self.waiting().min_by_key(|e| (e.queued_at, e.id))
    }

    /// Whether the line job `job` of `checkout` runs now.
    pub fn runs(&self, checkout: &str, job: &str) -> bool {
        let what = line_job(checkout, job);
        self.running().any(|e| e.what == what)
    }

    /// Whether `id` may start, given whether the host has room and when the
    /// reading was taken; else why not.
    pub fn may_start(&self, id: Uuid, room: Result<(), String>, read_at: i64) -> Result<(), String> {
        let oldest = self.oldest();
        match oldest {
            Some(e) if e.id == id => {}
            Some(e) => return Err(format!("waits behind {}", e.what)),
            None => return Err("not waiting".into()),
        }
        if self.running().next().is_none() {
            return Ok(());
        }
        room?;
        if read_at <= self.last_start {
            return Err("one start per reading".into());
        }
        Ok(())
    }

    /// Whether the running `id` holds others up past `bound` and must end.
    pub fn overheld(&self, id: Uuid, now: i64, bound: i64) -> bool {
        let Some(e) = self.entries.iter().find(|e| e.id == id) else { return false };
        let waiting_since = self.waiting().map(|w| w.queued_at).min();
        match (e.started_at, waiting_since) {
            (Some(started), Some(since)) => now - started.max(since) > bound,
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum HeavyEvent {
    HeavyQueued { id: Uuid, what: String, pid: u32, worker: Option<Uuid>, at: i64 },
    HeavyStarted { id: Uuid, at: i64 },
    HeavyEnded { id: Uuid },
}

pub struct Heavy;

impl Aggregate for Heavy {
    const NAME: &'static str = "heavy";
    type State = Queue;
    type Event = HeavyEvent;

    fn evolve(s: &mut Queue, event: HeavyEvent) {
        match event {
            HeavyEvent::HeavyQueued { id, what, pid, worker, at } => {
                s.entries.push(Entry { id, what, pid, worker, queued_at: at, started_at: None })
            }
            HeavyEvent::HeavyStarted { id, at } => {
                if let Some(e) = s.entries.iter_mut().find(|e| e.id == id) {
                    e.started_at = Some(at);
                }
                s.last_start = at;
            }
            HeavyEvent::HeavyEnded { id } => s.entries.retain(|e| e.id != id),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Enqueue {
    pub id: Uuid,
    pub what: String,
    pub pid: u32,
    pub worker: Option<Uuid>,
}

command!(EnqueueHeavy, "heavy.queue", Heavy, Enqueue, |_p| queue_id(), |s, p, now| {
    if s.entries.iter().any(|e| e.id == p.id) {
        return Ok(Vec::new());
    }
    Ok::<_, factum_core::Rejection>(vec![HeavyEvent::HeavyQueued {
        id: p.id,
        what: p.what.clone(),
        pid: p.pid,
        worker: p.worker,
        at: now.0,
    }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Begin {
    pub id: Uuid,
    /// Whether the host had room, and why not.
    pub room: Result<(), String>,
    /// When that reading was taken, ms.
    pub read_at: i64,
}

command!(StartHeavy, "heavy.start", Heavy, Begin, |_p| queue_id(), |s, p, now| {
    match s.may_start(p.id, p.room.clone(), p.read_at) {
        Ok(()) => Ok(vec![HeavyEvent::HeavyStarted { id: p.id, at: now.0 }]),
        Err(why) => no("wait", why),
    }
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct End {
    pub id: Uuid,
}

command!(EndHeavy, "heavy.end", Heavy, End, |_p| queue_id(), |s, p, now| {
    Ok::<_, factum_core::Rejection>(if s.entries.iter().any(|e| e.id == p.id) {
        vec![HeavyEvent::HeavyEnded { id: p.id }]
    } else {
        Vec::new()
    })
});
