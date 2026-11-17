//! Each bound of the host's readings at its edge: a reading at the bound
//! still has room, one past it does not.

use factorium::host::{Proc, Reading};
use serde_json::json;

use crate::{GIB, World, worker};

fn cores() -> f64 {
    std::thread::available_parallelism().unwrap().get() as f64
}

async fn ready(w: &World) {
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false).await.unwrap();
}

/// Why a tick with `reading` (and the workers' processes `procs`) holds the
/// start, if it does.
async fn held(reading: Reading, procs: Vec<Proc>) -> Option<String> {
    let w = World::new();
    ready(&w).await;
    *w.host.reading.lock().unwrap() = reading;
    *w.host.procs.lock().unwrap() = procs;
    w.tick().await.held
}

const ROOM: Reading = Reading { cpu_busy: 10, memory_free: 60, disk_free: 500 * GIB, disk_total: 1000 * GIB };

#[tokio::test]
async fn each_admission_bound_has_room_at_its_value_and_none_past_it() {
    // The defaults: disk 20 GiB, cpu 75%, memory 15%, the workers 80%.
    assert_eq!(held(Reading { disk_free: 20 * GIB, ..ROOM }, vec![]).await, None);
    let disk = held(Reading { disk_free: 20 * GIB - 1, ..ROOM }, vec![]).await.unwrap();
    assert_eq!(disk, "disk: 19 GiB free, under the floor of 20 GiB");
    assert_eq!(held(Reading { cpu_busy: 75, ..ROOM }, vec![]).await, None);
    assert_eq!(held(Reading { cpu_busy: 76, ..ROOM }, vec![]).await.unwrap(), "cpu: 76% busy, over 75%");
    assert_eq!(held(Reading { memory_free: 15, ..ROOM }, vec![]).await, None);
    assert_eq!(held(Reading { memory_free: 14, ..ROOM }, vec![]).await.unwrap(), "memory: 14% free, under 15%");
    let workers =
        |share: f64| vec![Proc { pid: 9, cpu: share * cores(), worker: Some("x".into()), ..Default::default() }];
    assert_eq!(held(ROOM, workers(80.0)).await, None);
    assert_eq!(held(ROOM, workers(81.0)).await.unwrap(), "workers: 81% of the CPU, over their 80%");
}

/// A build output in a landing worktree, as a gate leaves it.
fn built(w: &World) -> std::path::PathBuf {
    let target = w.db.parent().unwrap().join("landing").join("web-1").join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("out"), "built").unwrap();
    target
}

#[tokio::test]
async fn the_line_s_build_outputs_are_reclaimed_only_when_the_disk_is_under_its_floor() {
    let w = World::new();
    let target = built(&w);
    w.tick().await;
    assert!(target.exists(), "plenty of disk");
    w.host.reading.lock().unwrap().disk_free = 20 * GIB;
    w.tick().await;
    assert!(target.exists(), "at the floor");
    w.host.reading.lock().unwrap().disk_free = 19 * GIB;
    w.tick().await;
    assert!(!target.exists(), "under the floor");
    let worktree = target.parent().unwrap();
    assert!(worktree.exists(), "only the build output goes");
}

/// The worker's status and why, after its processes held `procs` for longer
/// than `hot_for`.
async fn after_holding(procs: impl Fn(String) -> Vec<Proc>) -> (String, String) {
    let w = World::new();
    ready(&w).await;
    w.tick().await;
    let tag = worker(&w.app).await["worker"].as_str().unwrap().to_string();
    *w.host.procs.lock().unwrap() = procs(tag);
    w.tick().await;
    w.advance(3);
    w.tick().await;
    let wk = worker(&w.app).await;
    (wk["status"].as_str().unwrap().into(), wk["why"].as_str().unwrap_or_default().into())
}

fn proc(tag: &str, cpu: f64, rss_kib: u64) -> Proc {
    Proc { pid: 77, ppid: 1, cpu, rss_kib, command: "cargo build".into(), worker: Some(tag.into()) }
}

#[tokio::test]
async fn a_worker_is_hot_only_past_each_of_its_shares() {
    // The defaults: 50% of the CPU, 16 GiB, 400 processes.
    let working = ("working".to_string(), String::new());
    assert_eq!(after_holding(|t| vec![proc(&t, 50.0 * cores(), 10)]).await, working);
    let (status, why) = after_holding(|t| vec![proc(&t, 51.0 * cores(), 10)]).await;
    assert!(status == "waiting" && why.starts_with("hot: cpu 51% over its share of 50%"), "{why}");

    let gib16 = 16 << 20;
    assert_eq!(after_holding(|t| vec![proc(&t, 1.0, gib16)]).await, working);
    let (status, why) = after_holding(|t| vec![proc(&t, 1.0, 2 * gib16)]).await;
    assert!(status == "waiting" && why.starts_with("hot: memory 32 GiB over 16 GiB"), "{why}");

    let many = |n: u64| move |t: String| (0..n).map(|i| Proc { pid: 1000 + i as u32, ..proc(&t, 0.0, 1) }).collect();
    assert_eq!(after_holding(many(400)).await, working);
    let (status, why) = after_holding(many(402)).await;
    assert!(status == "waiting" && why.starts_with("hot: 402 processes, over 400"), "{why}");
}

#[tokio::test]
async fn each_setting_says_what_it_bounds() {
    let w = World::new();
    let all = w.app.settings_set(None, None).await.unwrap();
    let disk = all.as_array().unwrap().iter().find(|s| s["name"] == "disk_floor_gib").unwrap().clone();
    assert_eq!(disk["what"], json!("nor while less disk than this is free; worktrees' target/ are reclaimed"));
}
