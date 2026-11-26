//! Which ready issue starts next (SPEC §2 step 2): the one whose checkout
//! has the fewest live workers; among those, the one ready longest; then
//! by checkout and number, so the order is total.

use std::collections::BTreeMap;

use crate::tracker::IssueRow;

/// `ready` in the order they start, given the checkout of each live worker.
pub fn order<'a>(mut ready: Vec<IssueRow>, live: impl IntoIterator<Item = &'a str>) -> Vec<IssueRow> {
    let mut workers = BTreeMap::<&str, usize>::new();
    for checkout in live {
        *workers.entry(checkout).or_default() += 1;
    }
    let on = |r: &IssueRow| workers.get(r.checkout.as_str()).copied().unwrap_or(0);
    ready.sort_by(|x, y| {
        (on(x), x.ready_at, &x.checkout, x.number).cmp(&(on(y), y.ready_at, &y.checkout, y.number))
    });
    ready
}
