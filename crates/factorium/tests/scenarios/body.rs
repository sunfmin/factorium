//! An issue's body edited while a worker holds it reaches the worker: its
//! brief is rewritten to the body as it is now, and its session is told to
//! read it again (factorium#278).

use serde_json::json;

use crate::fixture::{CLAUDE, World, id};

#[tokio::test]
async fn a_body_edited_under_a_worker_rewrites_its_brief_and_tells_it() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    let brief = w.data().brief("web-1-1");
    w.app.issue_comment("web", 1, "noted").await.unwrap();
    let after_comment = w.terminal(handle).sent.len();
    let shown = w.app.issue_edit("web", 1, None, Some("## Do\n\nthe new way")).await.unwrap();
    assert_eq!(shown["delivery"], "submitted", "{shown}");

    let text = std::fs::read_to_string(&brief).unwrap();
    assert!(text.starts_with("# web#1: t\n\n## Do\n\nthe new way\n"), "{text}");
    assert!(text.contains("## How you finish"), "the rest is kept: {text}");
    let sent = &w.terminal(handle).sent[after_comment..];
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].starts_with("factorium (your issue's body changed): "), "{sent:?}");
    assert!(sent[0].contains(&brief.display().to_string()), "it names its brief: {sent:?}");

    // A title edited alone is rewritten in the brief, and told to no one.
    let shown = w.app.issue_edit("web", 1, Some("t2"), None).await.unwrap();
    assert_eq!(shown["delivery"], json!(null), "{shown}");
    let text = std::fs::read_to_string(&brief).unwrap();
    assert!(text.starts_with("# web#1: t2\n\n## Do\n\nthe new way\n"), "{text}");
    assert_eq!(w.terminal(handle).sent.len(), after_comment + 1);
}

#[tokio::test]
async fn a_body_edited_with_no_worker_is_only_recorded() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "t", "old", false, &[]).await.unwrap();
    let shown = w.app.issue_edit("web", 1, None, Some("new")).await.unwrap();
    assert_eq!(shown["body"], "new");
    assert_eq!(shown["delivery"], json!(null), "{shown}");
}

/// A worker waiting for the watch is not typed at: its brief holds the new
/// body when the watch sends it back to work.
#[tokio::test]
async fn a_body_edited_under_a_waiting_worker_rewrites_its_brief_only() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    w.app.worker_escalate(id(&wk), "which way?").await.unwrap();
    let before = w.terminal(handle).sent.len();
    let shown = w.app.issue_edit("web", 1, None, Some("the new way")).await.unwrap();
    assert_eq!(shown["delivery"], "brief", "{shown}");
    assert_eq!(w.terminal(handle).sent.len(), before, "a waiting worker is not typed at");
    let text = std::fs::read_to_string(w.data().brief("web-1-1")).unwrap();
    assert!(text.starts_with("# web#1: t\n\nthe new way\n"), "{text}");
}
