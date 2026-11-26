//! Gates first (docs/adr/0009-admission-and-the-heavy-queue.md): a gate, and a worker's `worker exclusive`
//! (whose times a gate compares), run at the priority factorium runs at;
//! other heavy work — a cut, idle work, `worker exclusive --background` —
//! runs below it, its whole process tree.

use serde_json::json;

use crate::fixture::{World, id, nice_is, own_nice, worker, worktree};

#[tokio::test]
async fn gates_run_at_factoriums_priority_and_a_cut_below_it() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    let n = own_nice();
    let (gate, below) = (nice_is("-eq", n), nice_is("-gt", n));
    let toml = format!("[gate]\nfast = {gate:?}\nslow = {gate:?}\n\n[cut]\nsteps = [{below:?}]\nformula = \"web\"\n");
    w.push_by_hand("web", "proving", "factorium.toml", &toml);
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.commit(&worktree(&wk), "a", "x");
    assert_eq!(w.app.worker_gate(id(&wk)).await.unwrap()["green"], true, "a worker's fast gate");
    w.app.worker_done(id(&wk), None).await.unwrap();
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
