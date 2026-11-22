//! A line's promotion gate and cut run beside the daemon's ticks
//! (factorium#216): while one runs, workers start and are watched, and the
//! tick after its steps end records how it ended. A checkout's moves still
//! go one at a time: while one of them runs beside the ticks, it carries no
//! other.

use std::path::Path;

use serde_json::Value;

use factorium::Tick;

use crate::fixture::{LOOK, STUCK, World, until_file, worker};

/// `web`, whose proving declares `toml` (`{long}` a command that holds until
/// `dir/release`), and a kit; #1 done with a change. Answers #1's worker.
async fn declares(w: &World, dir: &Path, toml: &str) -> Value {
    w.checkout("web").await;
    w.kit().await;
    w.push_by_hand("web", "proving", "factorium.toml", &toml.replace("{long}", &w.holds(dir)));
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick_alone().await;
    let one = worker(&w.app).await;
    w.done(&one, "a", "x").await;
    one
}

/// The tick that lands #1: the one after its fast gate, which runs beside
/// the ticks too, ended.
async fn lands_alone(w: &World) -> Tick {
    w.gated().await;
    w.tick_alone().await
}

/// While the move's `{long}` holds, issue #2 becomes ready: the next tick
/// starts its worker, and three more ticks find the move running on. Then
/// it ends, and the tick after records it, `kind`; answers what that tick
/// said of the lines.
async fn starts_while_it_runs(w: &World, dir: &Path, kind: &str) -> Vec<Value> {
    until_file(&dir.join("started")).await;
    assert!(w.app.moves_run(), "the move runs beside the ticks");
    assert_eq!(w.recorded(kind), 0);
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    let t = w.tick_alone().await;
    let started = t.started.expect("the worker starts on the next tick, before the move ends");
    assert_eq!(started["issue"], "web#2", "{started}");
    for _ in 0..3 {
        w.advance(1);
        let t = w.tick_alone().await;
        assert!(t.lines.is_empty(), "the move runs on: {:?}", t.lines);
    }
    assert!(w.app.moves_run() && !dir.join("release").exists());
    assert_eq!(w.recorded(kind), 0, "not before its steps end");
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("its steps end");
    let t = w.tick_alone().await;
    assert_eq!(w.recorded(kind), 1, "the tick after its steps ended records it: {:?}", t.lines);
    assert!(!w.app.moves_run());
    t.lines
}

#[tokio::test]
async fn a_worker_starts_while_a_cut_runs_beside_the_ticks_and_the_cut_is_then_made() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"true\"\n\n[cut]\nsteps = [\"{long}\"]\n").await;
    // Lands #1 and promotes it (no slow gate); idle, the cut starts beside the ticks.
    let t = lands_alone(&w).await;
    assert_eq!(t.landed[0]["landed"], "web#1", "{t:?}");
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "it runs on: {:?}", t.lines);
    let lines = starts_while_it_runs(&w, &dir, "CutMade").await;
    let cut = lines.iter().find(|l| l["cut"] == "web").expect("the cut is recorded");
    assert_eq!(cut["version"], "v0.1.0", "{cut}");
    assert!(cut["failed"].is_null(), "{cut}");
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.0"]).is_ok(), "tagged");
}

#[tokio::test]
async fn a_worker_starts_while_a_promotions_slow_gate_runs_beside_the_ticks_and_it_then_promotes() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"true\"\nslow = \"{long}\"\n").await;
    let main = w.remote_tip("web", "main");
    let t = lands_alone(&w).await;
    let landed = t.landed[0]["commit"].as_str().expect("#1 lands").to_string();
    assert!(t.lines.is_empty(), "the slow gate runs on: {:?}", t.lines);
    let lines = starts_while_it_runs(&w, &dir, "Promoted").await;
    assert!(lines.iter().any(|l| l["promoted"] == "web" && l["tip"] == landed.as_str()), "{lines:?}");
    assert_ne!(main, landed);
    assert_eq!(w.remote_tip("web", "main"), landed);
}

#[tokio::test]
async fn while_a_move_of_a_checkout_runs_beside_the_ticks_none_other_of_it_starts_and_another_checkouts_do() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"true\"\n\n[cut]\nsteps = [\"{long}\"]\n").await;
    lands_alone(&w).await;
    until_file(&dir.join("started")).await;
    // #2 starts, and is done while the cut runs.
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    assert!(w.tick_alone().await.started.is_some());
    let two = w.worker_on("web", 2).await;
    w.done(&two, "b", "x").await;
    // Another checkout's worker starts, and is done while the cut runs.
    w.checkout("api").await;
    w.app.issue_add("api", "a", "", false, &[]).await.unwrap();
    assert!(w.tick_alone().await.started.is_some());
    w.done(&w.worker_on("api", 1).await, "a", "x").await;
    let proving = w.remote_tip("web", "proving");
    let mut landed = Vec::new();
    let mut ticks = 0;
    // Three ticks at least, and until api's landing, its gate beside the ticks too, ends.
    tokio::time::timeout(STUCK, async {
        while ticks < 3 || landed.is_empty() {
            w.advance(1);
            let t = w.tick_alone().await;
            let of_web = t.lines.iter().chain(&t.landed).filter(|l| l.to_string().contains("web")).collect::<Vec<_>>();
            assert!(of_web.is_empty(), "no move of web starts: {t:?}");
            landed.extend(t.landed);
            ticks += 1;
            tokio::time::sleep(LOOK).await;
        }
    })
    .await
    .expect("api lands");
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert_eq!(landed[0]["landed"], "api#1", "api's line moves beside web's cut");
    assert_eq!(w.remote_tip("web", "proving"), proving, "#2 waits for the cut");
    let web: Vec<String> = w.queued().await.into_iter().filter(|q| q.ends_with(" web")).collect();
    assert_eq!(web, ["cut of web"], "nothing else of web asks the queue");
    // Once the cut ends: it is recorded, and then #2 lands, in its order.
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("its steps end");
    let t = w.tick_alone().await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["version"] == "v0.1.0"), "{:?}", t.lines);
    assert_eq!(w.tick().await.landed[0]["landed"], "web#2", "then #2's landing");
}

/// Seen 2026-11-22 (factorium#231): a landing's fast gate ran in the tick, so
/// one past the hang bound (98 s and 113 s, with the rest of their ticks) read
/// as a hung daemon. It runs beside the ticks: while it holds past the hang
/// bound, ticks keep coming, none slow, workers start; the tick after it
/// ends lands.
#[tokio::test]
async fn ticks_keep_coming_while_a_landings_fast_gate_runs_past_the_hang_bound() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"{long}\"\n").await;
    let t = w.tick_alone().await;
    assert!(t.landed.is_empty(), "the gate runs on: {t:?}");
    until_file(&dir.join("started")).await;
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    let hang = factorium_domain::daemon::hang_bound(10_000);
    let mut started = Vec::new();
    for _ in 0..3 {
        w.clock.advance(hang + 30_000);
        let t = tokio::time::timeout(STUCK, w.tick_alone()).await.expect("a tick ends while the gate holds");
        assert!(t.landed.is_empty(), "the gate runs on: {t:?}");
        assert!(t.took_ms < hang / 2, "no tick waits for it: {} ms in {:?}", t.took_ms, t.longest);
        started.extend(t.started);
    }
    assert!(started.iter().any(|s| s["issue"] == "web#2"), "a worker starts while it runs: {started:?}");
    assert!(w.app.moves_run() && !dir.join("release").exists(), "past three hang bounds, it holds still");
    let doctor = w.app.doctor().await.unwrap();
    assert!(doctor["slow_tick"].is_null(), "{doctor}");
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("its steps end");
    let t = w.tick_alone().await;
    assert_eq!(t.landed[0]["landed"], "web#1", "the tick after it ended lands: {t:?}");
    assert_eq!(w.remote_tip("web", "proving"), t.landed[0]["commit"].as_str().unwrap());
}

/// A worker that asks to land again with new commits while its landing's gate
/// runs is not pushed as gated: its branch moved, and it lands again with them.
#[tokio::test]
async fn a_branch_that_moves_while_its_landings_gate_runs_lands_again_with_what_it_added() {
    let w = World::new();
    let dir = w.dir.join("long");
    let one = declares(&w, &dir, "[gate]\nfast = \"{long}\"\n").await;
    w.tick_alone().await;
    until_file(&dir.join("started")).await;
    w.commit(&crate::fixture::worktree(&one), "late", "y");
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("its steps end");
    let t = w.tick_alone().await;
    assert!(t.landed.is_empty(), "not pushed as gated: {t:?}");
    let landed = w.tick().await.landed;
    let commit = landed.iter().find(|l| l["landed"] == "web#1").expect("it lands again")["commit"].as_str().unwrap();
    let files = w.git(&w.dir.join("remotes/web.git"), &["ls-tree", "--name-only", commit]).unwrap();
    assert!(files.lines().any(|f| f == "late"), "with what it added: {files}");
}

/// A worker that no longer asks to land once its landing's gate ends (it
/// went to the watch meanwhile) is not landed.
#[tokio::test]
async fn a_worker_that_no_longer_asks_to_land_once_its_gate_ends_is_not_landed() {
    let w = World::new();
    let dir = w.dir.join("long");
    let one = declares(&w, &dir, "[gate]\nfast = \"{long}\"\n").await;
    let proving = w.remote_tip("web", "proving");
    w.tick_alone().await;
    until_file(&dir.join("started")).await;
    w.app.worker_escalate(crate::fixture::id(&one), "stuck").await.unwrap();
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("its steps end");
    assert!(w.tick_alone().await.landed.is_empty());
    assert_eq!(w.remote_tip("web", "proving"), proving, "nothing pushed");
}
