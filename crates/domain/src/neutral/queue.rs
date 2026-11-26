//! The heavy-command queue's start rule (SPEC.md §1.9): the shortest
//! declared hold first, none starved past the starve bound, one start per
//! settled reading beside what runs, the hold bound, and a line move past
//! its bound ahead of those that yield, then an urgent one ahead of those
//! that are not.

use uuid::Uuid;

/// One entry of the queue, as the rule reads it: the caller's entry says
/// what the rule needs through this.
pub trait Queued {
    fn id(&self) -> Uuid;
    /// What it is, to say what another waits behind.
    fn what(&self) -> &str;
    fn queued_at(&self) -> i64;
    fn started_at(&self) -> Option<i64>;
    /// Whether one past its bound goes ahead of it.
    fn yields(&self) -> bool;
    /// Whether it goes ahead of those that yield and are not, behind one past its bound.
    fn urgent(&self) -> bool;
    /// When what it does passes its bound, ms, if it has one.
    fn bound_at(&self) -> Option<i64>;
    /// How long it declared it holds what it waits for, ms; none: the default.
    fn declared(&self) -> Option<i64>;
    /// Since when it waits in the line at `now`, ms: when it was queued,
    /// unless it joined the line later.
    fn joined(&self, now: i64) -> i64;
}

/// How the line is ordered: how long one waits before it goes ahead of all
/// those that have not, and how long one that declares no hold is read to
/// hold, ms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Order {
    pub starve: i64,
    pub default: i64,
}

impl Order {
    /// Whether `e` has waited in the line past the starve bound at `now`.
    pub fn starved<E: Queued>(self, e: &E, now: i64) -> bool {
        now - e.joined(now) > self.starve
    }

    /// How long `e` is read to hold, ms: what it declared, or the default.
    pub fn holds<E: Queued>(self, e: &E) -> i64 {
        e.declared().unwrap_or(self.default)
    }

    /// Where `e` stands in the line by its own: the starved first, oldest
    /// first; then the shortest hold, oldest first.
    fn key<E: Queued>(self, e: &E, now: i64) -> (bool, i64, i64, Uuid) {
        let starved = self.starved(e, now);
        (!starved, if starved { 0 } else { self.holds(e) }, e.queued_at(), e.id())
    }
}

/// Whether `e` is past its bound at `now`.
fn overdue<E: Queued>(e: &E, now: i64) -> bool {
    e.bound_at().is_some_and(|b| now > b)
}

pub fn running<E: Queued>(entries: &[E]) -> impl Iterator<Item = &E> {
    entries.iter().filter(|e| e.started_at().is_some())
}

pub fn waiting<E: Queued>(entries: &[E]) -> impl Iterator<Item = &E> {
    entries.iter().filter(|e| e.started_at().is_none())
}

/// The waiting entry queued first.
pub fn oldest<E: Queued>(entries: &[E]) -> Option<&E> {
    waiting(entries).min_by_key(|e| (e.queued_at(), e.id()))
}

/// The waiting entries in the order they start at `now`, by `order`; but
/// one past its bound goes ahead of every entry that yields, just before
/// the first of them; and an urgent one ahead of every entry that yields
/// and is not urgent, just before the first of them, behind those past
/// their bound.
pub fn line<E: Queued>(entries: &[E], now: i64, order: Order) -> Vec<&E> {
    let first = |of: fn(&E) -> bool| waiting(entries).filter(|e| of(e)).map(|e| order.key(e, now)).min();
    let (first_yielding, first_ordinary) = (first(|e| e.yields()), first(|e| e.yields() && !e.urgent()));
    let mut line: Vec<&E> = waiting(entries).collect();
    line.sort_by_cached_key(|e| {
        let own = order.key(*e, now);
        // Past its bound first, then urgent, then the rest, at a place they share.
        let (ahead_of, rank) = match (overdue(*e, now), e.urgent()) {
            (true, _) => (first_yielding, 0),
            (false, true) => (first_ordinary, 1),
            (false, false) => (None, 2),
        };
        (ahead_of.map_or(own, |w| own.min(w)), rank, own)
    });
    line
}

/// The waiting entry that starts next at `now`: the first of its [`line`].
pub fn next<E: Queued>(entries: &[E], now: i64, order: Order) -> Option<&E> {
    line(entries, now, order).into_iter().next()
}

/// When a start is: the last one, and how long the host takes to show the
/// load a start brings, ms.
pub struct Starts {
    pub last: i64,
    pub settle: i64,
}

/// Whether `id` may start at `now`, given whether the host has room, when
/// the reading was taken, and the starts before; else why not.
pub fn may_start<E: Queued>(
    entries: &[E],
    starts: Starts,
    id: Uuid,
    room: Result<(), String>,
    read_at: i64,
    now: i64,
    order: Order,
) -> Result<(), String> {
    match next(entries, now, order) {
        Some(e) if e.id() == id => {}
        Some(e) => return Err(format!("waits behind {}", e.what())),
        None => return Err("not waiting".into()),
    }
    if running(entries).next().is_none() {
        return Ok(());
    }
    room?;
    if read_at <= starts.last || read_at - starts.last < starts.settle {
        return Err("one start per reading settled after the last start".into());
    }
    Ok(())
}

/// Whether the running `id` holds others up past `bound` and must end.
pub fn overheld<E: Queued>(entries: &[E], id: Uuid, now: i64, bound: i64) -> bool {
    let Some(e) = entries.iter().find(|e| e.id() == id) else { return false };
    let waiting_since = waiting(entries).map(|w| w.queued_at()).min();
    match (e.started_at(), waiting_since) {
        (Some(started), Some(since)) => now - started.max(since) > bound,
        _ => false,
    }
}
