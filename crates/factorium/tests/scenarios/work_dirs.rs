//! The line's work dirs: each job (a landing, a promotion, a cut, a run of
//! idle work) runs in an Orca worktree of its checkout of its own, named and
//! commented with what it is for, and removed once the job ends. A worker's
//! worktree is commented with its issue. A work dir whose job is gone is
//! removed at the next tick, or, holding commits found nowhere else, kept
//! for doctor to name.

use std::path::{Path, PathBuf};

use factorium_host::Orca;
use serde_json::Value;

use crate::fixture::{STUCK, World, until_file, worker};

/// How the comment of a work dir factorium made ends, before its maker's pid.
const MARK: &str = "factorium removes it when this ends (pid ";

/// The line's work dirs the stand-in Orca holds now: path, name and comment.
fn line_dirs(w: &World) -> Vec<(PathBuf, String, String)> {
    let made = w.host.orca.made.lock().unwrap();
    let line = made.iter().filter(|(_, (_, c))| c.contains(MARK) || c.contains("factorium kept it"));
    line.map(|(p, (n, c))| (p.clone(), n.clone(), c.clone())).collect()
}

/// `web`, its proving and its base declaring `toml`, `{long}` a command
/// that notes where it runs in `dir/where`, then holds until `dir/release`;
/// and a kit.
async fn declares(w: &World, dir: &Path, toml: &str) {
    let web = w.checkout("web").await;
    w.kit().await;
    let long = format!("pwd -P > {}/where; {}", dir.display(), w.holds(dir));
    let toml = toml.replace("{long}", &long);
    w.push_by_hand("web", "main", "factorium.toml", &toml);
    w.push_by_hand("web", "proving", "factorium.toml", &toml);
    w.git(&web, &["fetch", "-q", "origin"]).unwrap();
}

/// #1 of `web` done, its worker's change committed.
async fn one_done(w: &World) {
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick_alone().await;
    w.done(&worker(&w.app).await, "a", "x").await;
}

/// Once the job holding at `dir` started: the one work dir of the line,
/// named `name`, its comment saying `about`, and the job running in it.
async fn runs_in_one(w: &World, dir: &Path, name: &str, about: &str) {
    until_file(&dir.join("started")).await;
    let dirs = line_dirs(w);
    assert_eq!(dirs.len(), 1, "one work dir of the line: {dirs:?}");
    let (path, shown, comment) = &dirs[0];
    assert_eq!(shown, name);
    assert!(comment.contains(about) && comment.contains(MARK), "{comment}");
    let ran = std::fs::read_to_string(dir.join("where")).unwrap();
    assert_eq!(Path::new(ran.trim()), path.canonicalize().unwrap(), "the job runs in it");
}

/// The job holding at `dir` let end, and the tick that finishes it: no work
/// dir of the line is left. Answers that tick's lines and landings.
async fn ends_with_none(w: &World, dir: &Path) -> Vec<Value> {
    std::fs::write(dir.join("release"), "").unwrap();
    tokio::time::timeout(STUCK, w.app.beside_ended()).await.expect("its steps end");
    let t = w.tick_alone().await;
    assert_eq!(line_dirs(w), Vec::new(), "the work dir is removed once the job ends");
    t.lines.into_iter().chain(t.landed).chain(t.idle).collect()
}

#[tokio::test]
async fn a_landing_runs_in_a_work_dir_named_for_its_issue_removed_once_it_lands() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"{long}\"\n").await;
    one_done(&w).await;
    w.tick_alone().await;
    runs_in_one(&w, &dir, "landing web#1", "landing of web#1 (a)").await;
    let did = ends_with_none(&w, &dir).await;
    assert!(did.iter().any(|d| d["landed"] == "web#1"), "{did:?}");
}

#[tokio::test]
async fn a_promotion_runs_in_a_work_dir_named_for_its_tip_removed_once_it_promotes() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"true\"\nslow = \"{long}\"\n").await;
    one_done(&w).await;
    w.gated().await;
    let t = w.tick_alone().await;
    let landed = t.landed[0]["commit"].as_str().expect("#1 lands").to_string();
    let name = format!("promotion web {}", &landed[..7]);
    runs_in_one(&w, &dir, &name, &format!("promotion of web: its slow gate run on the proving tip {}", &landed[..7]))
        .await;
    let did = ends_with_none(&w, &dir).await;
    assert!(did.iter().any(|d| d["promoted"] == "web"), "{did:?}");
}

#[tokio::test]
async fn a_cut_runs_in_a_work_dir_named_for_its_version_removed_once_it_is_made() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[gate]\nfast = \"true\"\n\n[cut]\nsteps = [\"{long}\"]\n").await;
    one_done(&w).await;
    w.gated().await;
    w.tick_alone().await;
    runs_in_one(&w, &dir, "cut web v0.1.0", "cut of web v0.1.0").await;
    let did = ends_with_none(&w, &dir).await;
    assert!(did.iter().any(|d| d["cut"] == "web" && d["version"] == "v0.1.0"), "{did:?}");
}

#[tokio::test]
async fn idle_work_runs_in_a_work_dir_named_for_its_checkout_removed_once_it_ends() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &dir, "[idle]\nsteps = [\"{long}\"]\nevery = 3600\nlimit = 60\n").await;
    let t = w.tick_alone().await;
    assert!(t.idle.iter().any(|v| v["idle"] == "web" && v["started"] == true), "{:?}", t.idle);
    runs_in_one(&w, &dir, "idle web", "idle work of web").await;
    let did = ends_with_none(&w, &dir).await;
    assert!(did.iter().any(|d| d["idle"] == "web" && d["green"] == true), "{did:?}");
}

#[tokio::test]
async fn a_workers_worktree_is_commented_with_its_issue() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a title", "", false, &[]).await.unwrap();
    w.tick_alone().await;
    let path = crate::fixture::worktree(&worker(&w.app).await);
    let (name, comment) = w.host.orca.made.lock().unwrap().get(&path).cloned().expect("Orca made it");
    assert_eq!(name, "web-1-1");
    assert!(comment.contains("worker on web#1 (a title)") && comment.contains("lands or closes"), "{comment}");
}

#[tokio::test]
async fn a_work_dir_whose_job_is_gone_is_removed_at_the_next_tick_and_one_holding_commits_is_kept() {
    let w = World::new();
    let web = w.checkout("web").await;
    let head = w.git(&web, &["rev-parse", "HEAD"]).unwrap();
    let orca = Orca(&*w.host);
    let gone = format!("a landing. {MARK}999999).");
    let mine = format!("a promotion. {MARK}{}).", std::process::id());
    let left = orca.worktree(&web, "landing web#9", "left-9", &head, &gone).unwrap();
    let asked_by_none = orca.worktree(&web, "promotion web 1234567", "left-p", &head, &mine).unwrap();
    let holding = orca.worktree(&web, "cut web v9.9.9", "left-c", &head, &gone).unwrap();
    w.commit(&holding, "version", "9.9.9");
    w.git(&holding, &["checkout", "-q", "--detach"]).unwrap();
    w.git(&web, &["branch", "-D", "left-c"]).unwrap();
    let t = w.tick_alone().await;
    let removed: Vec<_> = t.work_dirs.iter().filter(|d| d["removed"] == true).map(|d| d["work_dir"].clone()).collect();
    assert_eq!(removed, ["landing web#9", "promotion web 1234567"], "{:?}", t.work_dirs);
    assert!(!left.exists() && !asked_by_none.exists());
    assert!(holding.exists(), "kept: it holds a commit found nowhere else");
    let dirs = line_dirs(&w);
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    assert!(dirs[0].2.ends_with("factorium kept it, as it holds commits found nowhere else."), "{dirs:?}");
    let finding = w.finding(&format!("work_dir:{}", holding.display())).await;
    assert!(finding.as_ref().is_some_and(|f| f.to_string().contains("cut web v9.9.9")), "{finding:?}");
    // The next tick leaves it be.
    assert_eq!(w.tick_alone().await.work_dirs, Vec::<Value>::new());
    assert!(holding.exists());
}
