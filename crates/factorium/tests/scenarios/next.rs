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
    let rows = || vec![ready("a", 1, 10), ready("a", 2, 20), ready("b", 7, 30), ready("c", 3, 5)];
    assert_eq!(order(rows(), &[]), ["c#3", "a#1", "a#2", "b#7"], "none live: the oldest ready first");
    assert_eq!(order(rows(), &["c"]), ["a#1", "a#2", "b#7", "c#3"], "c has a worker: it goes last");
    assert_eq!(order(rows(), &["a", "c"]), ["b#7", "c#3", "a#1", "a#2"]);
    assert_eq!(order(rows(), &["a", "a", "b", "c"]), ["c#3", "b#7", "a#1", "a#2"]);
    // Ready at once: checkout and number decide, so the order is total.
    let same = vec![ready("b", 1, 0), ready("a", 2, 0), ready("a", 1, 0)];
    assert_eq!(order(same, &[]), ["a#1", "a#2", "b#1"]);
}

async fn started(w: &World) -> String {
    let t = w.tick().await;
    t.started.as_ref().map(|s| s["issue"].as_str().unwrap().to_string()).unwrap_or_else(|| panic!("a start: {t:?}"))
}

#[tokio::test]
async fn a_checkout_with_one_ready_issue_starts_beside_one_with_three() {
    let w = World::new();
    w.checkout("a").await;
    w.checkout("b").await;
    w.kit().await;
    for title in ["one", "two", "three"] {
        w.app.issue_add("a", title, "", false, &[]).await.unwrap();
        w.advance(1);
    }
    w.app.issue_add("b", "late", "", false, &[]).await.unwrap();
    assert_eq!(started(&w).await, "a#1", "the oldest ready, none live");
    assert_eq!(started(&w).await, "b#1", "b has no worker, a has one");
    assert_eq!(started(&w).await, "a#2");
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
