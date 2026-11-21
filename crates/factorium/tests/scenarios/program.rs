//! A program of the owner's that is not a worker (Guild's instances, guildd)
//! runs its heavy commands through the host's one queue (factorium#166):
//! they start by the queue's rule beside workers' and gates', at the
//! priority they state, and the queue's depth reads as JSON.

use factum::Uuid;

use crate::fixture::{POLL, STUCK, World, nice_is, own_nice, until_file};

#[tokio::test]
async fn a_programs_command_waits_its_turn_behind_a_workers_and_runs_at_its_stated_priority() {
    let w = World::new();
    let (busy, log) = (w.dir.join("busy"), w.dir.join("log"));
    let held = w.holds(&busy);
    let by_worker = format!("{} && echo worker >> {}", nice_is("-eq", own_nice()), log.display());
    let by_program = format!("{} && echo program >> {}", nice_is("-gt", own_nice()), log.display());
    // No room beside what runs: one at a time.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let (b, wk, prog) = (w.run(), w.run(), w.run());
    let ran = |n: usize| std::fs::read_to_string(&log).map_or(0, |s| s.lines().count()) >= n;
    let asks = async {
        until_file(&busy.join("started")).await;
        let worker = wk.worker_exclusive(Some(Uuid::now_v7()), &w.dir, &by_worker);
        let program = async {
            // Queued after the worker's, so it waits behind it.
            w.until_queued(&by_worker).await;
            prog.heavy_run(&w.dir, &by_program, true).await
        };
        tokio::join!(worker, program)
    };
    let looks = async {
        w.until(async || w.queued().await.len() >= 3 && w.clock.asleep() == 3).await;
        let depth = w.run().heavy_depth().await.unwrap();
        assert_eq!((depth["running"].as_u64(), depth["waiting"].as_u64()), (Some(1), Some(2)), "{depth}");
        let entries = depth["entries"].as_array().unwrap();
        let of = |what: &str| entries.iter().find(|e| e["what"] == what).unwrap().clone();
        assert_eq!(of(&by_program)["program"], true, "{depth}");
        assert_eq!(of(&by_worker)["program"], false, "{depth}");
        w.host.reading.lock().unwrap().cpu_busy = 10;
        std::fs::write(busy.join("release"), "").unwrap();
        // The waiters ask again as the clock moves, until both ran.
        w.until(async || {
            if !ran(2) && w.clock.asleep() > 0 {
                w.clock.advance(POLL);
            }
            ran(2)
        })
        .await;
    };
    let (held_ran, (worker, program), ()) =
        tokio::time::timeout(STUCK, async { tokio::join!(b.worker_exclusive(None, &busy, &held), asks, looks) })
            .await
            .expect("both run");
    assert_eq!(held_ran.unwrap()["ok"], true);
    assert_eq!(worker.unwrap()["ok"], true, "the worker's at a gate's priority");
    assert_eq!(program.unwrap()["ok"], true, "the program's at background priority, as it stated");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "worker\nprogram\n", "oldest first");
    let depth = w.run().heavy_depth().await.unwrap();
    assert_eq!((depth["running"].as_u64(), depth["waiting"].as_u64()), (Some(0), Some(0)), "{depth}");
}

#[tokio::test]
async fn a_programs_command_stated_at_a_gates_priority_runs_at_it() {
    let w = World::new();
    let dir = w.dir.clone();
    let ran = w.app.heavy_run(&dir, &nice_is("-eq", own_nice()), false).await.unwrap();
    assert_eq!(ran["ok"], true, "{ran}");
}
