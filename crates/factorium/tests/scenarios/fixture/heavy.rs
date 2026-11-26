//! Steps of the heavy-command queue: another CLI run holding it, a command
//! whose asker went away, moving the clock a poll on, and a command whose
//! processes escape its tree.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use factum::Uuid;

use super::World;

/// A hang guard, not a timing bound: no scenario waits on real time (the
/// app's clock is the scenario's), so what is left is the real work of git,
/// sqlite and the fixtures' processes: the slowest scenario took 3.3s at
/// load 22 (factorium#119), 17s under 36 CPU hogs on 18 cores before it.
/// Idle work runs the suite niced and under coverage beside the workers'
/// gates: there scenarios ran 10-28x slower than in a worker's coverage run,
/// one of 1.4s took 35s (2026-11-26), so the guard leaves room for that.
/// It fails only on a hang, such as a waiter asleep on a clock no one moves.
/// How fast the suite runs is the gate's bound to keep.
pub const STUCK: Duration = Duration::from_secs(120);

/// How often a heavy command waiting for its turn asks again, and a running
/// one whether it holds others up: ms on the app's clock (`heavy.rs`).
pub const POLL: i64 = 500;

/// How often a scenario looks again at what nothing tells it of: a file a
/// fixture's process writes, the queue another run changes, a child's exit. Real time,
/// but a sleep, not a spin: what a look costs does not grow with how long a
/// loaded host leaves the scenario waiting (factorium#155).
pub const LOOK: Duration = Duration::from_millis(10);

/// What `comes` answers; past [`STUCK`], a hang, it fails, as `what` never came.
pub async fn bounded<T>(what: &str, comes: impl Future<Output = T>) -> T {
    tokio::time::timeout(STUCK, comes).await.unwrap_or_else(|_| panic!("{what} never came"))
}

/// Until `path` is there; past [`STUCK`], a hang, it fails.
pub async fn until_file(path: &Path) {
    let there = async {
        while !path.exists() {
            tokio::time::sleep(LOOK).await;
        }
    };
    bounded(&path.display().to_string(), there).await
}

impl World {
    /// Once `n` waits are asleep on the world's clock (each waiting or running
    /// heavy command has done what the time it woke at gave it to do), moves
    /// the clock a poll on: each asks again.
    pub async fn poll(&self, n: usize) {
        self.until_or(&format!("{n} waits asleep on the clock"), async || self.clock.asleep() == n).await;
        self.clock.advance(POLL);
    }

    /// Until `done`: looked at again each time the world's clock tells of a
    /// wait that began or ended, and every [`LOOK`] besides, for what moves
    /// without the clock (another run's entry leaving the queue). Past
    /// [`STUCK`], a hang, it fails, naming the line that waited.
    #[track_caller]
    pub fn until(&self, done: impl AsyncFnMut() -> bool) -> impl Future<Output = ()> {
        let at = std::panic::Location::caller();
        async move { self.until_or(&format!("the wait at {at}"), done).await }
    }

    /// [`World::until`], failing past [`STUCK`] as `what` never came.
    async fn until_or(&self, what: &str, mut done: impl AsyncFnMut() -> bool) {
        let comes = async {
            loop {
                let changed = self.clock.changed();
                if done().await {
                    return;
                }
                tokio::select! { () = changed => {}, () = tokio::time::sleep(LOOK) => {} }
            }
        };
        bounded(what, comes).await
    }

    /// Until the heavy command `what` has left the queue.
    pub async fn gone(&self, what: &str) {
        let left = format!("`{what}` leaving the queue");
        self.until_or(&left, async || !self.queued().await.iter().any(|h| h == what)).await;
    }

    /// Until the heavy command `what` is in the queue.
    pub async fn until_queued(&self, what: &str) {
        self.until_or(&format!("`{what}` queued"), async || self.queued().await.iter().any(|h| h == what)).await;
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

/// A command that starts two processes outside its tree as a process group
/// sees it: one in a process group of its own, and one whose parent exits
/// (reparented to launchd). Each writes its pid to `dir` and sleeps a
/// minute; the command itself holds as [`World::holds`] does.
pub fn escaping(w: &World, dir: &Path) -> String {
    let holds = w.holds(dir);
    let d = dir.display();
    format!(
        "perl -e 'setpgrp(0, 0); open my $f, \">\", \"{d}/g.tmp\"; print $f $$; close $f; \
         rename \"{d}/g.tmp\", \"{d}/grouped\"; exec \"sleep\", \"60\"' & \
         ( sh -c 'echo $$ > {d}/o.tmp; mv {d}/o.tmp {d}/orphan; exec sleep 60' & ); {holds}"
    )
}

/// What `ps` says of `pid`: its `field`, empty once it is gone.
fn ps(pid: u32, field: &str) -> String {
    let out = std::process::Command::new("ps").args(["-o", &format!("{field}="), "-p", &pid.to_string()]).output();
    String::from_utf8_lossy(&out.unwrap().stdout).trim().to_string()
}

/// Whether `pid` still runs: not gone, nor a zombie waiting to be reaped.
fn runs(pid: u32) -> bool {
    let stat = ps(pid, "stat");
    !stat.is_empty() && !stat.starts_with('Z')
}

/// The escaped processes, killed when dropped: a failing scenario leaves
/// none behind.
pub struct Escaped([u32; 2]);

impl Escaped {
    /// Those that still run.
    pub fn left(&self) -> Vec<u32> {
        self.0.into_iter().filter(|p| runs(*p)).collect()
    }
}

impl Drop for Escaped {
    fn drop(&mut self) {
        let pids: Vec<String> = self.0.iter().map(u32::to_string).collect();
        let _ =
            std::process::Command::new("kill").arg("-KILL").args(&pids).stderr(std::process::Stdio::null()).status();
    }
}

/// Once the command of [`escaping`] started both: their pids, each checked
/// to have escaped as it means to.
pub async fn escaped(dir: &Path) -> Escaped {
    until_file(&dir.join("grouped")).await;
    until_file(&dir.join("orphan")).await;
    let pid = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap().trim().parse::<u32>().unwrap();
    let (grouped, orphan) = (pid("grouped"), pid("orphan"));
    assert_eq!(ps(grouped, "pgid"), grouped.to_string(), "a process group of its own");
    assert_eq!(ps(orphan, "ppid"), "1", "its parent exited");
    Escaped([grouped, orphan])
}

/// The id of the heavy entry doctor shows as `what`.
pub async fn entry(w: &World, what: &str) -> Uuid {
    w.queued_entry(what).await["id"].as_str().unwrap().parse().unwrap()
}
