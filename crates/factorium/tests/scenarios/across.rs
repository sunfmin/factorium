//! An issue waits for an issue of another checkout (factorium#77): until it
//! lands, or, with `@cut`, until a cut of that checkout holds its landing.
//! No worker starts to find out, and no one readies it after.

use serde_json::json;

use factorium::Answer;
use factorium_domain::issue::{Blocker, Wait};

use crate::fixture::{World, refused};

fn by(s: &str) -> Blocker {
    s.parse().unwrap()
}

fn wait(s: &str) -> Wait {
    Wait::new(by(s), "needs its change")
}

/// The issue the tick started, if any.
fn started(t: &factorium::Tick) -> Option<String> {
    t.started.as_ref().map(|s| s["issue"].as_str().unwrap().to_string())
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
    let added = w.app.issue_add("factorium", "waits", "", false, &[wait("web#1")]).await.unwrap();
    assert_eq!((added["blocked_by"].clone(), added["blocked_across"].clone()), (json!([]), json!(["web#1"])));

    assert_eq!(started(&w.tick().await).as_deref(), Some("web#1"));
    assert_eq!(started(&w.tick().await), None, "factorium#2 waits while web#1 is open");

    let t = w.lands("web", 1, "a").await.tick;
    assert_eq!(started(&t).as_deref(), Some("factorium#1"), "web#1 landed: no one readies it");
}

#[tokio::test]
async fn an_issue_blocked_by_a_cut_starts_only_once_a_cut_holds_the_landing() {
    let w = World::new();
    two_checkouts(&w).await;
    w.app.issue_add("web", "dep", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "busy", "", false, &[]).await.unwrap();
    w.app.issue_add("factorium", "waits", "", false, &[]).await.unwrap();
    let shown = w.app.issue_block("factorium", 1, by("web#1@cut"), Some("needs its change"), false).await.unwrap();
    assert_eq!(shown["blocked_across"], json!(["web#1@cut"]));
    assert_eq!(started(&w.tick().await).as_deref(), Some("web#1"));
    assert_eq!(started(&w.tick().await).as_deref(), Some("web#2"));

    // web#1 lands; web#2 keeps web busy, so nothing is promoted or cut.
    let t = w.lands("web", 1, "a").await.tick;
    assert!(!t.lines.iter().any(|l| l["cut"] == "web"), "{:?}", t.lines);
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], "closed");
    assert_eq!(started(&t), None, "landed is not cut");
    // What older code kept of each stream is not read: the issue and the
    // line fold their events again (their code() names this code).
    w.snapshots_older_code_left();
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
    let block = |n: u64, b: &str, undo: bool| w.app.issue_block("factorium", n, by(b), Some("needs its change"), undo);
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
    let own = w.app.issue_add("web", "three", "", false, &[wait("web#1")]).await.unwrap();
    assert_eq!((own["blocked_by"].clone(), own["blocked_across"].clone()), (json!([1]), json!([])));
    // An issue of its own checkout is one it waits for, for its cut or not.
    let waits = |a: Answer| a.map(|s| (s["blocked_by"].clone(), s["blocked_across"].clone())).unwrap();
    w.app.issue_add("factorium", "own", "", false, &[]).await.unwrap();
    let own_cut = waits(block(2, "factorium#1@cut", false).await);
    assert_eq!(own_cut, (json!([]), json!(["factorium#1@cut"])));
    assert_eq!(waits(block(2, "1", false).await), (json!([1]), json!([])), "it waits for #1 closed now");
    for b in ["web#1", "web#2", "web#1"] {
        block(2, b, false).await.unwrap();
    }
    assert_eq!(waits(block(2, "1", false).await), (json!([1]), json!(["web#1", "web#2"])), "each once, in order");
    assert_eq!(waits(block(2, "factorium#1", true).await), (json!([]), json!(["web#1", "web#2"])));
    block(2, "1", false).await.unwrap();
    assert_eq!(waits(block(2, "web#1", true).await), (json!([1]), json!(["web#2"])));

    // A repo with no cut has none to wait for.
    w.commit(&path, "factorium.toml", "[gate]\nfast = \"true\"\n");
    w.app.issue_add("factorium", "two", "", false, &[]).await.unwrap();
    refused(w.app.issue_block("web", 1, by("factorium#2@cut"), Some("needs its change"), false).await, "no_cut");
    for bad in ["web#x", "web#1@tag", "#3"] {
        assert!(bad.parse::<Blocker>().is_err(), "{bad}");
    }
    // The same block given twice is kept once.
    let twice = w.app.issue_add("factorium", "twice", "", false, &[wait("web#1"), wait("web#1")]).await.unwrap();
    assert_eq!(twice["blocked_across"], json!(["web#1"]));
}

/// A worker files the follow-up its issue asks for (factorium#169): on
/// another checkout, waiting from the start for its own issue's cut. Its
/// issue keeps "filed …", the filed one names the issue and worker.
#[tokio::test]
async fn a_worker_files_a_follow_up_on_another_checkout_that_waits_for_a_cut() {
    let w = World::new();
    two_checkouts(&w).await;
    w.app.issue_add("web", "dep", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "busy", "", false, &[]).await.unwrap();
    assert_eq!(started(&w.tick().await).as_deref(), Some("web#1"));
    assert_eq!(started(&w.tick().await).as_deref(), Some("web#2"));
    let worker = crate::fixture::id(&w.worker_on("web", 1).await);
    let filed =
        w.app.worker_file(worker, "factorium", "follow-up", "what web#1 left", &[wait("web#1@cut")], false).await;
    let filed = filed.unwrap();
    assert_eq!((filed["checkout"].clone(), filed["number"].clone()), (json!("factorium"), json!(1)));
    assert_eq!(
        (filed["body"].clone(), filed["blocked_across"].clone()),
        (json!("what web#1 left"), json!(["web#1@cut"]))
    );
    let by_worker = json!(format!("worker {worker}"));
    let said = |c: &[serde_json::Value]| c.iter().map(|c| (c["text"].clone(), c["by"].clone())).collect::<Vec<_>>();
    let from = json!(format!("filed by worker {worker}, working web#1"));
    assert_eq!(said(&w.comments("factorium", 1).await), [(from, by_worker.clone())]);
    assert_eq!(said(&w.comments("web", 1).await), [(json!("filed factorium#1"), by_worker)]);
    refused(w.app.worker_file(worker, "factorium", " ", "", &[], false).await, "no_title");
    refused(w.app.worker_file(worker, "nowhere", "x", "", &[], false).await, "no_checkout");

    assert_eq!(started(&w.tick().await), None, "it waits for web#1's cut");
    let t = w.lands("web", 1, "a").await.tick;
    assert_eq!(started(&t), None, "landed is not cut");
    w.run().promote("web").await.unwrap();
    assert_eq!(started(&w.tick().await), None, "promoted is not cut");
    assert_eq!(w.run().cut("web").await.unwrap()["installed"], "web");
    assert_eq!(started(&w.tick().await).as_deref(), Some("factorium#1"));
}

/// A follow-up a worker files on its own checkout waits for the worker's
/// issue to land, whose change it usually builds on (factorium#184);
/// `independent` is ready at once.
#[tokio::test]
async fn a_worker_files_a_follow_up_on_its_own_checkout_that_waits_for_its_issue() {
    let w = World::new();
    two_checkouts(&w).await;
    w.app.issue_add("web", "change", "", false, &[]).await.unwrap();
    assert_eq!(started(&w.tick().await).as_deref(), Some("web#1"));
    let worker = crate::fixture::id(&w.worker_on("web", 1).await);
    let filed = w.app.worker_file(worker, "web", "tests", "for web#1", &[], false).await.unwrap();
    assert_eq!((filed["number"].clone(), filed["blocked_by"].clone()), (json!(2), json!([1])));
    assert_eq!(started(&w.tick().await), None, "web#2 waits while web#1 is open");

    let alone = w.app.worker_file(worker, "web", "aside", "", &[], true).await.unwrap();
    assert_eq!((alone["number"].clone(), alone["blocked_by"].clone()), (json!(3), json!([])));
    assert_eq!(started(&w.tick().await).as_deref(), Some("web#3"), "independent is ready at once");

    let t = w.lands("web", 1, "a").await.tick;
    assert_eq!(started(&t).as_deref(), Some("web#2"));
}
