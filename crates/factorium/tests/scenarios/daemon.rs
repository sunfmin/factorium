//! The daemon: a run of failing ticks and a program missing at start are
//! findings; as a launchd service it is installed, and stopped through
//! launchd.

use std::sync::atomic::AtomicBool;

use serde_json::json;

use crate::fixture::{CLAUDE, World};

#[tokio::test]
async fn ticks_failing_in_a_row_past_the_bound_are_a_finding_and_a_round() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.done(&wk, "a.txt", "a\n").await;
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

#[test]
fn the_service_runs_homebrews_link_not_the_versioned_keg() {
    let w = World::new();
    let link = w.running_as(&w.app, "factorium");
    let plist = w.app.service_show().unwrap()["plist"].as_str().unwrap().to_string();
    assert!(plist.contains(&format!("<string>{}</string>", link.display())), "the opt link: {plist}");
    assert!(!plist.contains("Cellar"), "{plist}");
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

#[tokio::test]
async fn a_run_ended_by_its_stop_flag_answers_its_ticks() {
    let w = World::new();
    w.app.settings_set(Some("tick_every"), Some(1)).await.unwrap();
    // Stopped during its first tick: it waits for no next one.
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let during = stop.clone();
    let stopping = move || during.store(true, std::sync::atomic::Ordering::Relaxed);
    *w.host.reading_procs.lock().unwrap() = Some(Box::new(stopping));
    assert_eq!(w.app.run(&stop).await.unwrap()["ticks"], 1);
}

/// The daemon waits between ticks on the app's clock: the next tick comes
/// when the clock is moved past it, in no real time.
#[tokio::test]
async fn the_daemon_waits_between_ticks_on_the_apps_clock() {
    use factorium::host::Clock;
    let w = World::new();
    w.app.settings_set(Some("tick_every"), Some(600)).await.unwrap();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    // Stopped during the tick that comes once the clock passed its wait.
    let (during, clock) = (stop.clone(), w.clock.clone());
    let next = w.app_now() + 600_000;
    let stopping = move || {
        if clock.now() >= next {
            during.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    };
    *w.host.reading_procs.lock().unwrap() = Some(Box::new(stopping));
    let moves = async {
        // Its first tick done, it waits, asleep on the clock.
        while w.clock.asleep() != 1 {
            tokio::task::yield_now().await;
        }
        w.advance(10);
    };
    let (ran, ()) = tokio::time::timeout(crate::fixture::STUCK, async { tokio::join!(w.app.run(&stop), moves) })
        .await
        .expect("the clock moved, it ticks");
    assert_eq!(ran.unwrap()["ticks"], 2);
}

/// A daemon whose binary Homebrew has since replaced execs the new one
/// between ticks; if that cannot run, it goes on as it is.
#[tokio::test]
async fn a_daemon_that_cannot_exec_the_binary_homebrew_links_now_goes_on() {
    let w = World::new();
    w.app.settings_set(Some("tick_every"), Some(1)).await.unwrap();
    w.running_as(&w.app, "factorium");
    // Homebrew links a newer one since, which may not be run (not executable).
    let newer = w.dir.join("brew/Cellar/factorium/0.1.0/bin");
    std::fs::create_dir_all(&newer).unwrap();
    std::fs::write(newer.join("factorium"), "a binary that may not be run").unwrap();
    std::fs::remove_file(w.dir.join("brew/opt/factorium")).unwrap();
    std::os::unix::fs::symlink(newer.parent().unwrap(), w.dir.join("brew/opt/factorium")).unwrap();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let during = stop.clone();
    let stopping = move || during.store(true, std::sync::atomic::Ordering::Relaxed);
    *w.host.reading_procs.lock().unwrap() = Some(Box::new(stopping));
    assert_eq!(w.app.run(&stop).await.unwrap(), json!({ "stopped": true, "ticks": 1 }));
}

/// Homebrew links `version` of the formula `factorium` (the `running_as`
/// one): answers its binary, canonical.
fn link(w: &World, version: &str) -> std::path::PathBuf {
    let bin = w.dir.join("brew/Cellar/factorium").join(version).join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("factorium"), format!("binary {version}")).unwrap();
    let opt = w.dir.join("brew/opt/factorium");
    std::fs::remove_file(&opt).unwrap();
    std::os::unix::fs::symlink(bin.parent().unwrap(), &opt).unwrap();
    bin.join("factorium").canonicalize().unwrap()
}

/// The binary kept to go back to: its version, and what it holds.
fn kept(w: &World) -> Vec<(String, String)> {
    let Ok(versions) = std::fs::read_dir(w.data().binary()) else { return Vec::new() };
    let binary = |v: &std::path::Path| std::fs::read_to_string(v.join("bin/factorium")).unwrap();
    versions.flatten().map(|v| (v.file_name().to_string_lossy().into_owned(), binary(&v.path()))).collect()
}

/// The new binary's store cannot read the database (0.1.42 on 2026-11-20):
/// the events are not where it looks.
fn unreadable(w: &World, unreadable: bool) {
    let (from, to) = if unreadable { ("events", "events_away") } else { ("events_away", "events") };
    w.migrate(&format!("ALTER TABLE {from} RENAME TO {to}"));
}

#[tokio::test]
async fn a_new_binary_whose_ticks_all_fail_goes_back_to_the_kept_one() {
    let w = World::new();
    let link0 = w.running_as(&w.app, "factorium");
    assert!(kept(&w).is_empty(), "nothing is kept before a tick goes well");
    assert_eq!(w.tick().await.restart, None);
    assert_eq!(kept(&w), [("0.0.9".into(), "a binary".into())], "the first good tick keeps it");
    let new = link(&w, "0.1.0");
    assert_eq!(w.tick().await.restart.as_deref(), Some(&*link0.to_string_lossy()));

    // The swapped-in binary, as the same pid: it cannot read the database.
    let swapped = w.run();
    swapped.runs_as(&new);
    unreadable(&w, true);
    for n in 1..=3 {
        w.advance(5);
        swapped.tick().await.expect_err("its store cannot read the database");
        assert_eq!(swapped.fallback().is_some(), n == 3, "tick {n}");
    }
    let back = swapped.fallback().unwrap();
    assert!(back.starts_with(w.data().binary().canonicalize().unwrap()), "{}", back.display());
    assert_eq!(kept(&w), [("0.0.9".into(), "a binary".into())], "a binary that never ticked well is not kept");

    // The kept binary, exec'd as the same pid, reads it.
    unreadable(&w, false);
    let old = w.run();
    old.runs_as(&back);
    w.advance(5);
    assert_eq!(old.tick().await.unwrap().restart, None, "not to 0.1.0 again while the link names it");
    let found = w.finding("daemon:rollback").await.expect("the rollback is a finding");
    let what = found["what"].as_str().unwrap();
    assert!(what.contains("from factorium 0.1.0 to 0.0.9") && what.contains("events"), "{what}");
    w.advance(5);
    assert_eq!(old.tick().await.unwrap().restart, None);
    assert!(w.finding("daemon:rollback").await.is_some(), "the kept binary does not end it");
    let episodes = w.app.watch_next().await.unwrap()["episodes"].clone();
    assert!(episodes.get("daemon:rollback").is_some(), "the next look's episode: {episodes}");

    // A newer one linked: the kept binary swaps to it, whose good first tick ends the rollback.
    let newer = link(&w, "0.1.1");
    w.advance(5);
    assert_eq!(old.tick().await.unwrap().restart.as_deref(), Some(&*link0.to_string_lossy()));
    let swapped = w.run();
    swapped.runs_as(&newer);
    w.advance(5);
    swapped.tick().await.unwrap();
    assert_eq!(w.finding("daemon:rollback").await, None);
    assert_eq!(kept(&w), [("0.1.1".into(), "binary 0.1.1".into())]);
}

#[tokio::test]
async fn a_swap_whose_first_tick_is_good_keeps_the_new_binary() {
    let w = World::new();
    w.running_as(&w.app, "factorium");
    w.tick().await;
    let new = link(&w, "0.1.0");
    let swapped = w.run();
    swapped.runs_as(&new);
    swapped.tick().await.unwrap();
    assert_eq!(kept(&w), [("0.1.0".into(), "binary 0.1.0".into())], "the one to go back to, alone");
    // Its ticks failing later go back to nothing: it is the kept one's version.
    unreadable(&w, true);
    for _ in 1..=4 {
        w.advance(5);
        swapped.tick().await.unwrap_err();
    }
    assert_eq!(swapped.fallback(), None);
}
