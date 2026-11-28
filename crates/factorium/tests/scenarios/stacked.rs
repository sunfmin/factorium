//! A checkout's landings stack (docs/adr/0019-stacked-landing-gates.md): each worker that asks
//! to land is merged onto the one below, the first onto the proving tip, their fast gates run at
//! once as the heavy queue has room, and proving moves in one push to the deepest merge whose
//! gate and every gate below it are green.

use std::path::{Path, PathBuf};

use std::time::{Duration, Instant};

use factorium_domain::settings::Resource;
use serde_json::{Value, json};

use crate::fixture::{LOOK, STUCK, World};

/// `web`, whose proving declares a fast gate that logs `start <files>` and `end <files>` (the
/// `f<n>` files of the tree it runs on) to `dir/log`; is red at once on a tree that holds `RED`;
/// and, on a tree of `k` such files (the `k`th of a stack), holds while [`hold`] holds it. A worker for each of `extra`, #1 up, each commits `f<n>` (and `extra[n-1]`, if any)
/// and is done. Answers the log.
async fn all_done(w: &World, dir: &Path, extra: &[Option<&str>]) -> PathBuf {
    all_done_with(w, dir, extra, "").await
}

/// [`all_done`], `more` added to its `[gate]`.
async fn all_done_with(w: &World, dir: &Path, extra: &[Option<&str>], more: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let (log, d) = (dir.join("log"), dir.display());
    let log = log.display();
    // `$k` is the shell's: the `k`th's hold is `dir/h<k>`.
    let held = w.holds(&dir.join("h$k"));
    let gate = format!(
        "k=$(ls f[0-9]* | wc -l | tr -d ' '); f=$(ls f[0-9]* | tr '\\\\n' ' '); echo \\\"start $f\\\" >> {log}; \
         [ -e RED ] && exit 1; if [ -d {d}/h$k ]; then ({held}) || exit 1; fi; echo \\\"end $f\\\" >> {log}"
    );
    w.checkout("web").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    w.push_by_hand("web", "proving", "factorium.toml", &format!("[gate]\nfast = \"{gate}\"\n{more}"));
    for n in 1..=extra.len() {
        w.app.issue_add("web", &format!("t{n}"), "", false, &[]).await.unwrap();
    }
    for _ in 0..extra.len() {
        w.tick().await;
    }
    for n in 1..=extra.len() as u64 {
        let wk = w.worker_on("web", n).await;
        if let Some(file) = extra[n as usize - 1] {
            w.commit(&crate::fixture::worktree(&wk), file, "x");
        }
        w.done(&wk, &format!("f{n}"), "x").await;
        w.advance(1);
    }
    dir.join("log")
}

/// Holds the gate of the `k`th of a stack, `on`, or lets it go.
fn hold(dir: &Path, k: usize, on: bool) {
    let held = dir.join(format!("h{k}"));
    if on { std::fs::create_dir_all(held).unwrap() } else { std::fs::write(held.join("release"), "").unwrap() }
}

/// What the gates logged, line by line.
fn logged(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

/// Ticks, each a minute on, until `n` of web landed or `stop` says so: what each tick landed.
async fn ticks_until(w: &World, n: usize) -> Vec<Vec<Value>> {
    let mut ticks = Vec::new();
    tokio::time::timeout(STUCK, async {
        while ticks.iter().map(|t: &Vec<Value>| t.len()).sum::<usize>() < n {
            w.advance(1);
            let landed: Vec<Value> =
                w.tick_alone().await.landed.into_iter().filter(|l| l["landed"].is_string()).collect();
            if !landed.is_empty() {
                ticks.push(landed);
            }
            tokio::time::sleep(LOOK).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{n} land: {ticks:?}"));
    ticks
}

/// The first parent of `commit` on web's remote.
fn parent(w: &World, commit: &str) -> String {
    w.git(&w.dir.join("remotes/web.git"), &["rev-parse", &format!("{commit}^1")]).unwrap()
}

/// The files of `commit` on web's remote.
fn files(w: &World, commit: &str) -> String {
    w.git(&w.dir.join("remotes/web.git"), &["ls-tree", "--name-only", commit]).unwrap()
}

fn issues(landed: &[Value]) -> Vec<&str> {
    landed.iter().map(|l| l["landed"].as_str().unwrap()).collect()
}

/// Until the gates logged `n` starts.
async fn until_started(w: &World, log: &Path, n: usize) {
    tokio::time::timeout(STUCK, async {
        while logged(log).iter().filter(|l| l.starts_with("start")).count() < n {
            w.advance(1);
            w.tick_alone().await;
            tokio::time::sleep(LOOK).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{n} gates start: {:?}", logged(log)));
}

#[tokio::test]
async fn three_landings_all_green_gate_at_once_and_proving_moves_once_to_the_third_merge() {
    let w = World::new();
    let dir = w.dir.join("gates");
    (1..=3).for_each(|k| hold(&dir, k, true));
    let log = all_done(&w, &dir, &[None, None, None]).await;
    until_started(&w, &log, 3).await;
    let mut starts: Vec<_> = logged(&log).into_iter().filter(|l| l.starts_with("start")).collect();
    starts.sort();
    assert_eq!(starts, ["start f1 ", "start f1 f2 ", "start f1 f2 f3 "], "each merged onto the one below");
    assert!(!logged(&log).iter().any(|l| l.starts_with("end")), "all three run at once: {:?}", logged(&log));
    let doctor = w.doctor().await;
    let stack: Vec<_> = doctor["landing"].as_array().unwrap().iter().map(|l| l["stack"]["depth"].clone()).collect();
    assert_eq!(stack, [Value::from(1), Value::from(2), Value::from(3)], "{doctor}");
    (1..=3).for_each(|k| hold(&dir, k, false));
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("their gates end");
    let ticks = ticks_until(&w, 3).await;
    assert_eq!(ticks.len(), 1, "one tick, one push: {ticks:?}");
    let landed = &ticks[0];
    assert_eq!(issues(landed), ["web#1", "web#2", "web#3"]);
    let commit = |i: usize| landed[i]["commit"].as_str().unwrap().to_string();
    assert_eq!(w.remote_tip("web", "proving"), commit(2), "proving is the third merge");
    assert_eq!((parent(&w, &commit(2)), parent(&w, &commit(1))), (commit(1), commit(0)), "stacked as gated");
    for n in 1..=3 {
        let shown = w.app.issue_show("web", n).await.unwrap();
        assert_eq!(shown["status"], "closed", "web#{n} lands and closes: {shown}");
    }
}

#[tokio::test]
async fn the_second_of_three_red_the_first_lands_and_the_third_is_gated_again_without_it() {
    let w = World::new();
    let dir = w.dir.join("gates");
    hold(&dir, 1, true);
    let log = all_done(&w, &dir, &[None, Some("RED"), None]).await;
    until_started(&w, &log, 3).await;
    // #2's gate is red at once, and #3's, stacked on it: neither is told while #1's runs.
    for _ in 0..3 {
        w.advance(1);
        assert!(w.tick_alone().await.landed.is_empty(), "nothing is told before #1's gate ends");
    }
    let doctor = w.doctor().await;
    let gates: Vec<_> = doctor["landing"].as_array().unwrap().iter().map(|l| l["stack"]["gate"].clone()).collect();
    let red = Value::from("red, told once the gates below it are green");
    assert_eq!(gates, [Value::from("its gate runs"), red.clone(), red], "{doctor}");
    hold(&dir, 1, false);
    // #1 lands; #2 is told red; #3, red only on #2, is stacked again.
    let mut red = Vec::new();
    tokio::time::timeout(STUCK, async {
        while red.is_empty() {
            w.advance(1);
            red.extend(w.tick_alone().await.landed.into_iter().filter(|l| l["red"].is_string()));
            tokio::time::sleep(LOOK).await;
        }
    })
    .await
    .expect("#2 is red");
    assert!(red[0]["red"].as_str().unwrap().contains("fast gate is red"), "{red:?}");
    assert_eq!(w.worker_on("web", 2).await["status"], "working", "#2 is told, back to work");
    assert_eq!(w.worker_on("web", 3).await["status"], "landing", "#3 is not told");
    let one = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!(one["status"], "closed", "#1 landed: {one}");
    let landed: Vec<Value> = ticks_until(&w, 1).await.concat();
    assert_eq!(issues(&landed), ["web#3"]);
    let (one, three) = (
        w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "proving^1"]).unwrap(),
        landed[0]["commit"].as_str().unwrap(),
    );
    assert_eq!(parent(&w, three), one, "#3 merged again onto #1's");
    assert!(files(&w, &one).contains("f1"), "{}", files(&w, &one));
    assert!(!files(&w, three).contains("RED") && !files(&w, three).contains("f2"), "{}", files(&w, three));
    assert!(logged(&log).contains(&"start f1 f3 ".to_string()), "gated again on T+A+C: {:?}", logged(&log));
}

#[tokio::test]
async fn a_merge_green_before_the_one_below_it_waits_and_lands_with_it() {
    let w = World::new();
    let dir = w.dir.join("gates");
    hold(&dir, 1, true);
    let log = all_done(&w, &dir, &[None, None, None]).await;
    until_started(&w, &log, 3).await;
    tokio::time::timeout(STUCK, async {
        while !logged(&log).contains(&"end f1 f2 f3 ".to_string()) {
            tokio::time::sleep(LOOK).await;
        }
    })
    .await
    .expect("#2's and #3's gates end green");
    for _ in 0..3 {
        w.advance(1);
        assert!(w.tick_alone().await.landed.is_empty(), "they wait for #1's gate");
    }
    let doctor = w.doctor().await;
    let gates: Vec<_> = doctor["landing"].as_array().unwrap().iter().map(|l| l["stack"]["gate"].clone()).collect();
    let waits = Value::from("green, waits for the gates below it");
    assert_eq!(gates, [Value::from("its gate runs"), waits.clone(), waits], "{doctor}");
    hold(&dir, 1, false);
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("#1's gate ends");
    let ticks = ticks_until(&w, 3).await;
    assert_eq!(ticks.len(), 1, "they land with it: {ticks:?}");
    assert_eq!(issues(&ticks[0]), ["web#1", "web#2", "web#3"]);
}

/// Two stacked, the upper green first: one push lands both. Its run is the line's kept trace
/// (`models/line.sh trace`).
#[tokio::test]
async fn two_stacked_the_upper_green_first_land_in_one_push() {
    let w = World::new();
    let dir = w.dir.join("gates");
    hold(&dir, 1, true);
    let log = all_done(&w, &dir, &[None, None]).await;
    until_started(&w, &log, 2).await;
    tokio::time::timeout(STUCK, async {
        while !logged(&log).contains(&"end f1 f2 ".to_string()) {
            tokio::time::sleep(LOOK).await;
        }
    })
    .await
    .expect("#2's gate ends green");
    w.advance(1);
    assert!(w.tick_alone().await.landed.is_empty(), "#2 waits for #1's gate");
    hold(&dir, 1, false);
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("#1's gate ends");
    let ticks = ticks_until(&w, 2).await;
    assert_eq!(ticks.len(), 1, "one push: {ticks:?}");
    assert_eq!(issues(&ticks[0]), ["web#1", "web#2"]);
    assert_eq!(w.remote_tip("web", "proving"), ticks[0][1]["commit"].as_str().unwrap());
}

#[tokio::test]
async fn a_host_with_room_for_one_gate_runs_one_stacked_gate_at_a_time_and_lands_them_in_order() {
    let w = World::new();
    let dir = w.dir.join("gates");
    let log = all_done(&w, &dir, &[None, None, None]).await;
    // No room beside what runs: one start at a time, each once nothing runs.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let landed: Vec<Value> = ticks_until(&w, 3).await.concat();
    assert_eq!(issues(&landed), ["web#1", "web#2", "web#3"]);
    let lines = logged(&log);
    for pair in lines.chunks(2) {
        assert!(pair[0].starts_with("start") && pair[1].starts_with("end"), "one at a time: {lines:?}");
    }
    assert_eq!(w.remote_tip("web", "proving"), landed[2]["commit"].as_str().unwrap());
}

#[tokio::test]
async fn stacked_gates_holding_a_one_slot_resource_run_in_turn_and_the_wait_is_not_their_limit() {
    let w = World::new();
    w.app.resource_declare("MAC", Some(Resource { slots: 1, note: None }), false).await.unwrap();
    let dir = w.dir.join("gates");
    hold(&dir, 1, true);
    let limit = Duration::from_secs(3);
    let more = format!("fast_limit = {}\nresources = [\"MAC\"]\n", limit.as_secs());
    let log = all_done_with(&w, &dir, &[None, None], &more).await;
    until_started(&w, &log, 1).await;
    // #2 stacked on #1, its gate asked: it waits for MAC, which #1's gate holds.
    let waits = tokio::time::timeout(STUCK, async {
        loop {
            w.advance(1);
            w.tick_alone().await;
            let doctor = w.doctor().await;
            let waiting = doctor["heavy"].as_array().unwrap().iter().find(|e| e["waits_for"] == "MAC").cloned();
            if let Some(e) = waiting {
                return (Instant::now(), e, doctor);
            }
            tokio::time::sleep(LOOK).await;
        }
    });
    let (since, entry, doctor) = waits.await.expect("#2's gate waits for MAC");
    assert_eq!((entry["started_at"].clone(), entry["resources"].clone()), (Value::Null, json!(["host", "MAC"])));
    let gates: Vec<_> = doctor["landing"].as_array().unwrap().iter().map(|l| l["stack"]["gate"].clone()).collect();
    assert_eq!(gates, [json!("its gate runs"), json!("its gate waits for MAC")], "{doctor}");
    assert_eq!(logged(&log), ["start f1 "], "the second has not started");
    hold(&dir, 1, false);
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("#1's gate ends");
    // #2's gate has waited longer than its limit before it may start.
    tokio::time::sleep((since + limit + Duration::from_millis(500)).saturating_duration_since(Instant::now())).await;
    let landed: Vec<Value> = ticks_until(&w, 2).await.concat();
    assert_eq!(issues(&landed), ["web#1", "web#2"], "neither ended by its limit");
    assert_eq!(logged(&log), ["start f1 ", "end f1 ", "start f1 f2 ", "end f1 f2 "], "one after the other");
}
