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

/// Lands, promotes and cuts one change of `web`, whose cut installs the
/// formula `web`, while the program is `running`'s binary; answers the tick.
async fn cut_while_running(running: &str) -> crate::Tick {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    *w.host.running.lock().unwrap() = Some(running.into());
    w.app.issue_add("web", "a", "", false).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "a", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    t
}

#[tokio::test]
async fn a_cut_of_the_daemons_own_formula_asks_for_a_restart() {
    let t = cut_while_running("web").await;
    assert_eq!(t.restart.as_deref(), Some("/brew/opt/web/bin/web"));
    let t = cut_while_running("other").await;
    assert_eq!(t.restart, None, "another formula's cut leaves the daemon as it is");
}

#[tokio::test]
async fn a_cut_homebrew_does_not_link_fails_and_says_so() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    // Homebrew's upgrade succeeds yet it stays on the old version.
    *w.host.brew_stuck.lock().unwrap() = Some("0.0.9".into());
    w.app.issue_add("web", "a", "", false).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "a", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    let why = cut["failed"].as_str().expect("the cut fails");
    assert!(why.contains("web 0.0.9") && why.contains("not 0.1.0"), "{why}");
    assert_eq!(cut["installed"], json!(null));
    let finding = w.app.doctor().await.unwrap()["findings"][0]["what"].as_str().unwrap().to_string();
    assert!(finding.contains("cutting v0.1.0 failed") && finding.contains("not 0.1.0"), "{finding}");
    assert_eq!(t.restart, None, "the daemon does not restart onto a version it lacks");
}

#[tokio::test]
async fn the_lines_work_dirs_are_clones_not_worktrees_of_the_checkout() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    // A landing dir made the old way: a worktree of the checkout.
    let landing = w.dir.join("data").join("landing").join("web");
    std::fs::create_dir_all(landing.parent().unwrap()).unwrap();
    git(&web, &["worktree", "add", "--quiet", "--detach", &landing.to_string_lossy(), "HEAD"]).unwrap();
    w.app.issue_add("web", "a", "", false).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "a", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.landed.len(), 1, "{:?}", t.landed);
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    for kind in ["landing", "promotion", "cut"] {
        assert!(w.dir.join("data").join(kind).join("web").join(".git").is_dir(), "{kind} is a clone");
    }
    let listed = git(&web, &["worktree", "list", "--porcelain"]).unwrap();
    let trees: Vec<_> = listed.lines().filter_map(|l| l.strip_prefix("worktree ")).collect();
    assert_eq!(trees.len(), 1, "only the checkout itself: {trees:?}");
    assert!(trees[0].ends_with("repos/web"), "{trees:?}");
}
