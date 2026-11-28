//! `issue show --brief` and the rows of `issue list`: where an issue stands,
//! without its body, its steps or any comment but the last, each field named
//! and valued as the full answer names and values it.

use serde_json::{Value, json};

use factorium_domain::issue::{Blocker, Wait};

use crate::fixture::{CLAUDE, World};

fn wait(s: &str) -> Wait {
    Wait::new(s.parse::<Blocker>().unwrap(), "needs its change")
}

/// Each field `brief` shares with `full` has the full answer's value.
fn same_as_full(brief: &Value, full: &Value) {
    for (k, v) in brief.as_object().unwrap() {
        if let Some(f) = full.get(k).filter(|_| !["waits", "comments", "spent"].contains(&k.as_str())) {
            assert_eq!(v, f, "`{k}` as the full answer has it");
        }
    }
}

#[tokio::test]
async fn the_brief_answers_where_an_issue_stands_and_the_list_rows_answer_the_same() {
    let w = World::new();
    w.working(CLAUDE, "at work").await;
    w.app.issue_comment("web", 1, "first word").await.unwrap();
    let long = "x".repeat(400);
    w.app.issue_comment("web", 1, &long).await.unwrap();
    w.app.issue_add("web", "gone", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "waits", "its body", false, &[wait("web#1"), wait("web#2")]).await.unwrap();
    w.app.issue_close("web", 2, "dropped").await.unwrap();
    w.advance(3);

    let (brief, full) =
        (w.app.issue_show_brief("web", 1).await.unwrap(), w.app.issue_show_steps("web", 1).await.unwrap());
    for gone in ["body", "role", "claim", "decided"] {
        assert!(brief.get(gone).is_none(), "no `{gone}`: {brief}");
    }
    same_as_full(&brief, &full);
    assert_eq!(brief["worker"], full["worker"], "{brief}");
    assert_eq!(brief["doing"]["status"], "working", "its live worker's step: {brief}");
    let last = full["comments"].as_array().unwrap().last().unwrap().clone();
    let shown = &brief["comments"];
    assert_eq!(shown.as_array().unwrap().len(), 1, "the last comment alone: {brief}");
    assert_eq!((shown[0]["by"].clone(), shown[0]["at"].clone()), (last["by"].clone(), last["at"].clone()));
    assert_eq!((shown[0]["text"].as_str().unwrap(), &shown[0]["cut"]), (&long[..300], &json!(true)), "{brief}");
    assert_eq!(brief["spent"], json!({ "ms": full["spent"]["ms"] }), "spent.ms alone, no steps: {brief}");
    assert!(full["spent"]["attempts"][0]["steps"].is_object(), "the full answer has steps: {full}");

    let (three, full3) = (w.app.issue_show_brief("web", 3).await.unwrap(), w.app.issue_show("web", 3).await.unwrap());
    same_as_full(&three, &full3);
    let unmet: Vec<_> = full3["waits"].as_array().unwrap().iter().filter(|w| w["met"].is_null()).cloned().collect();
    assert_eq!((three["waits"].clone(), unmet.len()), (json!(unmet), 1), "only the unmet waits: {three}");
    assert_eq!(
        (three["worker"].clone(), three["doing"].clone(), three["comments"].clone()),
        (json!(null), json!(null), json!([]))
    );

    let list = w.app.issue_list(Some("web"), false).await.unwrap();
    for row in list.as_array().unwrap() {
        let b = w.app.issue_show_brief("web", row["number"].as_u64().unwrap()).await.unwrap();
        for (k, v) in b.as_object().unwrap() {
            assert_eq!(&row[k], v, "the list row has the brief's `{k}`: {row}");
        }
        assert!(row.get("ready_at").is_some() && row.get("body").is_none(), "{row}");
    }
}
