//! The daemon: a run of failing ticks and a program missing at start are
//! findings; as a launchd service it is installed, and stopped through
//! launchd.

use std::sync::atomic::AtomicBool;

use serde_json::json;

use crate::fixture::{CLAUDE, OPENS, OPENS_ONCE, World, program};

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
    let ran = w.app.run(&AtomicBool::new(true), None).await.unwrap();
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
    assert!(w.host.loaded(), "loaded");

    // Installed again: the loaded one is replaced, and one a desktop login
    // loaded into the GUI domain goes.
    let gui = w.host.service_target("gui");
    w.host.service.lock().unwrap().push(gui.clone());
    w.app.service_install().unwrap();
    assert!(w.host.loaded());
    assert!(!w.host.service.lock().unwrap().contains(&gui), "no second daemon in gui/<uid>");

    let stopped = w.app.stop().await.unwrap();
    assert_eq!(stopped["service"], "com.github.sunfmin.factorium", "{stopped}");
    // Loaded, stopped and installed in this user's own domain.
    let ran = w.host.ran.lock().unwrap().clone();
    let user = w.host.service_target("user");
    let domain = user.rsplit_once('/').unwrap().0.to_string();
    for (call, named) in [("print", &user), ("bootout", &user), ("bootstrap", &domain)] {
        let line = format!("launchctl {call} {named}");
        assert!(ran.iter().any(|r| r.starts_with(&line)), "{line}: {ran:?}");
    }
    assert!(!w.host.loaded(), "launchd will not start it again");

    w.app.service_install().unwrap();
    w.app.service_remove().unwrap();
    assert!(!file.exists() && !w.host.loaded());
}

#[test]
fn install_bootstraps_once_launchd_has_dropped_the_agent_it_booted_out() {
    let w = World::new();
    w.app.service_install().unwrap();
    // launchd still finds the booted-out agent for three prints.
    *w.host.lingers.lock().unwrap() = 3;
    w.host.ran.lock().unwrap().clear();
    w.app.service_install().unwrap();
    assert!(w.host.loaded(), "loaded again");
    let ran = w.host.ran.lock().unwrap().clone();
    let at = |call: &str| ran.iter().position(|r| r.starts_with(call)).unwrap();
    let bootstraps = ran.iter().filter(|r| r.starts_with("launchctl bootstrap")).count();
    assert_eq!(bootstraps, 1, "{ran:?}");
    let user = w.host.service_target("user");
    let after = &ran[at("launchctl bootout")..at("launchctl bootstrap")];
    let prints = after.iter().filter(|r| **r == format!("launchctl print {user}")).count();
    assert_eq!(prints, 4, "three that find it, one that does not: {ran:?}");
    assert!(after.iter().any(|r| r.starts_with("sleep")), "it waits between prints: {ran:?}");
}

#[test]
fn install_fails_naming_the_agent_launchd_never_drops_and_does_not_bootstrap() {
    let w = World::new();
    w.app.service_install().unwrap();
    *w.host.lingers.lock().unwrap() = usize::MAX;
    w.host.ran.lock().unwrap().clear();
    let e = w.app.service_install().unwrap_err();
    let user = w.host.service_target("user");
    assert_eq!(e.code, "launchctl", "{e}");
    assert!(e.message.contains(&user), "names the job: {e}");
    assert!(e.message.contains(" 10 s "), "names the bound: {e}");
    let ran = w.host.ran.lock().unwrap().clone();
    assert!(!ran.iter().any(|r| r.starts_with("launchctl bootstrap")), "{ran:?}");
    let sleeps = ran.iter().filter(|r| r.starts_with("sleep")).count();
    assert_eq!(sleeps, 100, "within the bound: 10 s of tenths");
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
    let ran = w.app.run(&AtomicBool::new(true), None).await.unwrap();
    assert_eq!(ran, json!({ "stopped": true, "ticks": 0 }));
}

/// The pid file, locked as a daemon locks it while it runs, saying `said`.
fn held_saying(pid: &std::path::Path, said: &str) -> std::fs::File {
    let f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(pid).unwrap();
    f.try_lock().unwrap();
    std::fs::write(pid, said).unwrap();
    f
}

#[tokio::test]
async fn a_second_daemon_is_refused_while_the_first_holds_the_lock_whatever_its_file_says() {
    let w = World::new();
    let pid = w.data().pid();
    // Its pid not yet written, or a pid no process has (macOS gives at most 99998).
    for said in ["", "999999"] {
        let held = held_saying(&pid, said);
        let e = w.app.run(&AtomicBool::new(true), None).await.unwrap_err();
        assert_eq!(e.code, "running", "{said:?}: {e}");
        assert!(e.message.starts_with("a daemon runs already, a daemon holds "), "{said:?}: {e}");
        assert_eq!(std::fs::read_to_string(&pid).unwrap(), said, "the refused one leaves its file");
        drop(held);
    }
    // Once the holder ends, however it ends, the next one runs.
    assert_eq!(w.app.run(&AtomicBool::new(true), None).await.unwrap(), json!({ "stopped": true, "ticks": 0 }));
    assert!(!pid.exists());
}

#[tokio::test]
async fn two_daemons_started_together_one_runs_the_other_is_refused() {
    let w = World::new();
    w.app.settings_set(Some("tick_every"), Some(600)).await.unwrap();
    let stop = AtomicBool::new(false);
    let second = async {
        // The first ticked, and waits for its next tick, holding its lock.
        w.until(async || w.clock.asleep() == 1).await;
        let refused = w.app.run(&AtomicBool::new(true), None).await;
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        w.advance(10);
        refused
    };
    let (first, second) =
        tokio::time::timeout(crate::fixture::STUCK, async { tokio::join!(w.app.run(&stop, None), second) })
            .await
            .expect("the first one stops");
    let e = second.unwrap_err();
    assert_eq!(e.code, "running");
    assert_eq!(e.message, format!("a daemon runs already, as pid {}", std::process::id()));
    assert_eq!(first.unwrap()["stopped"], true);
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
    assert_eq!(w.app.run(&stop, None).await.unwrap()["ticks"], 1);
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
        w.until(async || w.clock.asleep() == 1).await;
        w.advance(10);
    };
    let (ran, ()) = tokio::time::timeout(crate::fixture::STUCK, async { tokio::join!(w.app.run(&stop, None), moves) })
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
    // Homebrew links a newer one since, which opens the database, then may not be run.
    w.links_newer("factorium", OPENS_ONCE);
    swaps(&w.app).await;
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let during = stop.clone();
    let stopping = move || during.store(true, std::sync::atomic::Ordering::Relaxed);
    *w.host.reading_procs.lock().unwrap() = Some(Box::new(stopping));
    assert_eq!(w.app.run(&stop, None).await.unwrap(), json!({ "stopped": true, "ticks": 1 }));
}

/// The restart `app`'s ticks ask for, once the proof the first started
/// beside them (factorium#241) passed.
async fn swaps(app: &factorium::App) -> String {
    let begun = std::time::Instant::now();
    loop {
        if let Some(restart) = app.tick().await.unwrap().restart {
            return restart;
        }
        assert!(begun.elapsed() < crate::fixture::STUCK, "the proof ends");
        tokio::time::sleep(crate::fixture::LOOK).await;
    }
}

/// Homebrew links `version` of the formula `factorium` (the `running_as`
/// one): answers its binary, canonical.
fn link(w: &World, version: &str) -> std::path::PathBuf {
    let bin = w.dir.join("brew/Cellar/factorium").join(version).join("bin");
    program(&bin.join("factorium"), &binary(version));
    let opt = w.dir.join("brew/opt/factorium");
    std::fs::remove_file(&opt).unwrap();
    std::os::unix::fs::symlink(bin.parent().unwrap(), &opt).unwrap();
    bin.join("factorium").canonicalize().unwrap()
}

/// A binary of `version` that opens the database.
fn binary(version: &str) -> String {
    format!("{OPENS}# {version}\n")
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
    assert_eq!(kept(&w), [("0.0.9".into(), OPENS.into())], "the first good tick keeps it");
    let new = link(&w, "0.1.0");
    assert_eq!(swaps(&w.app).await, link0.to_string_lossy());

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
    assert_eq!(kept(&w), [("0.0.9".into(), OPENS.into())], "a binary that never ticked well is not kept");

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
    assert_eq!(swaps(&old).await, link0.to_string_lossy());
    let swapped = w.run();
    swapped.runs_as(&newer);
    w.advance(5);
    swapped.tick().await.unwrap();
    assert_eq!(w.finding("daemon:rollback").await, None);
    assert_eq!(kept(&w), [("0.1.1".into(), binary("0.1.1"))]);
}

/// v0.1.83 linked, a CLI call of it wrote `DecidedNumbered` before the
/// daemon, on 0.1.82, swapped; its store could not read it, so every tick
/// failed, and the swap, a step of the tick, never came (factorium#252).
/// A failed tick still swaps once the linked binary is proven.
#[tokio::test]
async fn a_daemon_whose_ticks_fail_on_an_event_a_newer_binary_wrote_still_swaps_to_it() {
    let w = World::new();
    let link0 = w.running_as(&w.app, "factorium");
    w.checkout("web").await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    assert_eq!(w.tick().await.restart, None);
    // The newer binary linked, and one of its CLI calls writes first.
    link(&w, "0.1.0");
    w.an_event_a_newer_binary_wrote("FromANewerVersion");
    let begun = std::time::Instant::now();
    let restart = loop {
        w.advance(5);
        let ticked = w.app.tick().await;
        let why = ticked.as_ref().expect_err("its store cannot read the event").to_string();
        assert!(why.contains("FromANewerVersion"), "{why}");
        // It ticks on, and does not go back to the kept binary of its own version.
        if let Some(to) = w.app.after(&ticked) {
            break to;
        }
        assert!(begun.elapsed() < crate::fixture::STUCK, "the proof ends");
        tokio::time::sleep(crate::fixture::LOOK).await;
    };
    assert_eq!(restart, link0, "it swaps to the linked binary by itself");
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
    assert_eq!(kept(&w), [("0.1.0".into(), binary("0.1.0"))], "the one to go back to, alone");
    // Its ticks failing later go back to nothing: it is the kept one's version.
    unreadable(&w, true);
    for _ in 1..=4 {
        w.advance(5);
        swapped.tick().await.unwrap_err();
    }
    assert_eq!(swapped.fallback(), None);
}

/// The daemon's run execs the kept binary once its ticks all failed: here
/// the kept one cannot run (not executable), so it goes on as it is.
#[tokio::test]
async fn a_run_whose_ticks_all_fail_execs_the_kept_binary() {
    let w = World::new();
    w.running_as(&w.app, "factorium");
    w.tick().await;
    // Kept, it may not be run after: the exec fails, as this process must go on.
    let kept_bin = w.data().binary().join("0.0.9/bin/factorium");
    std::fs::set_permissions(&kept_bin, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let new = link(&w, "0.1.0");
    let swapped = w.run();
    swapped.runs_as(&new);
    unreadable(&w, true);
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let moves = async {
        // Each wait between ticks, until one tick went back and the exec was tried.
        loop {
            w.until(async || w.clock.asleep() == 1).await;
            if swapped.fallback().is_some() {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            w.advance(60);
            if swapped.fallback().is_some() {
                break;
            }
            w.until(async || w.clock.asleep() != 1).await;
        }
    };
    let (ran, ()) =
        tokio::time::timeout(crate::fixture::STUCK, async { tokio::join!(swapped.run(&stop, None), moves) })
            .await
            .expect("it ticks as the clock moves");
    assert_eq!(ran.unwrap()["ticks"], 3, "the third failed tick went back");
}

#[tokio::test]
async fn a_pid_file_that_does_not_open_refuses_the_run_as_io() {
    let w = World::new();
    let pid = w.data().pid();
    std::fs::create_dir_all(&pid).unwrap();
    let e = w.app.run(&AtomicBool::new(true), None).await.unwrap_err();
    assert_eq!(e.code, "io", "{e}");
    assert!(e.message.contains("does not open"), "{e}");
}

#[tokio::test]
async fn stop_leaves_the_file_of_a_daemon_that_holds_its_lock_and_has_not_written_its_pid() {
    let w = World::new();
    let pid = w.data().pid();
    let held = held_saying(&pid, "");
    assert_eq!(w.app.stop().await.unwrap()["why"], "no daemon runs");
    assert!(pid.exists(), "its holder keeps it");
    drop(held);
}

/// factorium#231: each tick is timed by its steps. One past half the hang
/// bound is a finding naming the step it spent most in, kept past the fast
/// ticks after it, for an hour.
#[tokio::test]
async fn a_tick_past_half_the_hang_bound_is_a_finding_naming_its_longest_step() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let quick = w.tick_alone().await;
    assert!(quick.took_ms < 60_000 && !quick.longest.step.is_empty(), "{quick:?}");
    // The landing's tick ends the worker: its terminal's close takes 70 s.
    let clock = w.clock.clone();
    *w.host.orca.while_closing.lock().unwrap() = Some(Box::new(move |_: &str| clock.advance(70_000)));
    w.done(&wk, "a.txt", "a\n").await;
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1", "{t:?}");
    *w.host.orca.while_closing.lock().unwrap() = None;
    w.tick_alone().await;
    let doctor = w.app.doctor().await.unwrap();
    let found: Vec<&str> = crate::fixture::findings(&doctor).iter().filter_map(|f| f["key"].as_str()).collect();
    assert!(found.contains(&"daemon:slow-tick:the lines' moves"), "{found:?}");
    assert!(doctor["slow_tick"]["took_ms"].as_i64().unwrap() >= 70_000, "{}", doctor["slow_tick"]);
    let slow = |doctor: &serde_json::Value| {
        crate::fixture::findings(doctor).iter().any(|f| f["key"].as_str().unwrap().starts_with("daemon:slow"))
    };
    w.clock.advance(60 * 60_000 - 1);
    assert!(slow(&w.app.doctor().await.unwrap()), "within the hour");
    w.clock.advance(1);
    assert!(!slow(&w.app.doctor().await.unwrap()), "an hour on, gone");
}

/// daemon.log has each tick that did something, and each slow one, with its
/// time; a quick tick that did nothing it leaves out.
#[test]
fn a_tick_is_logged_once_it_did_something_or_was_slow() {
    use factorium::Tick;
    let one = || vec![json!({})];
    assert!(!Tick::default().logged(), "quick, with nothing done");
    assert!(Tick { slow: true, ..Tick::default() }.logged(), "slow, with nothing done");
    let did = [
        Tick { started: Some(json!({})), ..Tick::default() },
        Tick { landed: one(), ..Tick::default() },
        Tick { lines: one(), ..Tick::default() },
        Tick { seen: one(), ..Tick::default() },
        Tick { idle: one(), ..Tick::default() },
        Tick { resumed: one(), ..Tick::default() },
    ];
    for t in did {
        assert!(t.logged(), "{t:?}");
    }
}
