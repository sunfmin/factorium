//! One cut of a checkout at a time, across processes: a cut holds the
//! checkout's cut lock from before it reads its line until it is recorded, so
//! `factorium cut` and the daemon's cut never run at once in its clone. A
//! promotion by hand leaves its cut to a cut by hand until the cut bound.

use std::path::Path;

use serde_json::json;

use crate::fixture::{STUCK, World, id, until_file, worker};

/// How many times the cut step holding in `dir` started.
fn runs(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("runs")).map_or(0, |s| s.lines().count())
}

/// A factorium.toml whose cut commits its version, then holds in `dir`,
/// counting its runs there.
fn holding_cut(w: &World, dir: &Path) -> String {
    let version = "git -c user.name=f -c user.email=f@f commit -q --allow-empty -m version";
    let long = format!("echo run >> {}/runs; {}", dir.display(), w.holds(dir));
    format!("[gate]\nfast = \"true\"\n\n[cut]\nsteps = [\"{version}\", \"{long}\"]\n")
}

#[tokio::test]
async fn cut_by_hand_while_the_daemons_cut_runs_is_refused_and_one_version_commit_is_tagged() {
    let w = World::new();
    let dir = w.dir.join("long");
    w.checkout("web").await;
    w.kit().await;
    w.push_by_hand("web", "proving", "factorium.toml", &holding_cut(&w, &dir));
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick_alone().await;
    w.done(&worker(&w.app).await, "a", "x").await;
    w.gated().await;
    // Lands #1 and promotes it, idle: the daemon's cut starts beside the ticks.
    let t = w.tick_alone().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    until_file(&dir.join("started")).await;
    let refused = w.run().cut("web").await.unwrap();
    assert_eq!(refused["cut"], json!(null), "{refused}");
    let why = refused["why"].as_str().unwrap();
    assert!(why.starts_with("a cut of web runs: the daemon's cut, pid "), "{why}");
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("its steps end");
    let t = w.tick_alone().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").unwrap_or_else(|| panic!("the cut: {:?}", t.lines));
    assert_eq!(cut["version"], "v0.1.0", "{cut}");
    assert_eq!(runs(&dir), 1, "one cut ran its steps");
    let remote = w.dir.join("remotes/web.git");
    assert_eq!(w.git(&remote, &["log", "-1", "--format=%s", "v0.1.0"]).unwrap().trim(), "version", "tagged on it");
    assert_eq!(w.git(&remote, &["rev-list", "--count", "main..v0.1.0"]).unwrap().trim(), "1", "one version commit");
}

#[tokio::test]
async fn the_daemon_starts_no_cut_while_cut_by_hand_runs() {
    let w = World::new();
    // #2 keeps working all along: the daemon cuts only once the bound passed.
    w.two_at_work().await;
    let dir = w.dir.join("long");
    w.done(&w.worker_on("web", 1).await, "factorium.toml", &holding_cut(&w, &dir)).await;
    assert_eq!(w.tick().await.landed.len(), 1);
    assert_eq!(w.run().promote("web").await.unwrap()["promoted"], "web");
    let cli = w.run();
    let by_hand = cli.cut("web");
    let ticks = async {
        until_file(&dir.join("started")).await;
        // Past the cut bound: the daemon's cut is due however busy.
        w.advance(24 * 60 + 1);
        let t = w.tick().await;
        std::fs::write(dir.join("release"), "").unwrap();
        t
    };
    let (cut, t) = tokio::time::timeout(STUCK, async { tokio::join!(by_hand, ticks) }).await.expect("ends");
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "{:?}", t.lines);
    assert_eq!(cut.unwrap()["version"], "v0.1.0");
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("nothing runs on");
    assert_eq!(runs(&dir), 1, "one cut ran its steps");
    assert!(!w.tick().await.lines.iter().any(|l| l["cut"] == "web"), "nothing left to cut");
}

#[tokio::test]
async fn after_a_promotion_by_hand_the_daemon_leaves_the_cut_to_one_by_hand_until_the_cut_bound() {
    let w = World::new();
    w.two_at_work().await;
    let dir = w.dir.join("long");
    w.done(&w.worker_on("web", 1).await, "factorium.toml", &holding_cut(&w, &dir)).await;
    assert_eq!(w.tick().await.landed.len(), 1);
    assert_eq!(w.run().promote("web").await.unwrap()["promoted"], "web");
    // #2 waits for the watch: the checkout is idle, the promotion by hand's.
    w.app.worker_escalate(id(&w.worker_on("web", 2).await), "which way?").await.unwrap();
    w.advance(1);
    assert!(!w.tick().await.lines.iter().any(|l| l["cut"] == "web"), "the gap is left to the hand");
    assert_eq!(runs(&dir), 0);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("release"), "").unwrap();
    assert_eq!(w.run().cut("web").await.unwrap()["version"], "v0.1.0");
    assert_eq!(runs(&dir), 1, "the cut by hand is the one that ran");
}

#[tokio::test]
async fn a_promotion_by_hand_left_uncut_is_cut_by_the_daemon_at_the_cut_bound() {
    let w = World::new();
    w.two_at_work().await;
    w.done(&w.worker_on("web", 1).await, "a", "x").await;
    assert_eq!(w.tick().await.landed.len(), 1);
    assert_eq!(w.run().promote("web").await.unwrap()["promoted"], "web");
    w.app.worker_escalate(id(&w.worker_on("web", 2).await), "which way?").await.unwrap();
    w.advance(1);
    assert!(!w.tick().await.lines.iter().any(|l| l["cut"] == "web"), "left to the hand");
    w.advance(24 * 60);
    let t = w.tick().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").unwrap_or_else(|| panic!("cut at the bound: {:?}", t.lines));
    assert_eq!(cut["version"], "v0.1.0", "{cut}");
}
