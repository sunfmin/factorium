//! Promotion and cut: when they run, and what a red one leaves.

use serde_json::json;

use factorium::LineClone;
use factorium::host::Host;
use factorium_domain::neutral::landing::{Bounds, Due, Line, LineEvent, Look, Move};

use crate::fixture::{CLAUDE, NEEDS_NEWER, World, findings, id, proved, worker, worktree};

#[tokio::test]
async fn a_red_slow_gate_holds_the_base_and_is_a_finding_until_a_new_tip() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    let main = w.remote_tip("web", "main");
    let wk = worker(&w.app).await;
    w.done(&wk, "SLOWRED", "x").await;
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1", "the fast gate is green");
    assert!(t.lines.iter().any(|l| l["promotion"] == "web" && l["red"].is_string()), "{:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), main);
    let doctor = w.app.doctor().await.unwrap();
    assert!(findings(&doctor)[0]["what"].as_str().unwrap().contains("slow gate is red"));
    w.advance(61);
    assert!(w.tick().await.lines.is_empty(), "the same tip is not gated again");
    assert_eq!(w.remote_tip("web", "main"), main, "nor promoted while it is the red one");
    let again = w.app.promote("web").await.unwrap();
    assert!(again["red"].is_string(), "by hand it is gated again: {again}");

    // A new tip, the red fixed: promoted.
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let tree = worktree(&wk);
    w.git(&tree, &["rm", "-q", "SLOWRED"]).unwrap();
    w.git(&tree, &["commit", "-q", "-m", "green"]).unwrap();
    w.app.worker_done(id(&wk), None).await.unwrap();
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
}

/// Seen 2026-11-22 (factorium#202): a red slow gate's finding carried only
/// its tail, and what named the failing seed was above it.
#[tokio::test]
async fn a_red_slow_gate_names_the_file_that_keeps_its_whole_output_and_old_ones_go() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    let outputs = w.data().gate_outputs("web");
    std::fs::create_dir_all(&outputs).unwrap();
    let old: Vec<_> = (0..25).map(|n| outputs.join(format!("{n:013}-slow-old.log"))).collect();
    for o in &old {
        std::fs::write(o, "old\n").unwrap();
    }
    let toml = "[gate]\nfast = \"true\"\nslow = \"echo the first line; seq 2 200; exit 1\"\n";
    w.done(&worker(&w.app).await, "factorium.toml", toml).await;
    let t = w.tick().await;
    let red = t.lines.iter().find(|l| l["promotion"] == "web").expect("a promotion ran")["red"].clone();
    let red = red.as_str().unwrap();
    assert!(!red.contains("the first line"), "past the tail: {red}");
    let doctor = w.app.doctor().await.unwrap();
    let what = findings(&doctor)[0]["what"].as_str().unwrap().to_string();
    let path = what.rsplit_once("whole output: ").expect("names the file").1.trim();
    let whole = std::fs::read_to_string(path).unwrap();
    assert!(whole.starts_with("the first line\n2\n") && whole.ends_with("200\n"), "{whole}");
    let left = std::fs::read_dir(&outputs).unwrap().count();
    assert_eq!(left, 20, "the newest 20 are kept");
    // The landing's fast gate and the promotion's slow gate are the newest two.
    assert!(!old[6].exists() && old[7].exists(), "the oldest go first");
}

#[tokio::test]
async fn promote_and_cut_by_hand_run_now_and_factorium_commits_as_the_hosts_identity_or_its_own() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    let author = |sha: &str| w.git(&w.dir.join("remotes/web.git"), &["log", "-1", "--format=%an <%ae>", sha]).unwrap();
    assert_eq!(w.app.promote("web").await.unwrap()["promoted"], json!(null));
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let landed = w.lands("web", 1, "a").await.commit;
    assert_eq!(author(&landed), "factorium <factorium@localhost>", "a fresh host has no identity");
    assert_eq!(w.app.promote("web").await.unwrap()["tip"], landed.as_str());
    assert_eq!(w.app.cut("web").await.unwrap()["version"], "v0.1.0");
    let mut config = std::fs::OpenOptions::new().append(true).open(w.host.home().join(".gitconfig")).unwrap();
    std::io::Write::write_all(&mut config, b"[user]\n\tname = Ann\n\temail = ann@host\n").unwrap();
    // Idle once #2 lands: that tick promotes and cuts by itself.
    let landed = w.lands("web", 2, "b").await.commit;
    assert_eq!(author(&landed), "Ann <ann@host>", "the host's config is its home's");
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.1"]).is_ok());
    assert_eq!(w.app.cut("web").await.unwrap()["cut"], json!(null), "nothing new to cut");
    let version = w.git(&w.dir.join("remotes/web.git"), &["show", "v0.1.1:VERSION"]);
    assert!(version.is_err(), "a step's own changes stay its own: the tag is on what it committed");
}

#[tokio::test]
async fn a_failed_cut_is_a_finding_and_waits_for_a_new_promotion() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    // The repo now declares a cut whose step fails.
    let toml = "[gate]\nfast = \"true\"\n\n[cut]\nsteps = [\"true\", \"exit 7\"]\n";
    w.push_by_hand("web", "proving", "factorium.toml", toml);
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let t = {
        let wk = worker(&w.app).await;
        w.done(&wk, "a", "x").await;
        w.tick().await
    };
    let failed = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    assert!(failed["failed"].as_str().unwrap().contains("exit 7"));
    assert!(w.findings().await[0]["what"].as_str().unwrap().contains("cutting v0.1.0 failed"));
    assert!(w.tick().await.lines.is_empty(), "not tried again on every tick");
    assert!(w.host.brewed().is_empty());
    // The fix lands and is promoted: the settled line cuts again by itself (factorium#251).
    w.push_by_hand("web", "proving", "factorium.toml", "[gate]\nfast = \"true\"\n\n[cut]\nsteps = [\"true\"]\n");
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    let t = {
        let wk = worker(&w.app).await;
        w.done(&wk, "b", "y").await;
        w.tick().await
    };
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("cut again after the promotion");
    assert_eq!((cut["version"].as_str(), cut["failed"].as_str()), (Some("v0.1.0"), None), "{cut}");
    assert!(w.findings().await.is_empty(), "{:?}", w.findings().await);
}

/// Lands, promotes and cuts one change of `web`, whose cut installs the
/// formula `web`; answers the tick.
async fn cut_one(w: &World) -> factorium::Tick {
    let wk = w.working(CLAUDE, "a").await;
    w.done(&wk, "a", "x").await;
    w.tick().await
}

#[tokio::test]
async fn a_cut_of_the_daemons_own_formula_asks_for_a_restart() {
    let w = World::new();
    let bin = w.running_as(&w.app, "web");
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(t.restart.as_deref(), Some(&*bin.to_string_lossy()));
    // The new binary was proven on a copy of the database, not the live one, gone once proven.
    let proved = proved(&w, "web", "0.1.0");
    assert_eq!(proved.len(), 1, "once, the cut's proof the swap takes: {proved:?}");
    assert!(proved[0] != w.db && proved[0].starts_with(w.data().root()), "{proved:?}");
    assert!(!proved[0].exists(), "a proven copy is not kept");
    let w = World::new();
    w.running_as(&w.app, "other");
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(t.restart, None, "another formula's cut leaves the daemon as it is");
}

#[tokio::test]
async fn a_cut_from_the_cli_restarts_the_daemon_onto_the_new_binary() {
    let w = World::new();
    let bin = w.running_as(&w.app, "web");
    w.checkout("web").await;
    w.kit().await;
    // Landed, and uncut while the checkout is busy: the daemon does not cut.
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    assert!(w.tick().await.started.is_some(), "b works on");
    w.done(&wk, "a", "x").await;
    let t = w.tick().await;
    assert!(!t.landed.is_empty(), "{t:?}");
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "{:?}", t.lines);
    assert_eq!(t.restart, None);
    // The owner, or the watch, cuts by hand in another process.
    w.run().promote("web").await.unwrap();
    let cut = w.run().cut("web").await.unwrap();
    assert_eq!(cut["installed"], "web", "{cut}");
    let t = w.tick().await;
    assert_eq!(t.restart.as_deref(), Some(&*bin.to_string_lossy()), "the next tick swaps the binary");
}

/// v0.1.81 counted as installed while PATH's `factorium` still linked the
/// keg before, which Homebrew had removed: every `factorium` on PATH was
/// gone (factorium#244). The cut relinks it once.
#[tokio::test]
async fn a_cut_whose_path_link_is_left_on_a_removed_keg_relinks_it() {
    let w = World::new();
    let bin = w.running_as(&w.app, "web");
    w.host.brew.lock().unwrap().stale_link = true;
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(w.host.brewed().iter().filter(|b| *b == "link --overwrite web").count(), 1);
    let on_path = w.dir.join("brew/bin/factorium").canonicalize().unwrap();
    assert_eq!(on_path, w.dir.join("brew/Cellar/web/0.1.0/bin/factorium").canonicalize().unwrap());
    assert_eq!(t.restart.as_deref(), Some(&*bin.to_string_lossy()));
}

#[tokio::test]
async fn a_cut_whose_path_link_relinking_does_not_fix_fails_naming_the_link() {
    let w = World::new();
    w.running_as(&w.app, "web");
    {
        let mut brew = w.host.brew.lock().unwrap();
        (brew.stale_link, brew.relink_fails) = (true, true);
    }
    let t = cut_one(&w).await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    let why = cut["failed"].as_str().expect("the cut fails");
    let link = w.dir.join("brew/bin/factorium");
    assert!(why.contains(&format!("`{}` points to `../Cellar/web/0.0.9/bin/factorium`", link.display())), "{why}");
    assert!(why.contains("after `brew link --overwrite web`"), "{why}");
    let finding = w.findings().await[0]["what"].as_str().unwrap().to_string();
    assert!(finding.contains("cutting v0.1.0 failed") && finding.contains("brew/bin/factorium"), "{finding}");
    // Read back, it is not made while the link stays stale.
    let again = w.tick().await;
    assert!(!again.lines.iter().any(|l| l["read_back"] == true), "{:?}", again.lines);
}

/// A version whose store needed a column the host's database lacked: once
/// swapped in, every tick and command failed (factorium#141).
#[tokio::test]
async fn a_cut_whose_binary_cannot_open_the_hosts_database_fails_and_the_old_one_runs_on() {
    let w = World::new();
    w.running_as(&w.app, "web");
    w.next_binary("web", NEEDS_NEWER);
    let t = cut_one(&w).await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    let why = cut["failed"].as_str().expect("the cut fails");
    assert!(why.contains("no such column: handed_back"), "{why}");
    assert_eq!(t.restart, None, "the old binary runs on");
    // It ran on a copy, kept and named for a look.
    let ran_on = proved(&w, "web", "0.1.0");
    assert_eq!(ran_on.len(), 1, "{ran_on:?}");
    let copy = &ran_on[0];
    assert!(copy != &w.db && copy.exists(), "{copy:?}");
    assert_eq!(std::fs::read(copy).unwrap()[..16], *b"SQLite format 3\0");
    let finding = w.findings().await[0]["what"].as_str().unwrap().to_string();
    assert!(finding.contains("cutting v0.1.0 failed") && finding.contains("no such column: handed_back"), "{finding}");
    assert!(finding.contains(&*copy.to_string_lossy()), "it names the copy: {finding}");
    // Another process on the old binary (the daemon, restarted) reads how it was proven, and
    // does not swap: one proof per binary on the host (factorium#241).
    let daemon = w.run();
    daemon.runs_as(&w.dir.join("brew/Cellar/web/0.0.9/bin/factorium").canonicalize().unwrap());
    assert_eq!(daemon.tick().await.unwrap().restart, None);
    assert_eq!(daemon.tick().await.unwrap().restart, None);
    assert_eq!(proved(&w, "web", "0.1.0").len(), 1, "not proven again");
}

/// The `sleep`s run since the cut's first `brew update`: its waits for the tap.
fn brew_waits(w: &World) -> Vec<String> {
    let ran = w.host.ran.lock().unwrap();
    let from = ran.iter().position(|r| r.starts_with("brew update")).unwrap_or(ran.len());
    ran[from..].iter().filter(|r| r.starts_with("sleep ")).cloned().collect()
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
    assert!(why.contains("for 90s"), "it names the bound waited: {why}");
    let upgrades = w.host.brewed().iter().filter(|b| *b == "upgrade web").count();
    assert_eq!(upgrades, 7, "an installed formula is upgraded, with backoff, before it fails");
    let slept = brew_waits(&w);
    assert_eq!(slept, ["sleep 2", "sleep 4", "sleep 8", "sleep 16", "sleep 30", "sleep 30"]);
    let finding = w.findings().await[0]["what"].as_str().unwrap().to_string();
    assert!(finding.contains("cutting v0.1.0 failed") && finding.contains("not 0.1.0"), "{finding}");
    assert_eq!(t.restart, None, "the daemon does not restart onto a version it lacks");
}

#[tokio::test]
async fn a_cut_installs_once_brew_update_sees_its_version() {
    let w = World::new();
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    // The tap is seconds behind: only the 3rd `brew update` sees the new formula.
    w.host.brew.lock().unwrap().behind = 2;
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(w.host.brew.lock().unwrap().installed["web"], "0.1.0");
    assert_eq!(w.host.brewed().iter().filter(|b| b.starts_with("update")).count(), 3);
    let slept = brew_waits(&w);
    assert_eq!(slept, ["sleep 2", "sleep 4"], "it backs off between tries");
    assert!(t.lines.iter().all(|l| l["failed"].is_null()), "no CutFailed: {:?}", t.lines);
    let doctor = w.app.doctor().await.unwrap();
    assert!(!doctor.to_string().contains("cutting v0.1.0 failed"), "no CutFailed: {doctor}");
}

/// Another brew held the formula's lock as the cut installed, and freed it
/// seconds later (factorium#173).
#[tokio::test]
async fn a_cut_waits_for_a_homebrew_lock_another_brew_holds() {
    let w = World::new();
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    w.host.brew.lock().unwrap().locked = 1;
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(w.host.brew.lock().unwrap().installed["web"], "0.1.0");
    assert_eq!(w.host.brewed().iter().filter(|b| *b == "upgrade web").count(), 2, "tried again once freed");
    assert_eq!(brew_waits(&w), ["sleep 2"], "it backs off before trying again");
    assert!(t.lines.iter().all(|l| l["failed"].is_null()), "no CutFailed: {:?}", t.lines);
}

#[tokio::test]
async fn a_cut_whose_version_the_lock_holder_installs_is_made() {
    let w = World::new();
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    {
        let mut brew = w.host.brew.lock().unwrap();
        (brew.locked, brew.holder_installs) = (1, true);
    }
    let t = cut_one(&w).await;
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    assert_eq!(w.host.brew.lock().unwrap().installed["web"], "0.1.0");
    assert_eq!(w.host.brewed().iter().filter(|b| *b == "upgrade web").count(), 1, "the holder installed it");
    assert!(brew_waits(&w).is_empty(), "nothing left to wait for");
    assert!(t.lines.iter().all(|l| l["failed"].is_null()), "no CutFailed: {:?}", t.lines);
}

#[tokio::test]
async fn a_cut_still_locked_past_the_bound_fails_naming_the_holder() {
    let w = World::new();
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    {
        let mut brew = w.host.brew.lock().unwrap();
        brew.locked = u32::MAX;
        brew.ps = "  101 /usr/sbin/sshd\n 4242 /h/homebrew/Library/Homebrew/vendor/ruby/bin/ruby brew.rb upgrade sunfmin/tap/web\n 300 vim web\n 301 /h/homebrew/bin/brew upgrade api\n".into();
    }
    let t = cut_one(&w).await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    let why = cut["failed"].as_str().expect("the cut fails");
    assert!(why.contains("has already locked") && why.contains("still locked after 90s"), "{why}");
    assert!(why.contains("held by 4242 ") && why.contains("upgrade sunfmin/tap/web"), "it names the holder: {why}");
    for other in ["sshd", "vim", "api"] {
        assert!(!why.contains(other), "only brew's processes on the formula: {why}");
    }
    assert_eq!(w.host.brewed().iter().filter(|b| *b == "upgrade web").count(), 7);
    assert_eq!(cut["installed"], json!(null));
}

#[tokio::test]
async fn a_cut_whose_upgrade_fails_other_than_by_a_lock_fails_at_once() {
    let w = World::new();
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    w.host.brew.lock().unwrap().refuses = Some("Error: web: SHA256 mismatch".into());
    let t = cut_one(&w).await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    let why = cut["failed"].as_str().expect("the cut fails");
    assert!(why.contains("SHA256 mismatch"), "{why}");
    assert_eq!(w.host.brewed().iter().filter(|b| *b == "upgrade web").count(), 1, "not waited for as a lock");
    assert!(brew_waits(&w).is_empty());
}

/// Its tag pushed, the cut's install is refused by brew (truefit's brew
/// fault `upgrade-refused`): the cut fails with brew's words and is a
/// finding, its tag stays on origin, and once brew installs that version
/// the next tick reads it back, with no new cut.
#[tokio::test]
async fn a_cut_whose_install_brew_refuses_after_its_tag_keeps_the_tag_and_is_read_back() {
    let w = World::new();
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    w.host.brew.lock().unwrap().fault("upgrade-refused", true);
    let t = cut_one(&w).await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    let why = cut["failed"].as_str().expect("the cut fails");
    assert!(why.contains("installing web failed") && why.contains("curl"), "{why}");
    let tag = w.remote_tip("web", "v0.1.0^{commit}");
    assert!(w.finding("cut:web:v0.1.0").await.is_some(), "{:?}", w.findings().await);
    assert_eq!(w.host.brew.lock().unwrap().installed["web"], "0.0.9");

    w.host.brew.lock().unwrap().fault("upgrade-refused", false);
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.1.0".into());
    let t = w.tick().await;
    let read = t.lines.iter().find(|l| l["cut"] == "web").expect("the failure is read back");
    assert_eq!((&read["read_back"], &read["commit"]), (&json!(true), &json!(tag)), "{read}");
    assert_eq!(w.finding("cut:web:v0.1.0").await, None);
}

/// A cut failed on the Homebrew lock, and the brew that held it then
/// installed that version: the next tick reads the install back and clears
/// the failure, without a new cut (factorium#185).
#[tokio::test]
async fn a_failed_cut_whose_version_got_installed_anyway_is_cleared() {
    let w = World::new();
    w.host.brew.lock().unwrap().installed.insert("web".into(), "0.0.9".into());
    w.host.brew.lock().unwrap().locked = u32::MAX;
    let t = cut_one(&w).await;
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    assert!(cut["failed"].as_str().is_some_and(|f| f.contains("has already locked")), "{cut}");
    let finding = || async { w.finding("cut:web:v0.1.0").await.is_some() };
    assert!(finding().await, "{:?}", w.findings().await);
    // Still not installed: the next tick leaves the failure as it is.
    let t = w.tick().await;
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "{:?}", t.lines);
    assert!(finding().await, "it still fails: {:?}", w.findings().await);
    // The other brew installs it and frees the lock.
    {
        let mut brew = w.host.brew.lock().unwrap();
        brew.locked = 0;
        brew.installed.insert("web".into(), "0.1.0".into());
    }
    let upgrades = || w.host.brewed().iter().filter(|b| *b == "upgrade web").count();
    let before = upgrades();
    let t = w.tick().await;
    let read = t.lines.iter().find(|l| l["cut"] == "web").expect("the failure is read back");
    assert_eq!(
        (&read["version"], &read["read_back"], &read["installed"]),
        (&json!("v0.1.0"), &json!(true), &json!("web"))
    );
    assert_eq!(upgrades(), before, "no new cut, no new install");
    assert!(!finding().await, "{:?}", w.findings().await);
    let t = w.tick().await;
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "the line counts as cut: {:?}", t.lines);
}

#[tokio::test]
async fn the_lines_work_dirs_are_clones_not_worktrees_of_the_checkout() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    // A landing dir made the old way: a worktree of the checkout.
    let landing = w.data().clone_of(LineClone::Landing, "web");
    std::fs::create_dir_all(landing.parent().unwrap()).unwrap();
    w.git(&web, &["worktree", "add", "--quiet", "--detach", &landing.to_string_lossy(), "HEAD"]).unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.done(&wk, "a", "x").await;
    let t = w.tick().await;
    assert_eq!(t.landed.len(), 1, "{:?}", t.landed);
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["installed"] == "web"), "{:?}", t.lines);
    for kind in [LineClone::Landing, LineClone::Promotion, LineClone::Cut] {
        assert!(w.data().clone_of(kind, "web").join(".git").is_dir(), "{kind:?} is a clone");
    }
    let listed = w.git(&web, &["worktree", "list", "--porcelain"]).unwrap();
    let trees: Vec<_> = listed.lines().filter_map(|l| l.strip_prefix("worktree ")).collect();
    assert_eq!(trees.len(), 1, "only the checkout itself: {trees:?}");
    assert!(trees[0].ends_with("repos/web"), "{trees:?}");
}

#[tokio::test]
async fn a_base_moved_outside_factorium_turns_promotion_red_and_stays() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    let why = w.app.promote("web").await.unwrap()["why"].as_str().unwrap().to_string();
    assert!(why.contains("nothing to promote"), "{why}");
    // #2 keeps the checkout busy, so #1's landing waits for the bound.
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    w.lands("web", 1, "a").await;
    // Someone pushes to the base by hand.
    w.push_by_hand("web", "main", "hotfix", "x");
    let moved = w.remote_tip("web", "main");
    w.advance(61);
    let t = w.tick().await;
    let red = t.lines.iter().find(|l| l["promotion"] == "web").expect("a promotion ran");
    assert!(red["red"].as_str().unwrap().contains("moved outside factorium"), "{red}");
    assert_eq!(w.remote_tip("web", "main"), moved, "the base is not moved over");
    let doctor = w.app.doctor().await.unwrap();
    assert!(findings(&doctor)[0]["what"].as_str().unwrap().contains("slow gate is red"), "{doctor}");
    assert!(w.tick().await.lines.is_empty(), "the same tip is not tried again");
}

#[tokio::test]
async fn one_landing_per_checkout_at_a_time_the_first_to_ask_first() {
    let w = World::new();
    w.two_at_work().await;
    for n in [2, 1] {
        let wk = w.worker_on("web", n).await;
        w.done(&wk, &format!("f{n}"), "x").await;
        w.advance(1);
    }
    let t = w.tick().await;
    assert_eq!(t.landed.len(), 1, "{:?}", t.landed);
    assert_eq!(t.landed[0]["landed"], "web#2", "#2 asked first");
    assert!(t.lines.is_empty(), "#1 waits to land: not idle, nothing promoted: {:?}", t.lines);
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1");
    // Idle once #1 lands: one promotion takes both landings and, nothing left unpromoted, cuts.
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    assert!(t.lines.iter().any(|l| l["cut"] == "web" && l["version"] == "v0.1.0"), "{:?}", t.lines);
}

#[tokio::test]
async fn a_worker_waiting_for_the_watch_leaves_its_checkout_idle() {
    let w = World::new();
    w.two_at_work().await;
    let wk = w.worker_on("web", 2).await;
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    let landed = w.lands("web", 1, "a").await.commit;
    assert_eq!(w.remote_tip("web", "main"), landed, "promoted at once: #2 only waits");
}

#[tokio::test]
async fn an_idle_cut_waits_until_everything_landed_is_promoted() {
    let w = World::new();
    // #2 keeps working all along: it is never silent.
    w.two_at_work().await;
    let main = w.remote_tip("web", "main");
    let landed = w.lands("web", 1, "a").await.commit;
    assert_eq!(w.remote_tip("web", "main"), main, "#2 still works: not idle");
    w.advance(29);
    assert!(w.tick().await.lines.is_empty(), "busy, within the bound less the hold bound");
    w.advance(2);
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "#2 still works: a cut waits for idle or its own bound");
    // #2 lands a change whose slow gate is red: idle, but not all promoted.
    w.lands("web", 2, "SLOWRED").await;
    w.tick().await;
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.0"]).is_err(), "no version is cut");
    // Its own bound passed, it is cut whatever is unpromoted.
    w.advance(24 * 60);
    w.tick().await;
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.0"]).is_ok(), "cut once the bound passed");
}

#[tokio::test]
async fn a_checkout_is_idle_whatever_another_checkout_s_workers_do() {
    let w = World::new();
    w.checkout("api").await;
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("api", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "w", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    // api's worker works on; web's lands, and web is idle.
    let web = w.worker_on("web", 1).await;
    w.done(&web, "w", "x").await;
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
}

#[tokio::test]
async fn holding_one_checkout_s_landings_holds_no_other_s() {
    let w = World::new();
    w.checkout("api").await;
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("api", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    w.app.hold("web", Some("repairs")).await.unwrap();
    let wk = worker(&w.app).await;
    w.done(&wk, "a", "x").await;
    assert_eq!(w.tick().await.landed[0]["landed"], "api#1");
    assert_eq!(w.app.hold("api", None).await.unwrap()["held"], json!(null));
}

/// The app reads a worker before it asks to land it or end it; the worker
/// may end in between (the daemon's tick, the watch). The command itself
/// refuses a worker that never started or has ended, so nothing is recorded
/// on it.
#[test]
fn landing_or_ending_a_worker_that_is_gone_is_refused_by_the_command_itself() {
    use factorium_domain::worker::{
        AskLanding, Because, EndWorker, GreenLanding, Land, Of, Red, RedLanding, Status, Worker,
    };
    use factum::{By, Command, Instant};
    let worker = factum::Uuid::now_v7();
    let ended = Worker { started: Some(Default::default()), status: Status::Ended, ..Default::default() };
    let (by, now) = (By::Principal("test".into()), Instant(0));
    let refused = |r: Result<_, factum::Rejection>| r.map(drop).unwrap_err().code;
    for (state, code) in [(&Worker::default(), "no_worker"), (&ended, "ended")] {
        assert_eq!(refused(AskLanding::decide(state, &Of { worker }, &by, now)), code);
        let land = Land { worker, commit: "c".into() };
        assert_eq!(refused(GreenLanding::decide(state, &land, &by, now)), code);
        let why = Because { worker, why: "x".into() };
        assert_eq!(refused(EndWorker::decide(state, &why, &by, now)), code);
    }
    // A live worker that has not asked to land takes no landing, green or red.
    let working = Worker { started: Some(Default::default()), ..Default::default() };
    assert_eq!(refused(GreenLanding::decide(&working, &Land { worker, commit: "c".into() }, &by, now)), "not_landing");
    let red = Red { worker, why: "x".into(), tip: None, moved: false, moved_max: 0 };
    assert_eq!(refused(RedLanding::decide(&working, &red, &by, now)), "not_landing");
}

#[tokio::test]
async fn a_gate_reading_tags_sees_origins_tags_in_the_line_s_clones_made_before_them() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    // Both gates describe the tip by its tags once `DESCRIBE` exists.
    let describe = "test ! -e DESCRIBE || git describe --tags";
    let toml = format!("[gate]\nfast = \"{describe}\"\nslow = \"{describe}\"\n");
    let clone = w.push_by_hand("web", "proving", "factorium.toml", &toml);
    // #1 lands and is promoted while origin has no tag: the clones are made.
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    w.lands("web", 1, "a").await;
    let promotion = w.data().clone_of(LineClone::Promotion, "web");
    assert!(promotion.join(".git").is_dir(), "the promotion clone is made");
    assert_eq!(w.git(&promotion, &["tag"]).unwrap(), "");
    // Origin gets a tag; #2's gates describe the tip by it.
    w.git(&clone, &["tag", "-a", "v0.0.9", "-m", "v0.0.9", "HEAD"]).unwrap();
    w.git(&clone, &["push", "-q", "origin", "v0.0.9"]).unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    let landed = w.lands("web", 2, "DESCRIBE").await.commit;
    assert_eq!(w.remote_tip("web", "main"), landed, "the slow gate is green on proving");
    assert_eq!(w.git(&promotion, &["tag"]).unwrap(), "v0.0.9", "the promotion clone has origin's tags");
}

#[tokio::test]
async fn a_line_s_clone_reused_has_the_checkout_s_remote_refs_as_they_are_now() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    let at = |repo: &std::path::Path, rev: &str| w.git(repo, &["rev-parse", rev]).ok();
    // A branch the checkout's origin has: #1's promotion makes the clone with it.
    w.git(&web, &["update-ref", "refs/remotes/origin/gone", "HEAD"]).unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    w.lands("web", 1, "a").await;
    let promotion = w.data().clone_of(LineClone::Promotion, "web");
    assert!(promotion.join(".git").is_dir(), "the promotion clone is made");
    let made = at(&promotion, "origin/HEAD").unwrap();
    assert!(at(&promotion, "refs/remotes/origin/gone").is_some(), "the clone has the checkout's branches");
    // #1's promotion moved the base in the checkout; the branch goes there.
    let base = at(&web, "origin/HEAD").unwrap();
    assert_ne!(base, made, "the base moved after the clone was made");
    w.git(&web, &["update-ref", "-d", "refs/remotes/origin/gone"]).unwrap();
    // #2's promotion reuses the clone.
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.lands("web", 2, "b").await;
    assert_eq!(at(&promotion, "origin/HEAD"), Some(base), "the reused clone's base is the checkout's new one");
    assert_eq!(at(&promotion, "refs/remotes/origin/gone"), None, "a branch the checkout lost is pruned");
}

/// A failed cut is owed again only once a commit other than the one it was cut from is promoted.
#[test]
fn a_failed_cut_waits_for_a_newer_promotion() {
    let mut l = Line::default();
    l.apply(LineEvent::Landed { number: 1, commit: "a".into(), at: 0 });
    l.apply(LineEvent::Promoted { tip: "a".into(), at: 0 });
    let (version, why, commit) = ("v0.1.0".to_string(), "x".to_string(), Some("a".to_string()));
    l.apply(LineEvent::CutFailed { version, why, commit });
    l.apply(LineEvent::Promoted { tip: "a".into(), at: 1 });
    assert!(l.cut_failed.is_some(), "the same commit holds no fix");
    l.apply(LineEvent::Landed { number: 2, commit: "b".into(), at: 2 });
    l.apply(LineEvent::Promoted { tip: "b".into(), at: 3 });
    assert!(l.cut_failed.is_none());
    let bounds = Bounds { promotion: 10, cut: 10, lead: 0 };
    let look = Look { now: 4, idle: true, asks: None, bounds, by_hand: false };
    assert!(l.due(Move::Cut, &look, || Ok::<_, ()>(String::new())) == Ok(Ok(Due::Cut)));
}
