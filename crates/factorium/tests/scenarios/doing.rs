//! What an issue is doing, as `issue list` and `issue show` answer its
//! `status`: its own status, or, held by a live worker, what that worker
//! does (`working`, `landing`, `parked`); and `workers`, one object
//! whatever the number of workers.

use serde_json::{Value, json};

use crate::fixture::{CLAUDE, World, id};

/// `web#number`'s status as `issue list` and as `issue show` answer it.
async fn status(w: &World, number: u64) -> (Value, Value) {
    let list = w.app.issue_list(Some("web"), false).await.unwrap();
    let row = list.as_array().unwrap().iter().find(|r| r["number"] == number).cloned().expect("listed");
    (row["status"].clone(), w.app.issue_show("web", number).await.unwrap()["status"].clone())
}

/// The live workers' answer: always an object holding `workers`, each row
/// naming its checkout and issue number.
async fn workers(w: &World) -> Vec<(Value, Value)> {
    let answer = w.app.workers(false).await.unwrap();
    let keys: Vec<&String> = answer.as_object().expect("an object").keys().collect();
    assert_eq!(keys, ["workers"], "{answer}");
    let rows = answer["workers"].as_array().expect("an array of workers");
    rows.iter().map(|r| (r["checkout"].clone(), r["number"].clone())).collect()
}

#[tokio::test]
async fn an_issue_shows_what_its_worker_does_and_workers_answers_one_shape() {
    let w = World::new();
    assert_eq!(workers(&w).await, [], "none");

    let one = w.working(CLAUDE, "one").await;
    assert_eq!(status(&w, 1).await, (json!("working"), json!("working")));
    assert_eq!(workers(&w).await, [(json!("web"), json!(1))], "one");

    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    assert_eq!(status(&w, 2).await, (json!("ready"), json!("ready")), "no worker yet");
    w.tick().await;
    assert_eq!(status(&w, 2).await, (json!("working"), json!("working")));
    assert_eq!(workers(&w).await, [(json!("web"), json!(1)), (json!("web"), json!(2))], "many");

    // #1's worker parked behind #2; #2's done waits for its landing.
    w.app.worker_escalate(id(&one), "needs #2").await.unwrap();
    w.app.issue_block("web", 1, 2, Some("needs #2's parser"), false).await.unwrap();
    assert_eq!(status(&w, 1).await, (json!("parked"), json!("parked")));
    w.done(&w.worker_on("web", 2).await, "two", "x").await;
    assert_eq!(status(&w, 2).await, (json!("landing"), json!("landing")));

    w.tick().await;
    let closed = w.app.issue_show("web", 2).await.unwrap();
    assert_eq!(closed["status"], "closed", "landed: {closed}");
}
