//! A waiting worker the watch parks behind other issues (it blocks the
//! worker's issue on them) goes back to work by itself on the first tick that
//! finds all its issue waits for met (factorium#113): told what was met and
//! how, as a comment on its issue, with no round run for it.

use serde_json::{Value, json};

use factorium_domain::issue::Blocker;

use crate::fixture::{CLAUDE, World, finding_keys, id};

/// How the worker stands, as `workers` lists it.
async fn status(w: &World, worker: &Value) -> Value {
    w.worker_on(worker["checkout"].as_str().unwrap(), worker["number"].as_u64().unwrap()).await["status"].clone()
}

/// What doctor lists as parked, and whether `worker` is a finding.
async fn doctor(w: &World, worker: &Value) -> (Value, bool) {
    let d = w.run().doctor().await.unwrap();
    let key = format!("worker:{}", worker["worker"].as_str().unwrap());
    let found = finding_keys(&d).contains(&key);
    (d["parked"].clone(), found)
}

/// factorium's comments on an issue.
async fn said(w: &World, checkout: &str, number: u64) -> Vec<String> {
    let comments = w.comments(checkout, number).await;
    comments.iter().filter(|c| c["by"] == "factorium").map(|c| c["text"].as_str().unwrap().to_string()).collect()
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
    w.app.issue_block("web", 1, 2, Some("needs #2's parser"), false).await.unwrap();
    w.app.issue_block("web", 1, 3, Some("needs #3's lexer"), false).await.unwrap();
    let (parked, found) = doctor(&w, &wk).await;
    assert!(!found, "the watch decided already: no round for it");
    let waits = json!([{ "on": "#2", "why": "needs #2's parser" }, { "on": "#3", "why": "needs #3's lexer" }]);
    assert_eq!(parked[0]["waits_for"], waits, "{parked}");
    assert!(parked[0]["why"].as_str().unwrap().contains("at its bound"), "{parked}");

    let t = w.lands("web", 2, "web2").await.tick;
    assert!(t.resumed.is_empty(), "#3 is open: {t:?}");
    assert_eq!(status(&w, &wk).await, "waiting");
    assert_eq!(doctor(&w, &wk).await.0[0]["waits_for"], json!([{ "on": "#3", "why": "needs #3's lexer" }]));

    let t = w.lands("web", 3, "web3").await.tick;
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
    let wk = w.worker_on("factorium", 1).await;
    w.app.worker_escalate(id(&wk), "needs web").await.unwrap();
    for b in ["web#1", "web#2@cut"] {
        w.app
            .issue_block("factorium", 1, b.parse::<Blocker>().unwrap(), Some("needs its change"), false)
            .await
            .unwrap();
    }
    let why = "needs its change";
    let waits = json!([{ "on": "web#1", "why": why }, { "on": "web#2@cut", "why": why }]);
    assert_eq!(doctor(&w, &wk).await.0[0]["waits_for"], waits);

    assert!(w.lands("web", 1, "web1").await.tick.resumed.is_empty(), "web#2 is open");
    // web#3 keeps web busy: web#2 lands, and is not cut.
    assert!(w.lands("web", 2, "web2").await.tick.resumed.is_empty(), "landed is not cut");
    assert_eq!(status(&w, &wk).await, "waiting");
    assert_eq!(
        doctor(&w, &wk).await,
        (
            json!([{ "worker": wk["worker"], "issue": "factorium#1", "why": "escalated: needs web", "waits_for": [{ "on": "web#2@cut", "why": why }] }]),
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
    w.app.issue_block("web", 1, 2, Some("needs its change"), false).await.unwrap();
    assert!(!doctor(&w, &wk).await.1, "parked");
    w.app.issue_block("web", 1, 2, None, true).await.unwrap();
    assert!(w.tick().await.resumed.is_empty());
    assert_eq!(status(&w, &wk).await, "waiting");
    assert_eq!(doctor(&w, &wk).await, (json!([]), true));
    assert!(said(&w, "web", 1).await.is_empty());
}

#[tokio::test]
async fn a_worker_blocked_while_it_works_then_in_a_declared_wait_goes_back_once_the_blocker_lands() {
    let w = World::new();
    let wk = w.working(CLAUDE, "blocked").await;
    w.app.issue_add("web", "dep", "", false, &[]).await.unwrap();
    w.tick().await;
    // Blocked while it works (factorium#256): nothing to park yet, it works on.
    w.app.issue_block("web", 1, 2, Some("needs its change"), false).await.unwrap();
    assert_eq!(status(&w, &wk).await, "working");
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    w.claude_session(&cwd, false, (w.app_now() / 1000) as u64);
    w.app.worker_waiting(id(&wk), "a rerun after the promotion gate", 45 * 60_000).await.unwrap();
    w.advance(5);
    assert!(w.tick().await.resumed.is_empty(), "#2 is open");
    assert_eq!(w.doctor().await["waiting"][0]["why"], "a rerun after the promotion gate");

    // #2 lands well within the 45 min it declared: the next tick sends it back.
    let t = w.lands("web", 2, "web2").await.tick;
    assert_eq!(t.resumed.len(), 1, "{t:?}");
    assert_eq!(t.resumed[0]["resumed"], true, "{t:?}");
    assert_eq!(status(&w, &wk).await, "working");
    let told = said(&w, "web", 1).await;
    let told = told.last().unwrap();
    assert!(told.contains("#2 (landed as ") && told.contains("git merge origin/proving"), "{told}");
    let sent = w.terminal(wk["terminal"].as_str().unwrap()).sent;
    assert!(sent.iter().any(|s| s.starts_with(&format!("factorium: {told}"))), "{sent:?}");
    // Back to work, its declared wait is over, and it is sent back once only.
    assert_eq!(w.doctor().await["waiting"], json!([]));
    w.advance(1);
    assert!(w.tick().await.resumed.is_empty());
}

#[tokio::test]
async fn a_worker_blocked_while_it_works_goes_back_when_it_waits_with_the_blocker_already_met() {
    let w = World::new();
    let wk = w.working(CLAUDE, "blocked").await;
    w.app.issue_add("web", "dep", "", false, &[]).await.unwrap();
    w.tick().await;
    w.app.issue_block("web", 1, 2, Some("needs its change"), false).await.unwrap();
    // #2 lands while it works: it is not interrupted.
    assert!(w.lands("web", 2, "web2").await.tick.resumed.is_empty(), "it works: nothing to send it back to");
    assert_eq!(status(&w, &wk).await, "working");
    // It then goes idle, here by escalating: the next tick sends it back.
    w.app.worker_escalate(id(&wk), "waits for #2").await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.resumed.len(), 1, "{t:?}");
    assert_eq!(status(&w, &wk).await, "working");
    assert!(said(&w, "web", 1).await.last().unwrap().contains("#2 (landed as "));
}

/// Ticks every 10 min for `mins` minutes.
async fn idle_for(w: &World, mins: i64) {
    for _ in 0..mins / 10 {
        w.advance(10);
        w.tick().await;
    }
}

#[tokio::test]
async fn a_blocked_idle_worker_is_never_silent_and_is_counted_again_once_sent_back() {
    let w = World::new();
    let wk = w.working(CLAUDE, "blocked").await;
    w.watch_session(false);
    w.app.issue_add("web", "dep", "", false, &[]).await.unwrap();
    w.tick().await;
    w.app.issue_block("web", 1, 2, Some("needs its change"), false).await.unwrap();
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    w.claude_session(&cwd, false, (w.app_now() / 1000) as u64);
    w.app.worker_waiting(id(&wk), "blocked on #2", 45 * 60_000).await.unwrap();
    // Past its declared wait and the two hours it is held to, #2 still open
    // (factorium#283): it is parked, not silent, and no round runs for it.
    idle_for(&w, 150).await;
    assert_eq!(status(&w, &wk).await, "working");
    assert!(!doctor(&w, &wk).await.1, "no finding for it");

    // #2 is met: it is sent back, and its silence counts from then.
    w.app.issue_close("web", 2, "not needed").await.unwrap();
    assert_eq!(w.tick().await.resumed.len(), 1);
    idle_for(&w, 30).await;
    assert_eq!(status(&w, &wk).await, "working", "at the bound, not past it");
    w.advance(1);
    w.tick().await;
    let silent = w.worker_on("web", 1).await;
    assert!(silent["why"].as_str().unwrap().starts_with("silent for 31 min"), "{silent}");
}

#[tokio::test]
async fn a_worker_whose_issue_waits_for_a_time_is_not_silent_before_it() {
    let w = World::new();
    let wk = w.working(CLAUDE, "later").await;
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    w.claude_session(&cwd, false, (w.app_now() / 1000) as u64);
    let at = w.app_now() + 150 * 60_000;
    w.app.issue_block("web", 1, Blocker::NotBefore(at), None, false).await.unwrap();
    idle_for(&w, 140).await;
    assert_eq!(status(&w, &wk).await, "working");
    assert!(!doctor(&w, &wk).await.1, "no finding for it");
}

#[tokio::test]
async fn a_worker_parked_at_its_prompt_with_no_declared_wait_goes_back_once_the_blocker_lands() {
    let w = World::new();
    let wk = w.working(CLAUDE, "blocked").await;
    w.watch_session(false);
    w.app.issue_add("web", "dep", "", false, &[]).await.unwrap();
    w.tick().await;
    // Its turn ended with no wait declared and no done: idle at its prompt.
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    w.claude_session(&cwd, false, (w.app_now() / 1000) as u64);
    w.app.issue_block("web", 1, 2, Some("needs its change"), false).await.unwrap();
    idle_for(&w, 20).await;
    assert_eq!(status(&w, &wk).await, "working");
    assert!(!doctor(&w, &wk).await.1, "parked, not silent");

    let t = w.lands("web", 2, "web2").await.tick;
    assert_eq!(t.resumed.len(), 1, "{t:?}");
    assert_eq!(t.resumed[0]["resumed"], true, "{t:?}");
    assert!(t.seen.is_empty(), "no silence first: {t:?}");
    let told = said(&w, "web", 1).await;
    assert!(told.last().unwrap().contains("#2 (landed as "), "{told:?}");
    assert_eq!(doctor(&w, &wk).await, (json!([]), false));
}
