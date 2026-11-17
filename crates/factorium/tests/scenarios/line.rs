//! Promotion and cut: when they run, and what a red one leaves.

use std::path::PathBuf;

use serde_json::json;

use factorium::host::Host;

use crate::{World, id, worker};

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
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), file, "x");
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
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
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
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let main = w.remote_tip("web", "main");
    let wk = worker(&w.app).await;
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "SLOWRED", "x");
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
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let landed = land(&w, 1, "a").await;
    assert_eq!(w.app.promote("web").await.unwrap()["tip"], landed.as_str());
    assert_eq!(w.app.cut("web").await.unwrap()["version"], "v0.1.0");
    // Idle once #2 lands: that tick promotes and cuts by itself.
    let _ = land(&w, 2, "b").await;
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.1"]).is_ok());
    assert_eq!(w.app.cut("web").await.unwrap()["cut"], json!(null), "nothing new to cut");
    let version = w.git(&w.dir.join("remotes/web.git"), &["show", "v0.1.1:VERSION"]);
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
    w.git(&w.dir, &["clone", "-q", &web.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
    w.git(&clone, &["remote", "set-url", "origin", &w.dir.join("remotes/web.git").to_string_lossy()]).unwrap();
    w.git(&clone, &["fetch", "-q"]).unwrap();
    w.git(&clone, &["checkout", "-q", "-B", "proving", "origin/proving"]).unwrap();
    w.commit(&clone, "factorium.toml", toml);
    w.git(&clone, &["push", "-q", "origin", "proving"]).unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let t = {
        let wk = worker(&w.app).await;
        w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "a", "x");
        w.app.worker_done(id(&wk)).await.unwrap();
        w.tick().await
    };
    let failed = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    assert!(failed["failed"].as_str().unwrap().contains("exit 7"));
    assert!(w.app.doctor().await.unwrap()["findings"][0]["what"].as_str().unwrap().contains("cutting v0.1.0 failed"));
    assert!(w.tick().await.lines.is_empty(), "not tried again on every tick");
    assert!(w.host.brewed().is_empty());
}

/// Lands, promotes and cuts one change of `web`, whose cut installs the
/// formula `web`; answers the tick.
async fn cut_one(w: &World) -> crate::Tick {
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "a", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    w.tick().await
}

#[tokio::test]
async fn a_cut_of_the_daemons_own_formula_asks_for_a_restart() {
    let w = World::new();
    let bin = w.running_as("web");
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(t.restart.as_deref(), Some(&*bin.to_string_lossy()));
    let w = World::new();
    w.running_as("other");
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(t.restart, None, "another formula's cut leaves the daemon as it is");
}

#[tokio::test]
async fn a_cut_homebrew_does_not_link_fails_and_says_so() {
    let w = World::new();
    // Homebrew's upgrade succeeds yet it stays on the old version.
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    w.host.brew.lock().unwrap().stuck = true;
    let t = cut_one(&w).await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    let why = cut["failed"].as_str().expect("the cut fails");
    assert!(why.contains("web 0.0.9") && why.contains("not 0.1.0"), "{why}");
    assert_eq!(cut["installed"], json!(null));
    let upgrades = w.host.brewed().iter().filter(|b| *b == "upgrade web").count();
    assert_eq!(upgrades, 2, "an installed formula is upgraded, twice before it fails");
    let finding = w.app.doctor().await.unwrap()["findings"][0]["what"].as_str().unwrap().to_string();
    assert!(finding.contains("cutting v0.1.0 failed") && finding.contains("not 0.1.0"), "{finding}");
    assert_eq!(t.restart, None, "the daemon does not restart onto a version it lacks");
}

#[tokio::test]
async fn a_cut_installs_once_brew_update_sees_its_version() {
    let w = World::new();
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    // The first `brew update` sees the tap as it was: its upgrade does nothing.
    w.host.brew.lock().unwrap().behind = 1;
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(w.host.brew.lock().unwrap().installed["web"], "0.1.0");
    assert_eq!(w.host.brewed().iter().filter(|b| b.starts_with("update")).count(), 2);
}

#[tokio::test]
async fn the_lines_work_dirs_are_clones_not_worktrees_of_the_checkout() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    // A landing dir made the old way: a worktree of the checkout.
    let landing = w.dir.join("data").join("landing").join("web");
    std::fs::create_dir_all(landing.parent().unwrap()).unwrap();
    w.git(&web, &["worktree", "add", "--quiet", "--detach", &landing.to_string_lossy(), "HEAD"]).unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "a", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.landed.len(), 1, "{:?}", t.landed);
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    for kind in ["landing", "promotion", "cut"] {
        assert!(w.dir.join("data").join(kind).join("web").join(".git").is_dir(), "{kind} is a clone");
    }
    let listed = w.git(&web, &["worktree", "list", "--porcelain"]).unwrap();
    let trees: Vec<_> = listed.lines().filter_map(|l| l.strip_prefix("worktree ")).collect();
    assert_eq!(trees.len(), 1, "only the checkout itself: {trees:?}");
    assert!(trees[0].ends_with("repos/web"), "{trees:?}");
}

#[tokio::test]
async fn a_base_moved_outside_factorium_turns_promotion_red_and_stays() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    let why = w.app.promote("web").await.unwrap()["why"].as_str().unwrap().to_string();
    assert!(why.contains("nothing to promote"), "{why}");
    // #2 keeps the checkout busy, so #1's landing waits for the bound.
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let _ = land(&w, 1, "a").await;
    // Someone pushes to the base by hand.
    let clone = w.dir.join("outside");
    w.git(&w.dir, &["clone", "-q", &web.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
    w.git(&clone, &["remote", "set-url", "origin", &w.dir.join("remotes/web.git").to_string_lossy()]).unwrap();
    w.git(&clone, &["fetch", "-q"]).unwrap();
    w.git(&clone, &["checkout", "-q", "-B", "main", "origin/main"]).unwrap();
    w.commit(&clone, "hotfix", "x");
    w.git(&clone, &["push", "-q", "origin", "main"]).unwrap();
    let moved = w.remote_tip("web", "main");
    w.advance(61);
    let t = w.tick().await;
    let red = t.lines.iter().find(|l| l["promotion"] == "web").expect("a promotion ran");
    assert!(red["red"].as_str().unwrap().contains("moved outside factorium"), "{red}");
    assert_eq!(w.remote_tip("web", "main"), moved, "the base is not moved over");
    let doctor = w.app.doctor().await.unwrap();
    assert!(doctor["findings"][0]["what"].as_str().unwrap().contains("slow gate is red"), "{doctor}");
    assert!(w.tick().await.lines.is_empty(), "the same tip is not tried again");
}

#[tokio::test]
async fn one_landing_per_checkout_at_a_time_the_first_to_ask_first() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let workers = w.app.workers(false).await.unwrap();
    let of = |n: u64| workers.as_array().unwrap().iter().find(|x| x["number"] == n).cloned().unwrap();
    for n in [2, 1] {
        let wk = of(n);
        w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), &format!("f{n}"), "x");
        w.app.worker_done(id(&wk)).await.unwrap();
        w.advance(1);
    }
    let t = w.tick().await;
    assert_eq!(t.landed.len(), 1, "{:?}", t.landed);
    assert_eq!(t.landed[0]["landed"], "web#2", "#2 asked first");
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
}

/// Starts #1 and #2 of `web`, neither ever silent.
async fn two(w: &World) {
    w.checkout("web").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
}

#[tokio::test]
async fn a_worker_waiting_for_the_watch_leaves_its_checkout_idle() {
    let w = World::new();
    two(&w).await;
    let wk = w.app.workers(false).await.unwrap().as_array().unwrap().iter().find(|x| x["number"] == 2).cloned();
    w.app.worker_escalate(id(&wk.unwrap()), "which API version?").await.unwrap();
    let landed = land(&w, 1, "a").await;
    assert_eq!(w.remote_tip("web", "main"), landed, "promoted at once: #2 only waits");
}

#[tokio::test]
async fn an_idle_cut_waits_until_everything_landed_is_promoted() {
    let w = World::new();
    two(&w).await;
    let _ = land(&w, 1, "a").await;
    w.advance(61);
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "#2 still works: no cut");
    // #2 lands a change whose slow gate is red: idle, but not all promoted.
    let _ = land(&w, 2, "SLOWRED").await;
    w.tick().await;
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.0"]).is_err(), "no version is cut");
    // Its own bound passed, it is cut whatever is unpromoted.
    w.advance(24 * 60);
    w.tick().await;
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.0"]).is_ok(), "cut once the bound passed");
}

#[tokio::test]
async fn factorium_commits_as_the_hosts_own_identity_or_as_itself_on_a_fresh_host() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    let author = |sha: &str| w.git(&w.dir.join("remotes/web.git"), &["log", "-1", "--format=%an <%ae>", sha]).unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let landed = land(&w, 1, "a").await;
    assert_eq!(author(&landed), "factorium <factorium@localhost>", "a fresh host has no identity");

    std::fs::write(w.host.home().join(".gitconfig"), "[user]\n\tname = Ann\n\temail = ann@host\n").unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    let landed = land(&w, 2, "b").await;
    assert_eq!(author(&landed), "Ann <ann@host>", "the host's config is its home's");
}
