//! Gates first (SPEC.md §1.9): a gate, and a worker's `worker exclusive`
//! (whose times a gate compares), run at the priority factorium runs at;
//! other heavy work — a cut, idle work, `worker exclusive --background` —
//! runs below it, its whole process tree.

use serde_json::json;

use crate::{World, id, worker, worktree};

/// This process's nice value: what a command run at its priority has.
pub(crate) fn own_nice() -> i32 {
    let out = std::process::Command::new("ps").args(["-o", "ni=", "-p", &std::process::id().to_string()]).output();
    String::from_utf8(out.unwrap().stdout).unwrap().trim().parse().unwrap()
}

/// A shell test that a command's process, and one it starts, has a nice
/// value `op` (`-eq`, `-gt`) `n`.
pub(crate) fn nice_is(op: &str, n: i32) -> String {
    format!("test $(sh -c 'ps -o ni= -p $$') {op} {n}")
}

#[tokio::test]
async fn gates_run_at_factoriums_priority_and_a_cut_below_it() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    let n = own_nice();
    let (gate, below) = (nice_is("-eq", n), nice_is("-gt", n));
    let toml = format!("[gate]\nfast = {gate:?}\nslow = {gate:?}\n\n[cut]\nsteps = [{below:?}]\nformula = \"web\"\n");
    let clone = w.dir.join("edit");
    w.git(&w.dir, &["clone", "-q", &web.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
    w.git(&clone, &["remote", "set-url", "origin", &w.dir.join("remotes/web.git").to_string_lossy()]).unwrap();
    w.git(&clone, &["fetch", "-q"]).unwrap();
    w.git(&clone, &["checkout", "-q", "-B", "proving", "origin/proving"]).unwrap();
    w.commit(&clone, "factorium.toml", &toml);
    w.git(&clone, &["push", "-q", "origin", "proving"]).unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.commit(&worktree(&wk), "a", "x");
    assert_eq!(w.app.worker_gate(id(&wk)).await.unwrap()["green"], true, "a worker's fast gate");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1", "the fast gate on proving: {t:?}");
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "the slow gate: {:?}", t.lines);
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut ran");
    assert_eq!(cut["failed"], json!(null), "the cut runs below: {cut}");
}

/// factorium#110: at background priority a worker's `slimfit check`
/// measured 2.7x what the gate measured of the same tree, so the baselines
/// it recorded were not the gate's.
#[tokio::test]
async fn a_workers_heavy_command_runs_at_a_gates_priority() {
    let w = World::new();
    let dir = w.dir.clone();
    let ran = w.app.worker_exclusive(None, &dir, &nice_is("-eq", own_nice())).await.unwrap();
    assert_eq!(ran["ok"], true, "{ran}");
}

#[tokio::test]
async fn a_workers_background_command_runs_below_factoriums_priority() {
    let w = World::new();
    let dir = w.dir.clone();
    let ran = w.app.worker_exclusive_background(None, &dir, &nice_is("-gt", own_nice())).await.unwrap();
    assert_eq!(ran["ok"], true, "{ran}");
}
