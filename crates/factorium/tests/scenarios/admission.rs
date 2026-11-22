//! Admission, the heavy-command queue, hot workers, failed sessions, and the
//! disk floor: decided from the host's readings alone.

use std::path::PathBuf;

use factorium::host::Proc;
use serde_json::json;

use crate::fixture::{CLAUDE, GIB, STUCK, World, id, until_file, worker};

async fn ready(w: &World) {
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
}

#[tokio::test]
async fn nothing_starts_while_the_host_has_no_room_and_a_long_hold_goes_to_the_watch() {
    let w = World::new();
    ready(&w).await;
    w.host.reading.lock().unwrap().cpu_busy = 90;
    let t = w.tick().await;
    assert!(t.started.is_none());
    assert!(t.held.unwrap().starts_with("cpu"));
    w.advance(29);
    w.tick().await;
    assert_eq!(w.findings().await, Vec::<serde_json::Value>::new(), "not held long yet");
    w.advance(2);
    w.tick().await;
    let found = w.findings().await[0]["what"].as_str().unwrap().to_string();
    assert!(found.contains("admission held") && found.contains("cpu"), "{found}");

    w.host.reading.lock().unwrap().cpu_busy = 20;
    assert!(w.tick().await.started.is_some());
    assert_eq!(w.findings().await, Vec::<serde_json::Value>::new(), "the hold ended");
}

#[tokio::test]
async fn memory_disk_and_the_workers_share_each_hold_admission() {
    let w = World::new();
    ready(&w).await;
    w.host.reading.lock().unwrap().memory_free = 5;
    assert!(w.tick().await.held.unwrap().starts_with("memory"));
    *w.host.reading.lock().unwrap() =
        factorium::host::Reading { cpu_busy: 10, memory_free: 60, disk_free: 5 * GIB, disk_total: 100 * GIB };
    assert!(w.tick().await.held.unwrap().starts_with("disk"));
    w.host.reading.lock().unwrap().disk_free = 500 * GIB;
    let cores = std::thread::available_parallelism().unwrap().get() as f64;
    *w.host.procs.lock().unwrap() =
        vec![Proc { pid: 9, cpu: 90.0 * cores, worker: Some("x".into()), ..Default::default() }];
    assert!(w.tick().await.held.unwrap().starts_with("workers"));
}

#[tokio::test]
async fn one_worker_starts_per_tick_and_an_empty_kit_starts_none() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    assert!(w.tick().await.held.unwrap().starts_with("kit"));
    w.kit().await;
    assert!(w.tick().await.started.is_some());
    assert_eq!(w.workers().await.len(), 1);
    assert!(w.tick().await.started.is_some());
    assert_eq!(w.workers().await.len(), 2);
}

#[tokio::test]
async fn a_worker_over_its_share_long_enough_is_hot_with_its_evidence() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    let cores = std::thread::available_parallelism().unwrap().get() as f64;
    let tag = Some(wk["worker"].as_str().unwrap().to_string());
    *w.host.procs.lock().unwrap() =
        vec![Proc { pid: 77, ppid: 1, cpu: 60.0 * cores, rss_kib: 10, command: "cargo build".into(), worker: tag }];
    w.host.reading.lock().unwrap().cpu_busy = 95;
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "over, but not for long");
    w.advance(3);
    w.tick().await;
    let hot = worker(&w.app).await;
    let why = hot["why"].as_str().unwrap();
    assert!(hot["status"] == "waiting" && why.starts_with("hot: cpu"), "{hot}");
    let evidence = PathBuf::from(why.rsplit("evidence in ").next().unwrap());
    assert!(std::fs::read_to_string(evidence).unwrap().contains("cargo build"));
}

#[tokio::test]
async fn what_a_worker_runs_in_its_heavy_slot_does_not_count_against_its_share() {
    let w = World::new();
    ready(&w).await;
    w.tick().await;
    // One worker: admission would start the other issue beside its slot's load.
    w.app.issue_wait_for_human("web", 2, "one worker here").await.unwrap();
    let wk = worker(&w.app).await;
    let cores = std::thread::available_parallelism().unwrap().get() as f64;
    let tag = Some(wk["worker"].as_str().unwrap().to_string());
    // The worker's `worker exclusive` is this process; its command runs under it.
    let mutants = Proc {
        pid: 77,
        ppid: std::process::id(),
        cpu: 90.0 * cores,
        rss_kib: 10,
        command: "cargo mutants".into(),
        worker: tag,
    };
    let dir = w.dir.join("slot");
    let line = &w.holds(&dir);
    let body = async {
        until_file(&dir.join("started")).await;
        *w.host.procs.lock().unwrap() = vec![mutants.clone()];
        w.host.reading.lock().unwrap().cpu_busy = 95;
        w.tick().await;
        w.advance(3);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "its load is the heavy slot's");
        std::fs::write(dir.join("release"), "").unwrap();
    };
    let cli = w.run();
    let ran = async { tokio::join!(cli.worker_exclusive(Some(id(&wk)), &dir, line), body) };
    let (held, ()) = tokio::time::timeout(STUCK, ran).await.expect("the slot's command ends");
    assert_eq!(held.unwrap()["ok"], true);
    // The same load outside the slot is the worker's own.
    w.tick().await;
    w.advance(3);
    w.tick().await;
    let hot = worker(&w.app).await;
    assert!(hot["status"] == "waiting" && hot["why"].as_str().unwrap().starts_with("hot: cpu"), "{hot}");
}

#[tokio::test]
async fn load_the_heavy_queue_granted_does_not_hold_admission_and_load_outside_it_does() {
    let w = World::new();
    ready(&w).await;
    let cores = std::thread::available_parallelism().unwrap().get() as f64;
    // A sweep's shard: under the `worker exclusive` that holds the slot, this process.
    let sweep = Proc {
        pid: 77,
        ppid: std::process::id(),
        cpu: 90.0 * cores,
        command: "cargo mutants".into(),
        ..Default::default()
    };
    let dir = w.dir.join("slot");
    let line = &w.holds(&dir);
    let body = async {
        until_file(&dir.join("started")).await;
        *w.host.procs.lock().unwrap() = vec![sweep.clone()];
        w.host.reading.lock().unwrap().cpu_busy = 95;
        let t = w.tick().await;
        assert!(t.started.is_some(), "the slot's load is the queue's to bound: {:?}", t.held);
        let doctor = w.app.doctor().await.unwrap();
        assert_eq!(doctor["reading"]["cpu_busy"], 5, "doctor reads as admission does: {doctor}");
        std::fs::write(dir.join("release"), "").unwrap();
    };
    let cli = w.run();
    let ran = async { tokio::join!(cli.worker_exclusive(None, &dir, line), body) };
    let (held, ()) = tokio::time::timeout(STUCK, ran).await.expect("the slot's command ends");
    assert_eq!(held.unwrap()["ok"], true);
    // The same load outside any slot holds admission.
    *w.host.procs.lock().unwrap() = vec![Proc { ppid: 1, ..sweep }];
    let t = w.tick().await;
    assert!(t.started.is_none() && t.held.as_deref().unwrap().starts_with("cpu: 95%"), "{:?}", t.held);
    let doctor = w.app.doctor().await.unwrap();
    assert_eq!(doctor["reading"]["cpu_busy"], 95, "doctor reads as admission does: {doctor}");
}

/// Seen 2026-11-21 (factorium#194): one entry ran, twelve waited, and
/// admission went on starting workers. The queue starts beside what runs
/// only while the host has room, so it held the host's reading near the
/// busy limit; admission's reading leaves out the running slot, so it read
/// room each time the host dipped under it, and every new worker's heavy
/// commands only waited behind the rest (guild#29 72 minutes).
#[tokio::test]
async fn a_heavy_command_waiting_past_the_queue_wait_bound_holds_admission() {
    let w = World::new();
    ready(&w).await;
    let cores = std::thread::available_parallelism().unwrap().get() as f64;
    // The running slot's load, under the `worker exclusive` that holds it (this process).
    let slot = Proc { pid: 77, ppid: std::process::id(), cpu: 90.0 * cores, ..Default::default() };
    let dir = w.dir.join("slot");
    let line = &w.holds(&dir);
    let (holder, waiter) = (w.run(), w.run());
    let body = async {
        until_file(&dir.join("started")).await;
        *w.host.procs.lock().unwrap() = vec![slot.clone()];
        // No room beside it: the next one waits.
        w.host.reading.lock().unwrap().cpu_busy = 95;
        let mut next = std::pin::pin!(waiter.worker_exclusive(None, &w.dir, "echo next"));
        tokio::select! { _ = &mut next => panic!("it started beside a busy host"), () = w.until_queued("echo next") => {} }
        // Waiting up to the bound is no backlog.
        w.clock.advance(300_000);
        let t = w.tick().await;
        assert!(t.started.is_some(), "waiting the bound is no backlog: {:?}", t.held);
        w.clock.advance(1_000);
        let t = w.tick().await;
        let why = t.held.as_deref().unwrap_or_default().to_string();
        assert!(t.started.is_none() && why.starts_with("queue: `echo next` has waited 301s"), "{why}");
        // The slot ends; the waiting one runs; admission opens again.
        std::fs::write(dir.join("release"), "").unwrap();
        w.gone(line).await;
        // The waiter asks again a poll on: with nothing running, it starts.
        let ran = async { tokio::join!(next, w.poll(1)).0 };
        assert_eq!(ran.await.unwrap()["ok"], true);
        *w.host.procs.lock().unwrap() = vec![];
        w.host.reading.lock().unwrap().cpu_busy = 10;
        assert!(w.tick().await.started.is_some(), "nothing waits: admission opens");
    };
    let ran = async { tokio::join!(holder.worker_exclusive(None, &dir, line), body) };
    let (held, ()) = tokio::time::timeout(STUCK, ran).await.expect("both commands end");
    assert_eq!(held.unwrap()["ok"], true);
}

#[tokio::test]
async fn a_worker_s_hot_clock_survives_a_restart_and_stops_once_it_is_back_within_its_share() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    let cores = std::thread::available_parallelism().unwrap().get() as f64;
    let tag = Some(wk["worker"].as_str().unwrap().to_string());
    let busy = Proc { pid: 77, ppid: 1, cpu: 60.0 * cores, rss_kib: 10, command: "cargo build".into(), worker: tag };
    let idle = Proc { cpu: 0.0, ..busy.clone() };
    *w.host.procs.lock().unwrap() = vec![busy.clone()];
    w.host.reading.lock().unwrap().cpu_busy = 95;
    w.tick().await;
    *w.host.procs.lock().unwrap() = vec![idle];
    w.advance(3);
    w.tick().await;
    *w.host.procs.lock().unwrap() = vec![busy];
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "its clock started again when it went over again");
    // The daemon restarts: a new run on the same database keeps the clock.
    let again = w.run();
    w.advance(3);
    again.tick().await.unwrap();
    let hot = worker(&again).await;
    assert!(hot["status"] == "waiting" && hot["why"].as_str().unwrap().starts_with("hot: cpu"), "{hot}");
}

#[tokio::test]
async fn a_worker_whose_terminal_is_gone_waits_for_the_watch() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    w.kill_terminal(wk["terminal"].as_str().unwrap());
    w.tick().await;
    let gone = worker(&w.app).await;
    assert_eq!(
        (gone["status"].clone(), gone["why"].clone()),
        (json!("waiting"), json!("its session's terminal is gone"))
    );
}

#[tokio::test]
async fn a_heavy_command_runs_with_its_exit_code_and_leaves_the_queue_empty() {
    let w = World::new();
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let dir = w.dir.clone();
    let ok = w.app.worker_exclusive(None, &dir, "echo built").await.unwrap();
    assert_eq!(
        (ok["ok"].clone(), ok["code"].clone()),
        (json!(true), json!(0)),
        "with nothing running it starts, room or not"
    );
    assert!(ok["tail"].as_str().unwrap().contains("built"));
    let bad = w.app.worker_exclusive(None, &dir, "exit 3").await.unwrap();
    assert_eq!(bad["code"], 3);
}

#[tokio::test]
async fn a_working_worker_with_an_open_turn_is_not_silent_and_one_idle_past_the_bound_is() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    let now = (w.app_now() / 1000) as u64;
    w.claude_session(&cwd, true, now);
    w.advance(45);
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "its turn is open");
    w.claude_session(&cwd, false, now + 60);
    w.tick().await;
    let silent = w.worker_on("web", 1).await;
    assert!(silent["why"].as_str().unwrap().starts_with("silent"), "{silent}");
}

/// A checkout, a kit and one worker at work; answers its terminal.
async fn one_working(w: &World) -> String {
    w.working(CLAUDE, "a").await["terminal"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn orca_failing_to_answer_is_no_gone_terminal() {
    let w = World::new();
    let terminal = one_working(&w).await;
    *w.host.orca.down.lock().unwrap() = Some("orca: the runtime is not running".into());
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "nothing is known of its terminal");
    *w.host.orca.down.lock().unwrap() = None;
    w.kill_terminal(&terminal);
    w.tick().await;
    assert_eq!(worker(&w.app).await["why"], "its session's terminal is gone", "Orca says it exited");
}

#[tokio::test]
async fn a_terminal_orca_no_longer_knows_is_gone() {
    let w = World::new();
    let terminal = one_working(&w).await;
    // Orca restarted: the handle is stale.
    w.host.orca.terminals.lock().unwrap().remove(&terminal);
    w.tick().await;
    let gone = worker(&w.app).await;
    assert_eq!(
        (gone["status"].clone(), gone["why"].clone()),
        (json!("waiting"), json!("its session's terminal is gone"))
    );
}

#[tokio::test]
async fn settings_list_each_with_its_unit_refuse_an_unknown_name_and_bound_in_their_unit() {
    let w = World::new();
    let all = w.app.settings_set(None, None).await.unwrap();
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 26, "every setting is listed");
    let held = all.iter().find(|s| s["name"] == "held_for").unwrap();
    assert_eq!((&held["unit"], &held["default"], &held["value"]), (&json!("seconds"), &json!(1800), &json!(1800)));
    let cpu = all.iter().find(|s| s["name"] == "cpu_busy_max").unwrap();
    assert_eq!(cpu["unit"], "percent");

    for (name, value) in [("held_fr", None), ("held_fr", Some(1))] {
        let no = w.app.settings_set(Some(name), value).await.unwrap_err();
        assert_eq!(no.code, "no_setting", "a typo is refused, not a panic");
    }

    // A time set in seconds bounds in seconds; a share in percent.
    ready(&w).await;
    w.app.settings_set(Some("held_for"), Some(120)).await.unwrap();
    w.host.reading.lock().unwrap().cpu_busy = 90;
    assert!(w.tick().await.started.is_none());
    w.advance(1);
    w.tick().await;
    assert_eq!(w.findings().await, Vec::<serde_json::Value>::new(), "held 60 of 120 seconds");
    w.advance(1);
    w.tick().await;
    let found = w.findings().await[0]["what"].as_str().unwrap().to_string();
    assert!(found.contains("admission held"), "{found}");
    w.app.settings_set(Some("cpu_busy_max"), Some(95)).await.unwrap();
    assert!(w.tick().await.started.is_some(), "90% is under 95%");
}

#[tokio::test]
async fn silence_counts_from_the_end_of_the_last_turn() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    w.advance(20);
    let closed = (w.app_now() / 1000) as u64;
    w.claude_session(&cwd, false, closed);
    w.advance(30);
    w.tick().await;
    let it = w.worker_on("web", 1).await;
    assert_eq!(it["status"], "working", "30 minutes since its turn ended: at the bound");
    w.advance(1);
    w.tick().await;
    let silent = w.worker_on("web", 1).await;
    assert!(silent["why"].as_str().unwrap().starts_with("silent for 31 min"), "{silent}");
}

#[tokio::test]
async fn a_worker_whose_heavy_entry_waits_or_runs_is_not_silent_until_the_bound_after_it_ended() {
    let w = World::new();
    // A run that takes longer than the silent bound, within the hold bound.
    w.app.settings_set(Some("hold_bound"), Some(7200)).await.unwrap();
    let wk = w.working(CLAUDE, "a").await;
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    let closed = (w.app_now() / 1000) as u64;
    w.claude_session(&cwd, false, closed);
    let (first, mine) = (w.dir.join("first"), w.dir.join("mutants"));
    let (first_line, line) = (&w.holds(&first), &w.holds(&mine));
    let cli = w.run();
    let queued = async {
        until_file(&first.join("started")).await;
        w.host.reading.lock().unwrap().cpu_busy = 99;
        cli.worker_exclusive(Some(id(&wk)), &mine, line).await
    };
    let body = async {
        until_file(&first.join("started")).await;
        w.until(async || w.heavy().await.iter().any(|e| e["what"] == line.as_str() && e["started_at"].is_null())).await;
        w.tick().await;
        w.advance(20);
        w.tick().await;
        let it = worker(&w.app).await;
        assert_eq!(it["status"], "working", "its own heavy entry waits in the queue: {it}");
        std::fs::write(first.join("release"), "").unwrap();
        w.gone(first_line).await;
        w.poll(1).await;
        until_file(&mine.join("started")).await;
        w.tick().await;
        w.advance(45);
        w.tick().await;
        let it = worker(&w.app).await;
        assert_eq!(it["status"], "working", "its turn ended 65 min ago, its own heavy entry runs: {it}");
        std::fs::write(mine.join("release"), "").unwrap();
    };
    let ran = async { tokio::join!(cli.worker_exclusive(None, &first, first_line), queued, body) };
    let (_, held, ()) = tokio::time::timeout(STUCK, ran).await.expect("the heavy commands end");
    assert_eq!(held.unwrap()["ok"], true);
    // Its silence counts from when its entry ended.
    w.tick().await;
    w.advance(30);
    w.tick().await;
    let it = worker(&w.app).await;
    assert_eq!(it["status"], "working", "30 minutes since its entry ended: at the bound: {it}");
    w.advance(1);
    w.tick().await;
    let silent = worker(&w.app).await;
    assert!(silent["why"].as_str().unwrap().starts_with("silent for 31 min"), "{silent}");
}

#[tokio::test]
async fn a_worker_that_declared_a_wait_is_not_silent_within_it_and_is_past_it_with_what_it_said() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    let closed = (w.app_now() / 1000) as u64;
    w.claude_session(&cwd, false, closed);
    let said = w.app.worker_waiting(id(&wk), "the model weights downloading", 60 * 60_000).await.unwrap();
    assert_eq!(said["minutes"], 60, "{said}");
    let shown = w.app.issue_show("web", 1).await.unwrap();
    let texts: Vec<_> = shown["comments"].as_array().unwrap().iter().map(|c| c["text"].clone()).collect();
    assert!(texts.contains(&json!("waits 60 min for: the model weights downloading")), "{shown}");
    w.advance(30);
    w.tick().await;
    w.advance(1);
    w.tick().await;
    assert_eq!(w.worker_on("web", 1).await["status"], "working", "31 min, within the 60 it declared");
    let doctor = w.doctor().await;
    assert_eq!(doctor["waiting"][0]["why"], "the model weights downloading", "{doctor}");
    w.advance(29);
    w.tick().await;
    assert_eq!(w.worker_on("web", 1).await["status"], "working", "at its bound, not past it");
    w.advance(1);
    w.tick().await;
    let silent = w.worker_on("web", 1).await;
    let why = silent["why"].as_str().unwrap();
    assert!(why.starts_with("silent for 61 min"), "{silent}");
    assert!(why.ends_with("past the 60 min wait it declared for: the model weights downloading"), "{silent}");
    assert_eq!(w.doctor().await["waiting"], json!([]), "it waits for the watch now");
}

#[tokio::test]
async fn a_declared_wait_past_the_cap_is_held_to_two_hours() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    let closed = (w.app_now() / 1000) as u64;
    w.claude_session(&cwd, false, closed);
    let said = w.app.worker_waiting(id(&wk), "a long download", 5 * 60 * 60_000).await.unwrap();
    assert_eq!(said["minutes"], 120, "{said}");
    w.advance(120);
    w.tick().await;
    assert_eq!(w.worker_on("web", 1).await["status"], "working", "at the cap");
    w.advance(1);
    w.tick().await;
    let silent = w.worker_on("web", 1).await;
    assert!(silent["why"].as_str().unwrap().contains("past the 120 min wait it declared"), "{silent}");
    let no = w.app.worker_waiting(id(&wk), "more", 60_000).await.unwrap_err();
    assert_eq!(no.code, "waiting", "a worker waiting for the watch declares nothing");
}

#[tokio::test]
async fn doctor_says_why_nothing_starts_and_an_empty_kit_is_a_finding_with_its_fix() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    // 18.5 GiB free under the floor of 20, and the CPU over its limit: each named, in its setting's unit.
    *w.host.reading.lock().unwrap() = factorium::host::Reading {
        cpu_busy: 90,
        memory_free: 60,
        disk_free: 18 * GIB + GIB / 2,
        disk_total: 100 * GIB,
    };
    assert!(w.tick().await.started.is_none());
    let d = w.doctor().await;
    assert_eq!(
        d["unstarted"],
        json!(["disk: 18.5 GiB free, under the floor of 20 GiB", "cpu: 90% busy, over 75%", "kit: the kit is empty"]),
        "{d}"
    );
    let kit = w.finding("kit:empty").await.expect("an empty kit with an issue ready is a finding");
    assert!(kit["what"].as_str().unwrap().contains("web#1"), "{kit}");
    assert!(kit["fix"].as_str().unwrap().contains("factorium kit set"), "{kit}");

    // The kit set, the host alone holds it.
    w.kit().await;
    assert!(w.finding("kit:empty").await.is_none());
    let d = w.doctor().await;
    assert_eq!(d["unstarted"], json!(["disk: 18.5 GiB free, under the floor of 20 GiB", "cpu: 90% busy, over 75%"]));

    // An issue could start: no such field.
    *w.host.reading.lock().unwrap() =
        factorium::host::Reading { cpu_busy: 10, memory_free: 60, disk_free: 500 * GIB, disk_total: 1000 * GIB };
    assert!(w.doctor().await.get("unstarted").is_none());
    assert!(w.tick().await.started.is_some());
    // Nothing ready: none either, however full the host.
    w.host.reading.lock().unwrap().cpu_busy = 95;
    assert!(w.doctor().await.get("unstarted").is_none());
}

#[tokio::test]
async fn a_ready_issue_that_waits_for_a_blocker_is_no_reason_but_an_empty_kit_is_still_a_finding() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    let blocker = factorium_domain::issue::Blocker::Here(1);
    w.app.issue_add("web", "b", "", false, &[blocker]).await.unwrap();
    w.app.issue_wait_for_human("web", 1, "not yet").await.unwrap();
    assert!(w.doctor().await.get("unstarted").is_none(), "web#2 waits for web#1");
    assert!(w.finding("kit:empty").await.is_some());
}

/// doctor's reasons from a reading, the heavy queue and the kit's answer alone.
#[test]
fn every_reason_admission_holds_is_named_and_the_kit_only_when_no_issue_can_start() {
    use factorium_domain::admission::Reading;
    use factorium_domain::doctor::{ReadySeen, Seen, unstarted};
    use factorium_domain::heavy::{Entry, Queue};
    let used = |issue: &str| ReadySeen {
        issue: issue.into(),
        waits: false,
        kit: Some("codex: every account is used up".into()),
    };
    let queued = Entry { what: "cargo test".into(), queued_at: 0, ..Entry::default() };
    let seen = Seen {
        ready: vec![used("web#1"), used("web#2")],
        reading: Reading { cpu_busy: 10, memory_free: 3, disk_free: 500 * GIB, workers_cpu: 90 },
        queue: Queue { entries: vec![queued], ..Queue::default() },
        ..Seen::default()
    };
    let s = Default::default();
    assert_eq!(
        unstarted(&seen, &s, 301_000),
        [
            "memory: 3% free, under 15%",
            "workers: 90% of the CPU, over their 80%",
            "queue: `cargo test` has waited 301s for its turn, over 300s",
            "kit: codex: every account is used up",
        ]
    );
    // One issue the kit can start: the kit holds nothing.
    let mut one = seen.clone();
    one.ready.push(ReadySeen { issue: "web#3".into(), ..ReadySeen::default() });
    assert!(!unstarted(&one, &s, 301_000).iter().any(|w| w.starts_with("kit")));
    // Room, and an issue the kit can start: nothing holds.
    let room = Reading { memory_free: 50, disk_free: 500 * GIB, ..Reading::default() };
    let free = Seen { ready: vec![ReadySeen::default()], reading: room, ..Seen::default() };
    assert_eq!(unstarted(&free, &s, 0), Vec::<String>::new());
}
