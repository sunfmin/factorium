//! The heavy-command queue's start rule: one host-wide line of the commands
//! that load the machine. The oldest waiting starts when the host has room,
//! or when nothing runs, so it never starves; one start per reading; and
//! one that holds others up past the hold bound is ended. An entry past its
//! bound takes the place of the oldest worker's entry waiting ahead of it:
//! workers' entries keep their order, and a running one is never cut short.

use uuid::Uuid;

/// One entry of the queue, as the rule reads it: the caller's entry says
/// what the rule needs through this.
pub trait Queued {
    fn id(&self) -> Uuid;
    /// What it is, to say what another waits behind.
    fn what(&self) -> &str;
    fn queued_at(&self) -> i64;
    fn started_at(&self) -> Option<i64>;
    /// Whether a worker queued it.
    fn workers(&self) -> bool;
    /// When what it does passes its bound, ms, if it has one.
    fn bound_at(&self) -> Option<i64>;
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

/// The waiting entry that starts next at `now`: the oldest, but one past its
/// bound goes ahead of every worker's entry, just before the oldest of them.
pub fn next<E: Queued>(entries: &[E], now: i64) -> Option<&E> {
    let first_worker = waiting(entries).filter(|e| e.workers()).map(|e| (e.queued_at(), e.id())).min();
    waiting(entries).min_by_key(|e| {
        let own = (e.queued_at(), e.id());
        let late = overdue(*e, now);
        let place = match first_worker {
            Some(w) if late => own.min(w),
            _ => own,
        };
        (place, !late, own)
    })
}

/// Whether `id` may start at `now`, given whether the host has room, when
/// the reading was taken, and when the last one started; else why not.
pub fn may_start<E: Queued>(
    entries: &[E],
    last_start: i64,
    id: Uuid,
    room: Result<(), String>,
    read_at: i64,
    now: i64,
) -> Result<(), String> {
    match next(entries, now) {
        Some(e) if e.id() == id => {}
        Some(e) => return Err(format!("waits behind {}", e.what())),
        None => return Err("not waiting".into()),
    }
    if running(entries).next().is_none() {
        return Ok(());
    }
    room?;
    if read_at <= last_start {
        return Err("one start per reading".into());
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
