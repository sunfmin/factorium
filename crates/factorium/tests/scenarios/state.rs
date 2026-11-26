//! A stream's state that does not read is refused, never answered as if the
//! stream were new.

use crate::fixture::World;

#[tokio::test]
async fn a_checkout_whose_stored_state_does_not_read_is_refused_not_answered_as_new() {
    let w = World::new();
    w.checkout("web").await;
    w.snapshots_that_do_not_read("checkout");
    let refused = w.app.issue_add("web", "a", "", false, &[]).await.expect_err("its state does not read");
    // Not `no_checkout`, as a new stream would answer: factum's fold refuses
    // it, as the store's, which the watch hears of.
    assert_eq!(refused.code, "store", "{refused}");
    assert!(refused.message.contains("checkout state") && refused.message.contains("integer `5`"), "{refused}");
}
