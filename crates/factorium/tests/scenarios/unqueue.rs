//! Ending a running heavy command ends what it runs: every process it
//! started, even one that left its process group or its parent, whether the
//! owner unqueues it or the hold bound ends it (factorium#128: four
//! `unqueue`d mutation runs ran on, and their `cargo-mutants` had left the
//! tree).

use std::path::Path;

use factum::Uuid;

use crate::fixture::{STUCK, World, until_file};

/// A command that starts two processes outside its tree as a process group
/// sees it: one in a process group of its own, and one whose parent exits
/// (reparented to launchd). Each writes its pid to `dir` and sleeps a
/// minute; the command itself holds as [`World::holds`] does.
pub(crate) fn escaping(w: &World, dir: &Path) -> String {
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
pub(crate) struct Escaped([u32; 2]);

impl Escaped {
    /// Those that still run.
    pub(crate) fn left(&self) -> Vec<u32> {
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
pub(crate) async fn escaped(dir: &Path) -> Escaped {
    until_file(&dir.join("grouped")).await;
    until_file(&dir.join("orphan")).await;
    let pid = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap().trim().parse::<u32>().unwrap();
    let (grouped, orphan) = (pid("grouped"), pid("orphan"));
    assert_eq!(ps(grouped, "pgid"), grouped.to_string(), "a process group of its own");
    assert_eq!(ps(orphan, "ppid"), "1", "its parent exited");
    Escaped([grouped, orphan])
}

/// The id of the heavy entry doctor shows as `what`.
pub(crate) async fn entry(w: &World, what: &str) -> Uuid {
    w.queued_entry(what).await["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn unqueue_ends_a_running_entrys_whole_process_tree_and_its_waiter_says_so() {
    let w = World::new();
    let dir = w.dir.join("escaping");
    let line = escaping(&w, &dir);
    let cli = w.run();
    let owner = async {
        let pids = escaped(&dir).await;
        let id = entry(&w, &line).await;
        let unqueued = w.run().unqueue(id).await.unwrap();
        assert_eq!(unqueued["ended"], true, "{unqueued}");
        // Ended by the time unqueue answers, not once its waiter looks.
        assert_eq!(pids.left(), Vec::<u32>::new(), "still running");
    };
    let both = async { tokio::join!(cli.worker_exclusive(None, &dir, &line), owner) };
    let (ran, ()) = tokio::time::timeout(STUCK, both).await.expect("the unqueued command ends");
    let refused = ran.expect_err("an unqueued command is no answer of its own");
    assert_eq!(refused.code, "unqueued");
    assert!(refused.message.contains("unqueued"), "{}", refused.message);
    assert!(w.queued().await.is_empty());
}

#[tokio::test]
async fn the_hold_bound_ends_a_running_entrys_whole_process_tree() {
    let w = World::new();
    // No room for a second heavy command beside it.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let dir = w.dir.join("escaping");
    let line = escaping(&w, &dir);
    let (a, b) = (w.run(), w.run());
    let others = async {
        let pids = escaped(&dir).await;
        let mut waiter = std::pin::pin!(b.worker_exclusive(None, &w.dir, "echo after"));
        let queued = async {
            while !w.queued().await.contains(&"echo after".to_string()) {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! { _ = &mut waiter => panic!("it ran beside the running one"), _ = queued => {} }
        w.advance(31);
        let again = async {
            w.gone(&line).await;
            w.poll(1).await;
        };
        (tokio::join!(waiter, again).0, pids)
    };
    let both = async { tokio::join!(a.worker_exclusive(None, &dir, &line), others) };
    let (ran, (after, pids)) = tokio::time::timeout(STUCK, both).await.expect("the held command ends");
    let ran = ran.unwrap();
    assert_eq!(ran["ok"], false);
    assert!(ran["tail"].as_str().unwrap().contains("held others up past the hold bound"), "{ran}");
    assert_eq!(pids.left(), Vec::<u32>::new(), "still running");
    assert_eq!(after.unwrap()["ok"], true, "the waiting command runs once the held one is ended");
}
