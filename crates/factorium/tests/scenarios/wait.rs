//! `issue wait`: a run that returns once the issues it names land, close,
//! or wait for a person, or at its timeout, each with its own exit code.

use factorium_domain::issue::Until;
use serde_json::{Value, json};

use crate::fixture::{World, bounded, refused};

/// How often a waiter looks at its issues again: ms on the app's clock (`wait.rs`).
const LOOK_MS: i64 = 1000;

/// `web` with #1 and #2 open and a worker on each.
async fn two_at_work() -> World {
    let w = World::new();
    w.two_at_work().await;
    w
}

/// Waits on `numbers` of `web` in another CLI run while `meanwhile` runs,
/// once the waiter is asleep on the clock; then moves the clock a look on.
async fn waited(
    w: &World,
    numbers: &[u64],
    until: Until,
    timeout: Option<i64>,
    meanwhile: impl AsyncFnOnce(),
) -> Value {
    let run = w.run();
    let waiting = run.issue_wait("web", numbers, until, timeout);
    let driving = async {
        w.until(async || w.clock.asleep() > 0).await;
        meanwhile().await;
        w.clock.advance(LOOK_MS);
    };
    bounded("the wait ending", async { tokio::join!(waiting, driving).0 }).await.unwrap()
}

#[tokio::test]
async fn an_issue_that_lands_while_waited_on_answers_exit_0_with_its_commit() {
    let w = two_at_work().await;
    let mut commit = String::new();
    let answer = waited(&w, &[1], Until::Any, None, async || commit = w.lands("web", 1, "a").await.commit).await;
    assert_eq!(answer["outcome"], "landed");
    assert_eq!(answer["code"], 0);
    let one = &answer["issues"][0];
    assert_eq!((one["number"].clone(), one["status"].clone()), (json!(1), json!("closed")));
    assert_eq!(one["commit"], commit.as_str());
    assert_eq!(one["outcome"], "landed");
}

#[tokio::test]
async fn an_issue_left_to_a_person_answers_its_own_code_and_the_why() {
    let w = two_at_work().await;
    let answer = waited(&w, &[1], Until::Any, None, async || {
        w.app.issue_wait_for_human("web", 1, "which schema?").await.unwrap();
    })
    .await;
    assert_eq!((answer["outcome"].clone(), answer["code"].clone()), (json!("waiting_for_human"), json!(4)));
    assert_eq!(answer["issues"][0]["reason"], "which schema?");
    assert_eq!(answer["issues"][0]["commit"], Value::Null);
}

#[tokio::test]
async fn until_closed_waits_past_a_person_and_a_close_without_landing_has_its_own_code() {
    let w = two_at_work().await;
    let answer = waited(&w, &[1], Until::Closed, None, async || {
        w.app.issue_wait_for_human("web", 1, "which schema?").await.unwrap();
        // The waiter looks, and waits on.
        w.clock.advance(LOOK_MS);
        w.until(async || w.clock.asleep() > 0).await;
        w.app.issue_close("web", 1, "not needed").await.unwrap();
    })
    .await;
    assert_eq!((answer["outcome"].clone(), answer["code"].clone()), (json!("closed"), json!(3)));
    assert_eq!(answer["issues"][0]["reason"], "not needed");
}

#[tokio::test]
async fn several_issues_end_the_wait_once_all_have_and_its_answer_is_the_worst() {
    let w = two_at_work().await;
    let answer = waited(&w, &[1, 2], Until::Any, None, async || {
        w.lands("web", 1, "a").await;
        w.clock.advance(LOOK_MS);
        w.until(async || w.clock.asleep() > 0).await;
        w.app.issue_close("web", 2, "not needed").await.unwrap();
    })
    .await;
    let outcomes: Vec<_> = answer["issues"].as_array().unwrap().iter().map(|i| i["outcome"].clone()).collect();
    assert_eq!(outcomes, [json!("landed"), json!("closed")]);
    assert_eq!((answer["outcome"].clone(), answer["code"].clone()), (json!("closed"), json!(3)));
}

#[tokio::test]
async fn at_its_timeout_it_answers_its_own_code_and_each_status_then() {
    let w = two_at_work().await;
    let answer = waited(&w, &[1], Until::Any, Some(LOOK_MS), async || {}).await;
    assert_eq!((answer["outcome"].clone(), answer["code"].clone()), (json!("timeout"), json!(5)));
    let one = &answer["issues"][0];
    assert_eq!((one["status"].clone(), one["outcome"].clone()), (json!("ready"), json!("timeout")));
}

#[tokio::test]
async fn an_issue_already_ended_answers_at_once_and_one_not_there_is_refused() {
    let w = two_at_work().await;
    w.app.issue_close("web", 2, "not needed").await.unwrap();
    let answer = w.run().issue_wait("web", &[2], Until::Any, None).await.unwrap();
    assert_eq!(answer["code"], 3);
    refused(w.run().issue_wait("web", &[2, 9], Until::Any, None).await, "no_issue");
}
