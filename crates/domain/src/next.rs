//! Which ready issue starts next (SPEC §2 step 2): the one whose checkout
//! has the fewest live workers; among those, the one ready longest; then
//! by checkout and number, so the order is total. A checkout with
//! `workers_per_checkout` live workers starts none but an urgent issue's or
//! one that starts ahead (a promotion's mutants issue, which admission's
//! load rules still hold); that worker still counts as live.

use std::collections::BTreeMap;

use crate::tracker::IssueRow;

/// How many live workers each checkout has, given the checkout of each.
pub fn live<'a>(workers: impl IntoIterator<Item = &'a str>) -> BTreeMap<&'a str, usize> {
    let mut live = BTreeMap::<&str, usize>::new();
    for checkout in workers {
        *live.entry(checkout).or_default() += 1;
    }
    live
}

/// `ready` in the order they start, given the checkout of each live worker.
pub fn order<'a>(mut ready: Vec<IssueRow>, workers: impl IntoIterator<Item = &'a str>) -> Vec<IssueRow> {
    let live = live(workers);
    let on = |r: &IssueRow| live.get(r.checkout.as_str()).copied().unwrap_or(0);
    ready.sort_by(|x, y| (on(x), x.ready_at, &x.checkout, x.number).cmp(&(on(y), y.ready_at, &y.checkout, y.number)));
    ready
}

/// Whether its checkout's cap holds the start of `issue`: neither urgent nor
/// starting ahead, and its checkout runs `cap` live workers or more, those
/// in landing among them.
pub fn capped(live: &BTreeMap<&str, usize>, issue: &IssueRow, cap: u64) -> bool {
    !issue.urgent && !issue.ahead && live.get(issue.checkout.as_str()).copied().unwrap_or(0) as u64 >= cap
}
