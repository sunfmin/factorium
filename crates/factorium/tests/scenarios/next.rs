//! Which ready issue starts next (SPEC §2 step 2): the one whose checkout
//! has the fewest live workers; among those, the one ready longest; then
//! by checkout and number. One checkout with many ready issues takes a
//! start in turn, not every one.

use factorium_domain::issue::Standing;
use factorium_domain::next;
use factorium_domain::tracker::{Holder, IssueRow};

use serde_json::json;

use crate::fixture::World;

fn ready(checkout: &str, number: u64, ready_at: i64) -> IssueRow {
    IssueRow {
        checkout: checkout.into(),
        number,
        title: String::new(),
        ready_at,
        standing: Standing::default(),
        holder: Holder { taken: false, worker: None },
        urgent: false,
        ahead: false,
    }
}

fn order(rows: Vec<IssueRow>, live: &[&str]) -> Vec<String> {
    next::order(rows, live.iter().copied()).into_iter().map(|r| format!("{}#{}", r.checkout, r.number)).collect()
}

#[test]
fn the_checkout_with_the_fewest_live_workers_goes_first_then_the_oldest_ready() {
    let rows = || vec![ready("app", 1, 10), ready("app", 2, 20), ready("web", 7, 30), ready("api", 3, 5)];
    assert_eq!(order(rows(), &[]), ["api#3", "app#1", "app#2", "web#7"], "none live: the oldest ready first");
    assert_eq!(order(rows(), &["api"]), ["app#1", "app#2", "web#7", "api#3"], "api has a worker: it goes last");
    assert_eq!(order(rows(), &["app", "api"]), ["web#7", "api#3", "app#1", "app#2"]);
    assert_eq!(order(rows(), &["app", "app", "web", "api"]), ["api#3", "web#7", "app#1", "app#2"]);
    // Ready at once: checkout and number decide, so the order is total.
    let same = vec![ready("web", 1, 0), ready("app", 2, 0), ready("app", 1, 0)];
    assert_eq!(order(same, &[]), ["app#1", "app#2", "web#1"]);
}

async fn started(w: &World) -> String {
    let t = w.tick().await;
    t.started.as_ref().map(|s| s["issue"].as_str().unwrap().to_string()).unwrap_or_else(|| panic!("a start: {t:?}"))
}

#[tokio::test]
async fn a_checkout_with_one_ready_issue_starts_beside_one_with_three() {
    let w = World::new();
    w.checkout("app").await;
    w.checkout("web").await;
    w.kit().await;
    for title in ["one", "two", "three"] {
        w.app.issue_add("app", title, "", false, &[]).await.unwrap();
        w.advance(1);
    }
    w.app.issue_add("web", "late", "", false, &[]).await.unwrap();
    assert_eq!(started(&w).await, "app#1", "the oldest ready, none live");
    assert_eq!(started(&w).await, "web#1", "web has no worker, app has one");
    assert_eq!(started(&w).await, "app#2");
}

#[tokio::test]
async fn within_a_checkout_the_issue_ready_longest_starts_first_whatever_its_number() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "asks first", "", true, &[]).await.unwrap();
    w.advance(1);
    w.app.issue_add("web", "ready first", "", false, &[]).await.unwrap();
    w.advance(1);
    w.app.issue_ready("web", 1).await.unwrap();
    assert_eq!(started(&w).await, "web#2", "web#1 became ready after it");
    assert_eq!(started(&w).await, "web#1");
}

/// A tick's start, if any.
async fn start(w: &World) -> Option<String> {
    w.tick().await.started.map(|s| s["issue"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn a_checkout_at_its_cap_starts_no_worker_while_another_checkouts_ready_issue_starts() {
    let w = World::new();
    w.checkout("app").await;
    w.checkout("web").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    w.app.settings_set(Some("workers_per_checkout"), Some(2)).await.unwrap();
    for title in ["one", "two", "three"] {
        w.app.issue_add("app", title, "", false, &[]).await.unwrap();
    }
    assert_eq!(start(&w).await.as_deref(), Some("app#1"));
    assert_eq!(start(&w).await.as_deref(), Some("app#2"));
    // One of its two in landing still counts: app is at its cap.
    w.app.hold("app", Some("a landing waits")).await.unwrap();
    w.done(&w.worker_on("app", 1).await, "one.txt", "one\n").await;
    w.tick().await;
    assert_eq!(w.worker_on("app", 1).await["status"], "landing");
    assert_eq!(start(&w).await, None, "app#3 waits for app's cap");
    let capped = w.doctor().await["capped"].clone();
    assert_eq!(capped, json!([{ "checkout": "app", "live": 2, "cap": 2, "waiting": ["app#3"] }]));

    w.app.issue_add("web", "late", "", false, &[]).await.unwrap();
    assert_eq!(start(&w).await.as_deref(), Some("web#1"), "another checkout's ready issue starts");
    assert_eq!(start(&w).await, None);
    // Its cap raised, app starts again, and doctor names no checkout at its cap.
    w.app.settings_set(Some("workers_per_checkout"), Some(3)).await.unwrap();
    assert_eq!(start(&w).await.as_deref(), Some("app#3"));
    assert_eq!(w.doctor().await["capped"], json!([]), "app is at 3 of 3, but nothing waits for it");
}

#[tokio::test]
async fn an_urgent_issue_of_a_checkout_at_its_cap_starts() {
    let w = World::new();
    w.checkout("app").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    w.app.settings_set(Some("workers_per_checkout"), Some(1)).await.unwrap();
    w.app.issue_add("app", "one", "", false, &[]).await.unwrap();
    w.app.issue_add("app", "two", "", false, &[]).await.unwrap();
    w.app.issue_add("app", "three", "", false, &[]).await.unwrap();
    assert_eq!(start(&w).await.as_deref(), Some("app#1"));
    assert_eq!(start(&w).await, None, "app is at its cap of 1");
    w.app.issue_mark_urgent("app", 3, true).await.unwrap();
    assert_eq!(start(&w).await.as_deref(), Some("app#3"), "the cap does not hold an urgent issue");
    assert_eq!(start(&w).await, None, "app#2 still waits");
}
