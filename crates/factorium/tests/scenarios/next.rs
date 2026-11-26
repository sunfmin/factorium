//! Which ready issue starts next (SPEC §2 step 2): the one whose checkout
//! has the fewest live workers; among those, the one ready longest; then
//! by checkout and number. One checkout with many ready issues takes a
//! start in turn, not every one.

use factorium_domain::issue::Standing;
use factorium_domain::next;
use factorium_domain::tracker::{Holder, IssueRow};

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
