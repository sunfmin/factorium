//! Steps of the heavy-command queue: another CLI run holding it, a command
//! whose asker went away, and moving the clock a poll on.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use super::World;

/// A hang guard, not a timing bound: no scenario waits on real time (the
/// app's clock is the scenario's), so what is left is the real work of git,
/// sqlite and the fixtures' processes: the slowest scenario took 3.3s at
/// load 22 (factorium#119), 17s under 36 CPU hogs on 18 cores before it.
/// It fails only on a hang, such as a waiter asleep on a clock no one moves.
/// How fast the suite runs is the gate's bound to keep.
pub const STUCK: Duration = Duration::from_secs(30);

/// How often a heavy command waiting for its turn asks again, and a running
/// one whether it holds others up: ms on the app's clock (`heavy.rs`).
pub const POLL: i64 = 500;

/// How often a scenario looks again at what nothing tells it of: a file a
/// fixture's process writes, the queue another run changes, a child's exit. Real time,
/// but a sleep, not a spin: what a look costs does not grow with how long a
/// loaded host leaves the scenario waiting (factorium#155).
pub const LOOK: Duration = Duration::from_millis(10);

/// Until `path` is there.
pub async fn until_file(path: &Path) {
    while !path.exists() {
        tokio::time::sleep(LOOK).await;
    }
}

impl World {
    /// Once `n` waits are asleep on the world's clock (each waiting or running
    /// heavy command has done what the time it woke at gave it to do), moves
    /// the clock a poll on: each asks again.
    pub async fn poll(&self, n: usize) {
        self.until(async || self.clock.asleep() == n).await;
        self.clock.advance(POLL);
    }

    /// Until `done`: looked at again each time the world's clock tells of a
    /// wait that began or ended, and every [`LOOK`] besides, for what moves
    /// without the clock (another run's entry leaving the queue).
    pub async fn until(&self, mut done: impl AsyncFnMut() -> bool) {
        loop {
            let changed = self.clock.changed();
            if done().await {
                return;
            }
            tokio::select! { () = changed => {}, () = tokio::time::sleep(LOOK) => {} }
        }
    }

    /// Until the heavy command `what` has left the queue.
    pub async fn gone(&self, what: &str) {
        self.until(async || !self.queued().await.iter().any(|h| h == what)).await;
    }

    /// Until the heavy command `what` is in the queue.
    pub async fn until_queued(&self, what: &str) {
        self.until(async || self.queued().await.iter().any(|h| h == what)).await;
    }

    /// Runs `during` while another CLI run's heavy command holds the queue, then
    /// lets that command end.
    pub async fn while_busy<T>(&self, during: impl Future<Output = T>) -> T {
        let dir = self.dir.join("busy");
        let line = &self.holds(&dir);
        let cli = self.run();
        // No room for a second heavy command beside it.
        self.host.reading.lock().unwrap().cpu_busy = 99;
        let body = async {
            until_file(&dir.join("started")).await;
            let t = during.await;
            self.host.reading.lock().unwrap().cpu_busy = 10;
            std::fs::write(dir.join("release"), "").unwrap();
            t
        };
        let both = async { tokio::join!(cli.worker_exclusive(None, &dir, line), body) };
        let (held, t) = tokio::time::timeout(STUCK, both).await.expect("nothing waits on the busy queue");
        assert_eq!(held.unwrap()["ok"], true);
        t
    }

    /// Stages a heavy command that started and whose asker went away: its
    /// entry runs on, and no hold-bound check ends it.
    pub async fn running_on(&self) {
        let started = self.dir.join("running-on");
        let cli = self.run();
        let line = format!("touch {}; sleep 1", started.display());
        let mut runs = std::pin::pin!(cli.worker_exclusive(None, &self.dir, &line));
        tokio::select! { _ = &mut runs => panic!("it ended before its asker went"), _ = until_file(&started) => {} }
    }
}
