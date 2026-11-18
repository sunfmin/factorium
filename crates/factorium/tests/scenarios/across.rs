//! An issue waits for an issue of another checkout (factorium#77): until it
//! lands, or, with `@cut`, until a cut of that checkout holds its landing.
//! No worker starts to find out, and no one readies it after.

use serde_json::json;

use factorium::Answer;
use factorium_domain::issue::Blocker;

use crate::{World, id, refused, worktree};

fn by(s: &str) -> Blocker {
    s.parse().unwrap()
}

/// The issue the tick started, if any.
fn started(t: &crate::Tick) -> Option<String> {
    t.started.as_ref().map(|s| s["issue"].as_str().unwrap().to_string())
}

/// The live worker on issue `number` of `web`.
async fn on(w: &World, number: u64) -> serde_json::Value {
    let all = w.app.workers(false).await.unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .find(|r| r["checkout"] == "web" && r["number"] == number)
        .cloned()
        .unwrap_or_else(|| panic!("{all}"))
}

/// Checkouts `web` and `factorium`, and a kit; answers factorium's path.
async fn two_checkouts(w: &World) -> std::path::PathBuf {
    w.checkout("web").await;
    let path = w.checkout("factorium").await;
    w.kit().await;
    path
}

#[tokio::test]
async fn an_issue_blocked_by_another_checkouts_starts_once_that_one_lands() {
    let w = World::new();
    two_checkouts(&w).await;
    w.app.issue_add("web", "dep", "", false, &[]).await.unwrap();
    let added = w.app.issue_add("factorium", "waits", "", false, &[by("web#1")]).await.unwrap();
    assert_eq!((added["blocked_by"].clone(), added["blocked_across"].clone()), (json!([]), json!(["web#1"])));

    assert_eq!(started(&w.tick().await).as_deref(), Some("web#1"));
    assert_eq!(started(&w.tick().await), None, "factorium#2 waits while web#1 is open");

    let wk = on(&w, 1).await;
    w.commit(&worktree(&wk), "a", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert!(!t.landed.is_empty(), "{t:?}");
    assert_eq!(started(&t).as_deref(), Some("factorium#1"), "web#1 landed: no one readies it");
}

#[tokio::test]
async fn an_issue_blocked_by_a_cut_starts_only_once_a_cut_holds_the_landing() {
    let w = World::new();
    two_checkouts(&w).await;
    w.app.issue_add("web", "dep", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "busy", "", false, &[]).await.unwrap();
    w.app.issue_add("factorium", "waits", "", false, &[]).await.unwrap();
    let shown = w.app.issue_block("factorium", 1, by("web#1@cut"), false).await.unwrap();
    assert_eq!(shown["blocked_across"], json!(["web#1@cut"]));
    assert_eq!(started(&w.tick().await).as_deref(), Some("web#1"));
    assert_eq!(started(&w.tick().await).as_deref(), Some("web#2"));

    // web#1 lands; web#2 keeps web busy, so nothing is promoted or cut.
    let wk = on(&w, 1).await;
    w.commit(&worktree(&wk), "a", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert!(!t.landed.is_empty(), "{t:?}");
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "{:?}", t.lines);
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], "closed");
    assert_eq!(started(&t), None, "landed is not cut");
    // What older code kept of each stream is not read: the issue and the
    // line fold their events again (their code() names this code).
    stale(&w);
    assert_eq!(w.run().issue_show("factorium", 1).await.unwrap()["blocked_across"], json!(["web#1@cut"]));
    w.run().promote("web").await.unwrap();
    assert_eq!(started(&w.tick().await), None, "promoted is not cut");

    let cut = w.run().cut("web").await.unwrap();
    assert_eq!(cut["installed"], "web", "{cut}");
    assert_eq!(started(&w.tick().await).as_deref(), Some("factorium#1"));
}

#[tokio::test]
async fn a_block_across_names_an_issue_that_is_there_and_a_cut_its_repo_declares() {
    let w = World::new();
    let path = two_checkouts(&w).await;
    w.app.issue_add("web", "one", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    w.app.issue_add("factorium", "waits", "", false, &[]).await.unwrap();
    let block = |n: u64, b: &str, undo: bool| w.app.issue_block("factorium", n, by(b), undo);
    let across = |a: Answer| a.unwrap()["blocked_across"].clone();

    refused(block(1, "web#9", false).await, "no_issue");
    refused(block(1, "nope#1", false).await, "no_checkout");
    refused(block(1, "factorium#1@cut", false).await, "blocks_itself");
    assert_eq!(across(block(1, "web#1", false).await), json!(["web#1"]));
    assert_eq!(across(block(1, "web#1", false).await), json!(["web#1"]));
    assert_eq!(across(block(1, "web#1@cut", false).await), json!(["web#1@cut"]), "it waits for the cut now");
    assert_eq!(across(block(1, "web#2", false).await), json!(["web#1@cut", "web#2"]));
    assert_eq!(across(block(1, "web#1", true).await), json!(["web#2"]));
    // Of its own checkout, named in full, is one of its own.
    let own = w.app.issue_add("web", "three", "", false, &[by("web#1")]).await.unwrap();
    assert_eq!((own["blocked_by"].clone(), own["blocked_across"].clone()), (json!([1]), json!([])));

    // A repo with no cut has none to wait for.
    w.commit(&path, "factorium.toml", "[gate]\nfast = \"true\"\n");
    w.app.issue_add("factorium", "two", "", false, &[]).await.unwrap();
    refused(w.app.issue_block("web", 1, by("factorium#2@cut"), false).await, "no_cut");
    for bad in ["web#x", "web#1@tag", "#3"] {
        assert!(bad.parse::<Blocker>().is_err(), "{bad}");
    }
    // The same block given twice is kept once.
    let twice = w.app.issue_add("factorium", "twice", "", false, &[by("web#1"), by("web#1")]).await.unwrap();
    assert_eq!(twice["blocked_across"], json!(["web#1"]));
}

/// Keeps for every stream a snapshot that older code wrote, of no use to this code.
fn stale(w: &World) {
    let sql = "INSERT OR REPLACE INTO snapshots (stream, version, code, state) \
               SELECT stream, MAX(version), 'older', 'null' FROM events GROUP BY stream;";
    let ran = std::process::Command::new("sqlite3").arg(&w.db).arg(sql).output().unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
}
