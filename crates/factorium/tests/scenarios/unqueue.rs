//! Ending a running heavy command ends what it runs: every process it
//! started, even one that left its process group or its parent, whether the
//! owner unqueues it or the hold bound ends it (factorium#128: four
//! `unqueue`d mutation runs ran on, and their `cargo-mutants` had left the
//! tree).

use crate::fixture::{STUCK, World, entry, escaped, escaping};

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
        let queued = w.until_queued("echo after");
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
