//! The daemon: a run of failing ticks and a program missing at start are
//! findings; as a launchd service it is installed, and stopped through
//! launchd.

use std::sync::atomic::AtomicBool;

use serde_json::json;

use crate::{CLAUDE, World, id, worktree};

#[tokio::test]
async fn ticks_failing_in_a_row_past_the_bound_are_a_finding_and_a_round() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.commit(&worktree(&wk), "a.txt", "a\n");
    w.app.worker_done(id(&wk)).await.unwrap();
    // Its remote is gone: every landing's fetch fails.
    let remote = w.dir.join("remotes/web.git");
    let away = w.dir.join("remotes/away.git");
    std::fs::rename(&remote, &away).unwrap();

    for n in 1..=3 {
        w.advance(5);
        let failed = w.app.tick().await.expect_err("the landing cannot fetch");
        assert_eq!(failed.code, "git");
        let found = w.finding("daemon:ticks").await;
        assert_eq!(found.is_some(), n == 3, "tick {n}: {found:?}");
    }
    let found = w.finding("daemon:ticks").await.unwrap();
    assert!(found["what"].as_str().unwrap().contains("last 3 ticks failed: git:"), "{found}");
    w.advance(5);
    w.app.tick().await.unwrap_err();
    let episodes = w.app.watch_next().await.unwrap()["episodes"].clone();
    assert!(episodes.get("daemon:ticks").is_some(), "the watch looked though the tick failed: {episodes}");

    std::fs::rename(&away, &remote).unwrap();
    w.advance(5);
    w.tick().await;
    assert_eq!(w.finding("daemon:ticks").await, None, "a tick that works ends the run");
}

#[tokio::test]
async fn a_program_missing_at_start_is_a_finding() {
    let w = World::new();
    w.kit().await;
    *w.host.missing.lock().unwrap() = vec!["orca".into(), "claude".into()];
    let ran = w.app.run(&AtomicBool::new(true)).await.unwrap();
    assert_eq!(ran["ticks"], 0);
    for program in ["orca", "claude"] {
        let found = w.finding(&format!("missing:{program}")).await.expect(program);
        assert!(found["what"].as_str().unwrap().contains("on its PATH"), "{found}");
    }
    assert_eq!(w.finding("missing:git").await, None, "git is found");

    w.host.missing.lock().unwrap().clear();
    assert_eq!(w.app.daemon_start().await.unwrap()["missing"], json!([]));
    assert_eq!(w.finding("missing:orca").await, None, "a start that finds it clears it");
}

#[tokio::test]
async fn the_service_is_installed_for_launchd_and_stop_unloads_it() {
    let w = World::new();
    assert_eq!(w.app.stop().await.unwrap()["stopped"], false);
    let installed = w.app.service_install().unwrap();
    let file = std::path::PathBuf::from(installed["installed"].as_str().unwrap());
    assert!(file.starts_with(w.dir.join("home/Library/LaunchAgents")), "{}", file.display());
    let plist = std::fs::read_to_string(&file).unwrap();
    assert!(plist.contains(&format!("<string>{}</string>", w.db.display())), "its database: {plist}");
    assert!(*w.host.service.lock().unwrap(), "loaded");

    // Installed again: the loaded one is replaced.
    w.app.service_install().unwrap();
    assert!(*w.host.service.lock().unwrap());

    let stopped = w.app.stop().await.unwrap();
    assert_eq!(stopped["service"], "com.github.sunfmin.factorium", "{stopped}");
    assert!(!*w.host.service.lock().unwrap(), "launchd will not start it again");

    w.app.service_install().unwrap();
    w.app.service_remove().unwrap();
    assert!(!file.exists() && !*w.host.service.lock().unwrap());
}

#[tokio::test]
async fn a_program_two_kit_entries_need_is_missing_once() {
    let w = World::new();
    w.app.kit_set(&["claude-code=claude".into(), "codex=claude --x".into()]).await.unwrap();
    *w.host.missing.lock().unwrap() = vec!["claude".into()];
    assert_eq!(w.app.daemon_start().await.unwrap()["missing"], json!(["claude"]));
}

#[tokio::test]
async fn a_daemon_that_restarted_as_its_own_pid_is_no_other_daemon() {
    let w = World::new();
    // A daemon that runs a newly installed binary keeps its pid, and its file.
    std::fs::write(w.data().pid(), std::process::id().to_string()).unwrap();
    let ran = w.app.run(&AtomicBool::new(true)).await.unwrap();
    assert_eq!(ran, json!({ "stopped": true, "ticks": 0 }));
}

#[tokio::test(start_paused = true)]
async fn a_run_ended_by_its_stop_flag_answers_its_ticks() {
    let w = World::new();
    w.app.settings_set(Some("tick_every"), Some(1)).await.unwrap();
    // Stopped during its first tick; its wait after it passes on the paused clock.
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let during = stop.clone();
    let stopping = move || during.store(true, std::sync::atomic::Ordering::Relaxed);
    *w.host.reading_procs.lock().unwrap() = Some(Box::new(stopping));
    assert_eq!(w.app.run(&stop).await.unwrap()["ticks"], 1);
}
