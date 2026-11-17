//! The watch: its session, its look, episodes and rounds, its report, and
//! what it does with its commands.

use factorium_harness::Harness;
use serde_json::json;

use crate::harnesses::write_session;
use crate::{World, id, worker};

/// A checkout, a kit, a worker that escalated, and the watch's session up.
async fn escalated(w: &World) -> serde_json::Value {
    let wk = escalated_alone(w).await;
    watch_up(w, false);
    wk
}

/// A checkout, a kit, a worker that escalated; the watch's terminal opened.
async fn escalated_alone(w: &World) -> serde_json::Value {
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    wk
}

/// The watch's session writes its turn: open, or closed.
fn watch_up(w: &World, open: bool) {
    let home = w.dir.join("data").join("watch");
    let at = (factum::Clock::now(w.clock.as_ref()).0 / 1000) as u64;
    write_session(w, Harness::ClaudeCode, &home.to_string_lossy(), open, at);
}

#[tokio::test]
async fn no_round_is_typed_before_the_watch_s_session_is_up_and_idle() {
    let w = World::new();
    escalated_alone(&w).await;
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"], json!(null), "its session is not up yet");
    watch_up(&w, true);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"], json!(null), "its turn is open");
    assert!(w.terminal(&watch_terminal(&w)).sent.is_empty());
    watch_up(&w, false);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"]["n"], 1);
}

fn watch_terminal(w: &World) -> String {
    let t = w.host.orca.terminals.lock().unwrap();
    t.iter().find(|(_, t)| t.title == "factorium watch").map(|(h, _)| h.clone()).expect("the watch's terminal")
}

#[tokio::test]
async fn an_escalation_becomes_an_episode_and_one_round_sent_to_the_watch() {
    let w = World::new();
    let wk = escalated(&w).await;
    w.advance(5);
    let t = w.tick().await;
    assert_eq!(t.watch.as_ref().unwrap()["round"]["n"], 1);
    let term = w.terminal(&watch_terminal(&w));
    assert!(term.line.contains("BRIEF.md"), "the watch is handed its brief: {}", term.line);
    assert!(term.line.contains("FACTORIUM_WATCH"));
    let round = term.sent.last().unwrap();
    assert!(round.contains("round 1") && round.contains("which API version?"), "{round}");

    let next = w.app.watch_next().await.unwrap();
    assert!(next["episode"]["what"].as_str().unwrap().contains(wk["worker"].as_str().unwrap()));

    // The episode stands, but it already had its round.
    w.app.watch_report("Answered: v2.", None).await.unwrap();
    w.advance(5);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["round"], json!(null));
    assert_eq!(w.terminal(&watch_terminal(&w)).sent.len(), 1);

    // Sent back to work, the worker's episode closes.
    w.app.watch_restart(id(&wk), "Use v2.").await.unwrap();
    assert_eq!(worker(&w.app).await["status"], "working");
    assert!(w.terminal(wk["terminal"].as_str().unwrap()).sent.last().unwrap().contains("Use v2."));
    w.advance(5);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["episodes"], json!({}));
}

#[tokio::test]
async fn a_report_ends_only_a_running_round_and_its_owner_part_reaches_doctor_once() {
    let w = World::new();
    crate::refused(w.app.watch_report("nothing", None).await, "no_round");
    escalated(&w).await;
    w.advance(5);
    w.tick().await;
    crate::refused(w.app.watch_report(" ", None).await, "no_text");
    w.app.watch_report("The account ran out.", Some("buy more Codex credit")).await.unwrap();
    assert_eq!(w.app.doctor().await.unwrap()["human"], json!(["buy more Codex credit"]));
    assert_eq!(w.app.doctor().await.unwrap()["human"], json!([]), "seen once");
}

#[tokio::test]
async fn a_round_past_its_bound_ends_and_the_next_episode_gets_one() {
    let w = World::new();
    escalated(&w).await;
    w.advance(5);
    w.tick().await;
    w.app.issue_add("web", "u", "", false).await.unwrap();
    w.tick().await;
    let second =
        w.app.workers(false).await.unwrap().as_array().unwrap().iter().find(|x| x["number"] == 2).cloned().unwrap();
    w.app.worker_escalate(id(&second), "stuck too").await.unwrap();
    w.advance(5);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["round"]["n"], 1, "one round at a time");
    w.advance(61);
    w.tick().await;
    let next = w.app.watch_next().await.unwrap();
    assert_eq!(next["round"]["n"], 2);
    assert!(next["episode"]["what"].as_str().unwrap().contains("stuck too"));
}

#[tokio::test]
async fn restarting_a_worker_whose_session_is_gone_starts_its_issue_again() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.kill_terminal(wk["terminal"].as_str().unwrap());
    w.tick().await;
    let r = w.app.watch_restart(id(&wk), "go on").await.unwrap();
    assert_eq!(r["ended"], true);
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["issue"], "web#1");
    assert_ne!(again["worker"], wk["worker"]);
}

#[tokio::test]
async fn a_new_watch_session_gets_the_round_that_is_running() {
    let w = World::new();
    escalated(&w).await;
    w.advance(5);
    w.tick().await;
    let first = watch_terminal(&w);
    assert_eq!(w.terminal(&first).sent.len(), 1);
    w.kill_terminal(&first);
    w.advance(5);
    let t = w.tick().await.watch.unwrap();
    let second = t["watch"].as_str().unwrap().to_string();
    assert_ne!(second, first, "a new session");
    assert_eq!(t["round"]["n"], 1, "the same round");
    let sent = w.terminal(&second).sent;
    assert!(sent.len() == 1 && sent[0].contains("round 1"), "{sent:?}");
    w.advance(5);
    w.tick().await;
    assert_eq!(w.terminal(&second).sent.len(), 1, "once per session");
}

#[tokio::test]
async fn the_worker_and_the_watch_are_told_the_database_the_app_was_opened_on() {
    let w = World::on("other.db");
    let wk = escalated_alone(&w).await;
    let db = w.db.to_string_lossy().into_owned();
    let worker = w.terminal(wk["terminal"].as_str().unwrap());
    assert!(worker.line.contains(&format!("FACTORIUM_DB='{db}'")), "{}", worker.line);
    let watch = w.terminal(&watch_terminal(&w));
    assert!(watch.line.contains(&format!("FACTORIUM_DB='{db}'")), "{}", watch.line);
    let rules = std::fs::read_to_string(w.dir.join("data/watch/CLAUDE.md")).unwrap();
    assert!(rules.contains(&format!("The database is {db}.")), "{rules}");
}
