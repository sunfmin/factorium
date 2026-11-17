//! The checkout list and the tracker.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::{World, refused};

fn numbers(list: &Value) -> Vec<(String, u64)> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|i| (i["checkout"].as_str().unwrap().to_string(), i["number"].as_u64().unwrap()))
        .collect()
}

#[tokio::test]
async fn a_checkout_is_listed_with_its_branches_and_real_path_and_gets_its_proving_branch() {
    let w = World::new();
    let web = w.repo("web");
    w.app.checkout_add("web", &web.join("..").join("web"), "main", "proving").await.unwrap();
    let list = w.app.checkout_list().await.unwrap();
    assert_eq!(
        list,
        json!([{ "name": "web", "path": std::fs::canonicalize(&web).unwrap(), "base": "main", "proving": "proving" }])
    );
    assert_eq!(w.remote_tip("web", "proving"), w.remote_tip("web", "main"));
}

#[tokio::test]
async fn a_checkout_is_refused_twice_without_a_path_an_origin_or_a_good_name_or_with_one_branch() {
    let w = World::new();
    w.checkout("web").await;
    refused(w.app.checkout_add("web", &w.repo("other"), "main", "proving").await, "listed");
    refused(w.app.checkout_add("gone", &w.dir.join("nowhere"), "main", "proving").await, "no_path");
    std::fs::create_dir_all(w.dir.join("plain")).unwrap();
    refused(w.app.checkout_add("plain", &w.dir.join("plain"), "main", "proving").await, "no_base");
    refused(w.app.checkout_add("a b", &w.repo("ab"), "main", "proving").await, "invalid_name");
    refused(w.app.checkout_add("one", &w.repo("one"), "main", "main").await, "proving_is_base");
    assert_eq!(w.app.checkout_list().await.unwrap().as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_removed_checkout_leaves_the_list_takes_no_issues_and_can_come_back() {
    let w = World::new();
    w.checkout("web").await;
    w.app.checkout_remove("web").await.unwrap();
    assert_eq!(w.app.checkout_list().await.unwrap(), json!([]));
    refused(w.app.checkout_remove("web").await, "not_listed");
    refused(w.app.issue_add("web", "t", "", false, &[]).await, "no_checkout");
    let again = w.repo("web2");
    w.app.checkout_add("web", &again, "main", "proving").await.unwrap();
    assert_eq!(w.app.checkout_list().await.unwrap()[0]["path"], json!(std::fs::canonicalize(again).unwrap()));
}

#[tokio::test]
async fn issues_are_numbered_per_checkout_from_one() {
    let w = World::new();
    w.checkout("web").await;
    w.checkout("api").await;
    let first = w.app.issue_add("web", "  Fix the login  ", "body", false, &[]).await.unwrap();
    assert_eq!(
        (first["number"].clone(), first["title"].clone(), first["status"].clone()),
        (json!(1), json!("Fix the login"), json!("ready"))
    );
    w.app.issue_add("web", "second", "", false, &[]).await.unwrap();
    w.app.issue_add("api", "first of api", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "third", "", true, &[]).await.unwrap();
    assert_eq!(
        numbers(&w.app.issue_list(None, false).await.unwrap()),
        vec![("api".into(), 1), ("web".into(), 1), ("web".into(), 2), ("web".into(), 3)]
    );
    assert_eq!(w.app.issue_show("web", 3).await.unwrap()["status"], "parked");
}

#[tokio::test]
async fn an_issue_needs_a_listed_checkout_and_a_title() {
    let w = World::new();
    w.checkout("web").await;
    refused(w.app.issue_add("nope", "t", "", false, &[]).await, "no_checkout");
    refused(w.app.issue_add("web", "   ", "", false, &[]).await, "no_title");
    refused(w.app.issue_show("web", 1).await, "no_issue");
    refused(w.app.issue_close("web", 1, "r").await, "no_issue");
    assert_eq!(w.app.issue_list(None, true).await.unwrap(), json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_runs_adding_at_once_take_different_numbers() {
    let w = World::new();
    w.checkout("web").await;
    let runs = [Arc::new(w.run()), Arc::new(w.run())];
    let adds: Vec<_> = (0..8)
        .map(|i| {
            let app = runs[i % 2].clone();
            tokio::spawn(async move { app.issue_add("web", "same time", "", false, &[]).await.unwrap() })
        })
        .collect();
    let mut got = Vec::new();
    for add in adds {
        got.push(add.await.unwrap()["number"].as_u64().unwrap());
    }
    got.sort();
    assert_eq!(got, (1..=8).collect::<Vec<_>>());
}

#[tokio::test]
async fn closed_issues_leave_the_list_unless_asked_for_and_their_numbers_are_not_reused() {
    let w = World::new();
    w.checkout("web").await;
    w.checkout("api").await;
    w.app.issue_add("web", "one", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    w.app.issue_add("api", "three", "", false, &[]).await.unwrap();
    let closed = w.app.issue_close("web", 1, "done elsewhere").await.unwrap();
    assert_eq!((closed["status"].clone(), closed["reason"].clone()), (json!("closed"), json!("done elsewhere")));
    assert_eq!(numbers(&w.app.issue_list(None, false).await.unwrap()), vec![("api".into(), 1), ("web".into(), 2)]);
    assert_eq!(numbers(&w.app.issue_list(Some("web"), false).await.unwrap()), vec![("web".into(), 2)]);
    assert_eq!(
        numbers(&w.app.issue_list(Some("web"), true).await.unwrap()),
        vec![("web".into(), 1), ("web".into(), 2)]
    );
    assert_eq!(w.app.issue_add("web", "four", "", false, &[]).await.unwrap()["number"], 3);
}

#[tokio::test]
async fn an_edit_shows_in_the_list_and_keeps_what_it_does_not_name() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "old", "the body", false, &[]).await.unwrap();
    let e = w.app.issue_edit("web", 1, Some("new"), None).await.unwrap();
    assert_eq!((e["title"].clone(), e["body"].clone()), (json!("new"), json!("the body")));
    let e = w.app.issue_edit("web", 1, None, Some("another")).await.unwrap();
    assert_eq!((e["title"].clone(), e["body"].clone()), (json!("new"), json!("another")));
    refused(w.app.issue_edit("web", 1, Some(" "), None).await, "no_title");
    assert_eq!(w.app.issue_list(None, false).await.unwrap()[0]["title"], "new");
}

#[tokio::test]
async fn a_parked_issue_is_parked_in_the_list_until_readied() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "one", "", false, &[]).await.unwrap();
    let p = w.app.issue_park("web", 1, "needs a decision").await.unwrap();
    assert_eq!((p["status"].clone(), p["reason"].clone()), (json!("parked"), json!("needs a decision")));
    assert_eq!(w.app.issue_list(None, false).await.unwrap()[0]["status"], "parked");
    let r = w.app.issue_ready("web", 1).await.unwrap();
    assert_eq!((r["status"].clone(), r["reason"].clone()), (json!("ready"), json!(null)));
    assert_eq!(w.app.issue_list(None, false).await.unwrap()[0]["status"], "ready");
}

#[tokio::test]
async fn an_issue_waits_for_others_of_its_checkout() {
    let w = World::new();
    w.checkout("web").await;
    for t in ["one", "two", "three"] {
        w.app.issue_add("web", t, "", false, &[]).await.unwrap();
    }
    w.app.issue_block("web", 3, 1, false).await.unwrap();
    w.app.issue_block("web", 3, 1, false).await.unwrap();
    assert_eq!(w.app.issue_block("web", 3, 2, false).await.unwrap()["blocked_by"], json!([1, 2]));
    refused(w.app.issue_block("web", 3, 3, false).await, "blocks_itself");
    refused(w.app.issue_block("web", 3, 9, false).await, "no_issue");
    refused(w.app.issue_block("web", 9, 1, false).await, "no_issue");
    assert_eq!(w.app.issue_block("web", 3, 1, true).await.unwrap()["blocked_by"], json!([2]));
    assert_eq!(w.app.issue_block("web", 3, 1, true).await.unwrap()["blocked_by"], json!([2]));
}

#[tokio::test]
async fn what_one_run_records_the_next_run_reads() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "one", "", false, &[]).await.unwrap();
    let next = w.run();
    assert_eq!(numbers(&next.issue_list(None, false).await.unwrap()), vec![("web".into(), 1)]);
    assert_eq!(next.issue_add("web", "two", "", false, &[]).await.unwrap()["number"], 2);
}
