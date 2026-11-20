//! Time, as the app reads it and waits on it: the system's, or a clock only
//! the program moves (a scenario's), whose waits end when it is moved past
//! them and never on real time. Everything the app times goes through one:
//! the engine's events, the heavy queue's polls, the daemon's wait between
//! ticks, idle work's limit and the dates of the commits it makes.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// A wait on a clock.
pub type Sleep<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Milliseconds since the Unix epoch.
    fn now(&self) -> i64;
    /// Ends once this clock reads `ms` later than it does now.
    fn sleep(&self, ms: u64) -> Sleep<'_>;
}

/// The system's clock.
#[derive(Debug)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        since.map_or(0, |d| d.as_millis() as i64)
    }

    fn sleep(&self, ms: u64) -> Sleep<'_> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_millis(ms)))
    }
}

/// A clock only the program moves: a scenario advances it, and the waits it
/// passes end.
#[derive(Debug)]
pub struct ManualClock {
    at: tokio::sync::watch::Sender<i64>,
    /// The waits not yet ended, by when each ends: how many of each.
    waits: Arc<Mutex<BTreeMap<i64, usize>>>,
    /// Told each time a wait begins or ends, or the clock moves.
    changed: Arc<tokio::sync::Notify>,
}

impl ManualClock {
    pub fn new(at: i64) -> ManualClock {
        ManualClock { at: tokio::sync::watch::Sender::new(at), waits: Default::default(), changed: Default::default() }
    }

    pub fn advance(&self, ms: i64) {
        self.at.send_modify(|t| *t += ms);
        self.changed.notify_waiters();
    }

    /// How many waits are asleep: not yet ended, and not yet passed. Once
    /// every waiter a scenario started is asleep, each has done what the
    /// time it was woken at gave it to do.
    pub fn asleep(&self) -> usize {
        let now = self.now();
        self.waits.lock().unwrap().range(now + 1..).map(|(_, n)| n).sum()
    }

    /// Ends once [`ManualClock::asleep`] may answer otherwise than when this
    /// was called: a wait began or ended, or the clock moved.
    pub fn changed(&self) -> Sleep<'_> {
        let mut told = Box::pin(self.changed.notified());
        told.as_mut().enable();
        told
    }
}

impl Clock for ManualClock {
    fn now(&self) -> i64 {
        *self.at.borrow()
    }

    fn sleep(&self, ms: u64) -> Sleep<'_> {
        let until = self.now().saturating_add(ms as i64);
        *self.waits.lock().unwrap().entry(until).or_default() += 1;
        self.changed.notify_waiters();
        let gone = Gone { waits: self.waits.clone(), changed: self.changed.clone(), until };
        let mut at = self.at.subscribe();
        Box::pin(async move {
            let _gone = gone;
            let _ = at.wait_for(|t| *t >= until).await;
        })
    }
}

/// Takes a wait off the count when it ends or is dropped.
struct Gone {
    waits: Arc<Mutex<BTreeMap<i64, usize>>>,
    changed: Arc<tokio::sync::Notify>,
    until: i64,
}

impl Drop for Gone {
    fn drop(&mut self) {
        let mut waits = self.waits.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = waits.get_mut(&self.until) {
            *n -= 1;
            if *n == 0 {
                waits.remove(&self.until);
            }
        }
        drop(waits);
        self.changed.notify_waiters();
    }
}
