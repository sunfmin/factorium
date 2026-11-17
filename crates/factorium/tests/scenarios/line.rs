//! Promotion and cut: when they run, and what a red one leaves.

use std::path::PathBuf;

use serde_json::json;

use crate::{World, commit, git, id, worker};

/// Lands one commit of `file` through a worker; answers the landed commit.
async fn land(w: &World, number: u64, file: &str) -> String {
    let wk = w
        .app
        .workers(false)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["number"] == number)
        .cloned()
        .unwrap();
    commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), file, "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    t.landed.iter().find(|l| l["landed"] == format!("web#{number}")).expect("landed")["commit"].as_str().unwrap().into()
}

#[tokio::test]
async fn a_busy_checkout_promotes_once_the_oldest_landing_waited_the_bound() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    // #2 keeps working all along: it is never silent.
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false).await.unwrap();
    w.app.issue_add("web", "b", "", false).await.unwrap();
    w.tick().await;
    w.tick().await;
    let main = w.remote_tip("web", "main");
    let landed = land(&w, 1, "a").await;
    assert_eq!(w.remote_tip("web", "main"), main, "#2 still works: not idle");
    w.advance(61);
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "a cut waits for idle or its own bound");
}

#[tokio::test]
async fn a_red_slow_gate_holds_the_base_and_is_a_finding_until_a_new_tip() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false).await.unwrap();
    w.tick().await;
    let main = w.remote_tip("web", "main");
    let wk = worker(&w.app).await;
    commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "SLOWRED", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1", "the fast gate is green");
    assert!(t.lines.iter().any(|l| l["promotion"] == "web" && l["red"].is_string()), "{:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), main);
    let doctor = w.app.doctor().await.unwrap();
    assert!(doctor["findings"][0]["what"].as_str().unwrap().contains("slow gate is red"));
    assert!(w.tick().await.lines.is_empty(), "the same tip is not gated again");
}

#[tokio::test]
async fn promote_and_cut_by_hand_run_now() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    assert_eq!(w.app.promote("web").await.unwrap()["promoted"], json!(null));
    w.app.issue_add("web", "a", "", false).await.unwrap();
    w.app.issue_add("web", "b", "", false).await.unwrap();
    w.tick().await;
    w.tick().await;
    let landed = land(&w, 1, "a").await;
    assert_eq!(w.app.promote("web").await.unwrap()["tip"], landed.as_str());
    assert_eq!(w.app.cut("web").await.unwrap()["version"], "v0.1.0");
    // Idle once #2 lands: that tick promotes and cuts by itself.
    let _ = land(&w, 2, "b").await;
    assert!(git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.1"]).is_ok());
    assert_eq!(w.app.cut("web").await.unwrap()["cut"], json!(null), "nothing new to cut");
    let version = git(&w.dir.join("remotes/web.git"), &["show", "v0.1.1:VERSION"]);
    assert!(version.is_err(), "a step's own changes stay its own: the tag is on what it committed");
}

#[tokio::test]
async fn a_failed_cut_is_a_finding_and_waits_for_a_new_promotion() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    // The repo now declares a cut whose step fails.
    let toml = "[gate]\nfast = \"true\"\n\n[cut]\nsteps = [\"exit 7\"]\n";
    let clone = w.dir.join("edit");
    git(&w.dir, &["clone", "-q", &web.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
    git(&clone, &["remote", "set-url", "origin", &w.dir.join("remotes/web.git").to_string_lossy()]).unwrap();
    git(&clone, &["fetch", "-q"]).unwrap();
    git(&clone, &["checkout", "-q", "-B", "proving", "origin/proving"]).unwrap();
    commit(&clone, "factorium.toml", toml);
    git(&clone, &["push", "-q", "origin", "proving"]).unwrap();
    w.app.issue_add("web", "a", "", false).await.unwrap();
    w.tick().await;
    let t = {
        let wk = worker(&w.app).await;
        commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "a", "x");
        w.app.worker_done(id(&wk)).await.unwrap();
        w.tick().await
    };
    let failed = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    assert!(failed["failed"].as_str().unwrap().contains("exit 7"));
    assert!(w.app.doctor().await.unwrap()["findings"][0]["what"].as_str().unwrap().contains("cutting v0.1.0 failed"));
    assert!(w.tick().await.lines.is_empty(), "not tried again on every tick");
    assert!(w.host.brewed.lock().unwrap().is_empty());
}
