//! A waiting worker the watch parks behind other issues (it blocks the
//! worker's issue on them) goes back to work by itself on the first tick that
//! finds all its issue waits for met (factorium#113): told what was met and
//! how, as a comment on its issue, with no round run for it.

use serde_json::{Value, json};

use factorium_domain::issue::Blocker;

use crate::{CLAUDE, World, id, worktree};

/// The live worker on `checkout`#`number`.
async fn on(w: &World, checkout: &str, number: u64) -> Value {
    let all = w.app.workers(false).await.unwrap();
    let found = all.as_array().unwrap().iter().find(|r| r["checkout"] == checkout && r["number"] == number).cloned();
    found.unwrap_or_else(|| panic!("{all}"))
}

/// The worker on `checkout`#`number` commits and lands: answers the tick.
async fn land(w: &World, checkout: &str, number: u64) -> crate::Tick {
    let wk = on(w, checkout, number).await;
    w.commit(&worktree(&wk), &format!("{checkout}{number}"), "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert!(!t.landed.is_empty(), "{t:?}");
    t
}

/// How the worker stands, as `workers` lists it.
async fn status(w: &World, worker: &Value) -> Value {
    on(w, worker["checkout"].as_str().unwrap(), worker["number"].as_u64().unwrap()).await["status"].clone()
}

/// What doctor lists as parked, and whether `worker` is a finding.
async fn doctor(w: &World, worker: &Value) -> (Value, bool) {
    let d = w.run().doctor().await.unwrap();
    let key = format!("worker:{}", worker["worker"].as_str().unwrap());
    let found = d["findings"].as_array().unwrap().iter().any(|f| f["key"] == key);
    (d["parked"].clone(), found)
}

/// factorium's comments on an issue.
async fn said(w: &World, checkout: &str, number: u64) -> Vec<String> {
    let shown = w.app.issue_show(checkout, number).await.unwrap();
    let comments = shown["comments"].as_array().unwrap().iter();
    comments.filter(|c| c["by"] == "factorium").map(|c| c["text"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn a_worker_parked_behind_issues_of_its_checkout_goes_back_to_work_once_they_land() {
    let w = World::new();
    let wk = w.working(CLAUDE, "parked").await;
    w.watch_session(false);
    for title in ["two", "three"] {
        w.app.issue_add("web", title, "", false, &[]).await.unwrap();
        w.tick().await;
    }
    w.app.worker_escalate(id(&wk), "the fast gate is red: proving is at its bound").await.unwrap();
    w.app.issue_block("web", 1, 2, false).await.unwrap();
    w.app.issue_block("web", 1, 3, false).await.unwrap();
    let (parked, found) = doctor(&w, &wk).await;
    assert!(!found, "the watch decided already: no round for it");
    assert_eq!(parked[0]["waits_for"], json!(["#2", "#3"]), "{parked}");
    assert!(parked[0]["why"].as_str().unwrap().contains("at its bound"), "{parked}");

    let t = land(&w, "web", 2).await;
    assert!(t.resumed.is_empty(), "#3 is open: {t:?}");
    assert_eq!(status(&w, &wk).await, "waiting");
    assert_eq!(doctor(&w, &wk).await.0[0]["waits_for"], json!(["#3"]));

    let t = land(&w, "web", 3).await;
    assert_eq!(t.resumed.len(), 1, "{t:?}");
    assert_eq!(status(&w, &wk).await, "working");
    let told = said(&w, "web", 1).await;
    let [told] = told.as_slice() else { panic!("one comment: {told:?}") };
    assert!(told.contains("#2 (landed as ") && told.contains("#3 (landed as "), "{told}");
    assert!(told.contains("git merge origin/proving"), "{told}");
    let sent = w.terminal(wk["terminal"].as_str().unwrap()).sent;
    assert!(sent.last().unwrap().starts_with(&format!("factorium: {told}")), "{sent:?}");
    assert_eq!(doctor(&w, &wk).await.0, json!([]));

    // No round ran for it; its episode is no more.
    w.advance(5);
    w.tick().await;
    let next = w.app.watch_next().await.unwrap();
    assert_eq!((next["round"].clone(), next["episodes"].clone()), (json!(null), json!({})), "{next}");
}

#[tokio::test]
async fn a_worker_parked_behind_another_checkouts_issue_and_cut_goes_back_once_both_are_met() {
    let w = World::new();
    w.checkout("web").await;
    w.checkout("factorium").await;
    w.kit().await;
    for title in ["dep", "cut dep", "busy"] {
        w.app.issue_add("web", title, "", false, &[]).await.unwrap();
    }
    w.app.issue_add("factorium", "waits", "", false, &[]).await.unwrap();
    for _ in 0..4 {
        w.tick().await;
    }
    let wk = on(&w, "factorium", 1).await;
    w.app.worker_escalate(id(&wk), "needs web").await.unwrap();
    for b in ["web#1", "web#2@cut"] {
        w.app.issue_block("factorium", 1, b.parse::<Blocker>().unwrap(), false).await.unwrap();
    }
    assert_eq!(doctor(&w, &wk).await.0[0]["waits_for"], json!(["web#1", "web#2@cut"]));

    assert!(land(&w, "web", 1).await.resumed.is_empty(), "web#2 is open");
    // web#3 keeps web busy: web#2 lands, and is not cut.
    assert!(land(&w, "web", 2).await.resumed.is_empty(), "landed is not cut");
    assert_eq!(status(&w, &wk).await, "waiting");
    assert_eq!(
        doctor(&w, &wk).await,
        (
            json!([{ "worker": wk["worker"], "issue": "factorium#1", "why": "escalated: needs web", "waits_for": ["web#2@cut"] }]),
            false
        )
    );

    w.run().promote("web").await.unwrap();
    assert!(w.tick().await.resumed.is_empty(), "promoted is not cut");
    w.run().cut("web").await.unwrap();
    assert_eq!(w.tick().await.resumed.len(), 1);
    assert_eq!(status(&w, &wk).await, "working");
    let told = said(&w, "factorium", 1).await.join(" / ");
    assert!(told.contains("web#1 (landed as ") && told.contains(", in a cut)"), "{told}");
}

#[tokio::test]
async fn a_waiting_worker_with_nothing_unmet_to_wait_for_stays_the_watch_s() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    w.app.worker_escalate(id(&wk), "stuck").await.unwrap();
    // Not parked: it waits for the watch, whatever closes.
    w.app.issue_add("web", "three", "", false, &[]).await.unwrap();
    w.app.issue_close("web", 3, "not needed").await.unwrap();
    w.tick().await;
    assert_eq!(status(&w, &wk).await, "waiting");
    assert_eq!(doctor(&w, &wk).await, (json!([]), true));

    // Parked, then no longer blocked: back to the watch, not to work.
    w.app.issue_block("web", 1, 2, false).await.unwrap();
    assert!(!doctor(&w, &wk).await.1, "parked");
    w.app.issue_block("web", 1, 2, true).await.unwrap();
    assert!(w.tick().await.resumed.is_empty());
    assert_eq!(status(&w, &wk).await, "waiting");
    assert_eq!(doctor(&w, &wk).await, (json!([]), true));
    assert!(said(&w, "web", 1).await.is_empty());
}
