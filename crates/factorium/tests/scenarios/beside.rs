//! A line's promotion gate and cut run beside the daemon's ticks
//! (factorium#216): while one runs, workers start and are watched, and the
//! tick after its steps end records how it ended. A checkout's landing,
//! promotion and cut run apart (docs/adr/0018-a-checkout-s-moves-run-apart.md): its workers
//! land while its promotion's gate or its cut runs, no second promotion or cut starts beside the
//! first, and its landings stack (docs/adr/0019-stacked-landing-gates.md).

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

/// Issues `titles` of `web`, numbered from `first`, become ready, and ticks start a worker on
/// each; each commits and is done.
async fn done_while_it_runs(w: &World, first: u64, titles: &[&str]) {
    for t in titles {
        w.app.issue_add("web", t, "", false, &[]).await.unwrap();
    }
    for _ in titles {
        assert!(w.tick_alone().await.started.is_some(), "a worker starts while the move runs");
    }
    for (n, t) in (first..).zip(titles) {
        w.done(&w.worker_on("web", n).await, t, "x").await;
    }
}

/// Ticks, each a minute on, until `n` landings of web landed: what each landed, in order. Each
/// landing's fast gate runs beside the ticks, and the next tick finishes it.
async fn land_while_it_runs(w: &World, n: usize) -> Vec<Value> {
    let mut landed = Vec::new();
    tokio::time::timeout(STUCK, async {
        while landed.len() < n {
            w.advance(1);
            landed.extend(w.tick_alone().await.landed.into_iter().filter(|l| l["landed"].is_string()));
            tokio::time::sleep(LOOK).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{n} land while the move runs: {landed:?}"));
    landed
}

/// What web asks of the heavy queue, waiting or running.
async fn queued_of_web(w: &World) -> Vec<String> {
    w.queued().await.into_iter().filter(|q| q.ends_with(" web")).collect()
}

#[tokio::test]
async fn a_worker_lands_while_a_promotions_slow_gate_runs_and_the_base_moves_to_the_tip_it_read() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"true\"\nslow = \"{long}\"\n").await;
    let main = w.remote_tip("web", "main");
    let read = lands_alone(&w).await.landed[0]["commit"].as_str().expect("#1 lands").to_string();
    until_file(&dir.join("started")).await;
    done_while_it_runs(&w, 2, &["b"]).await;
    let landed = land_while_it_runs(&w, 1).await;
    assert_eq!(landed[0]["landed"], "web#2", "#2 lands while the slow gate of #1's tip runs");
    let two = landed[0]["commit"].as_str().unwrap().to_string();
    assert_eq!(w.remote_tip("web", "proving"), two);
    assert_eq!(w.remote_tip("web", "main"), main, "the base waits for the slow gate");
    assert_eq!(queued_of_web(&w).await, ["slow gate of web"], "no second promotion of web starts beside it");
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("its gate ends");
    let t = w.tick_alone().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web" && l["tip"] == read.as_str()), "{:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), read, "the base moves to the tip it read, not past it");
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web" && l["tip"] == two.as_str()), "then #2's: {:?}", t.lines);
}

#[tokio::test]
async fn two_workers_land_while_a_cut_waits_on_its_remote_step_and_their_promotion_is_cut_next() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"true\"\n\n[cut]\nsteps = [\"{long}\"]\n").await;
    let one = lands_alone(&w).await.landed[0]["commit"].as_str().expect("#1 lands").to_string();
    until_file(&dir.join("started")).await;
    done_while_it_runs(&w, 2, &["b", "c"]).await;
    let landed = land_while_it_runs(&w, 2).await;
    let mut issues: Vec<_> = landed.iter().map(|l| l["landed"].as_str().unwrap()).collect();
    issues.sort();
    assert_eq!(issues, ["web#2", "web#3"], "both land while the cut runs");
    let last = landed[1]["commit"].as_str().unwrap().to_string();
    w.advance(1);
    w.tick_alone().await;
    assert_eq!(w.remote_tip("web", "main"), last, "and are promoted, the host idle");
    assert!(w.app.moves_run() && !dir.join("release").exists(), "the cut runs on");
    assert_eq!(queued_of_web(&w).await, ["cut of web"], "no second cut of web starts beside it");
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("its steps end");
    let t = w.tick_alone().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").unwrap_or_else(|| panic!("the cut: {:?}", t.lines));
    assert_eq!((&cut["version"], &cut["commit"]), (&"v0.1.0".into(), &one.as_str().into()), "of the base it read");
    let t = w.tick().await;
    let next = t.lines.iter().find(|l| l["cut"] == "web").unwrap_or_else(|| panic!("owed a cut: {:?}", t.lines));
    assert_eq!((&next["version"], &next["commit"]), (&"v0.1.1".into(), &last.as_str().into()), "{next}");
}

#[tokio::test]
async fn while_a_landing_runs_beside_the_ticks_the_next_of_its_checkout_stacks_on_it_and_another_checkouts_lands() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"{long}\"\n").await;
    w.tick_alone().await;
    until_file(&dir.join("started")).await;
    done_while_it_runs(&w, 2, &["b"]).await;
    // Another checkout's worker starts, and is done while web's landing runs.
    w.checkout("api").await;
    w.app.issue_add("api", "a", "", false, &[]).await.unwrap();
    assert!(w.tick_alone().await.started.is_some());
    w.done(&w.worker_on("api", 1).await, "a", "x").await;
    let mut landed = Vec::new();
    tokio::time::timeout(STUCK, async {
        while landed.is_empty() {
            w.advance(1);
            landed.extend(w.tick_alone().await.landed);
            assert_eq!(queued_of_web(&w).await, ["fast gate of web"; 2], "#2 stacked on #1, its gate asked");
            tokio::time::sleep(LOOK).await;
        }
    })
    .await
    .expect("api lands");
    assert_eq!(landed[0]["landed"], "api#1", "api's line moves beside web's landing: {landed:?}");
    std::fs::write(dir.join("release"), "").unwrap();
    let issues: Vec<_> =
        land_while_it_runs(&w, 2).await.iter().map(|l| l["landed"].as_str().unwrap().to_string()).collect();
    assert_eq!(issues, ["web#1", "web#2"], "then each of web, in its order");
}
