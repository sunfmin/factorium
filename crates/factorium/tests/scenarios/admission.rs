//! Admission, the heavy-command queue, hot workers, failed sessions, and the
//! disk floor: decided from the host's readings alone.

use std::path::PathBuf;

use factorium::host::Proc;
use serde_json::json;

use crate::{CLAUDE, GIB, World, id, worker};

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
    assert_eq!(w.app.doctor().await.unwrap()["findings"], json!([]), "not held long yet");
    w.advance(2);
    w.tick().await;
    let found = w.app.doctor().await.unwrap()["findings"][0]["what"].as_str().unwrap().to_string();
    assert!(found.contains("admission held") && found.contains("cpu"), "{found}");

    w.host.reading.lock().unwrap().cpu_busy = 20;
    assert!(w.tick().await.started.is_some());
    assert_eq!(w.app.doctor().await.unwrap()["findings"], json!([]), "the hold ended");
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
    assert_eq!(w.app.workers(false).await.unwrap().as_array().unwrap().len(), 1);
    assert!(w.tick().await.started.is_some());
    assert_eq!(w.app.workers(false).await.unwrap().as_array().unwrap().len(), 2);
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
    let (cwd, id_) = (wk["worktree"].as_str().unwrap().to_string(), id(&wk));
    let now = (w.app_now() / 1000) as u64;
    crate::harnesses::write_session(&w, factorium_harness::Harness::ClaudeCode, None, &cwd, true, now);
    w.advance(45);
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "its turn is open");
    crate::harnesses::write_session(&w, factorium_harness::Harness::ClaudeCode, None, &cwd, false, now + 60);
    w.tick().await;
    let silent =
        w.app.workers(false).await.unwrap().as_array().unwrap().iter().find(|x| id(x) == id_).cloned().unwrap();
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
    assert_eq!(all.len(), 18, "every setting is listed");
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
    assert_eq!(w.app.doctor().await.unwrap()["findings"], json!([]), "held 60 of 120 seconds");
    w.advance(1);
    w.tick().await;
    let found = w.app.doctor().await.unwrap()["findings"][0]["what"].as_str().unwrap().to_string();
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
    crate::harnesses::write_session(&w, factorium_harness::Harness::ClaudeCode, None, &cwd, false, closed);
    w.advance(30);
    w.tick().await;
    let all = w.app.workers(false).await.unwrap();
    let it = all.as_array().unwrap().iter().find(|x| x["worker"] == wk["worker"]).cloned().unwrap();
    assert_eq!(it["status"], "working", "30 minutes since its turn ended: at the bound");
    w.advance(1);
    w.tick().await;
    let all = w.app.workers(false).await.unwrap();
    let silent = all.as_array().unwrap().iter().find(|x| x["worker"] == wk["worker"]).cloned().unwrap();
    assert!(silent["why"].as_str().unwrap().starts_with("silent for 31 min"), "{silent}");
}
