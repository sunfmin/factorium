//! The heavy-command queue's start rule: one host-wide line of the commands
//! that load the machine. The oldest waiting starts when the host has room,
//! or when nothing runs, so it never starves; one start per reading; and
//! one that holds others up past the hold bound is ended.

use uuid::Uuid;

/// One entry of the queue, as the rule reads it: the caller's entry says
/// what the rule needs through this.
pub trait Queued {
    fn id(&self) -> Uuid;
    /// What it is, to say what another waits behind.
    fn what(&self) -> &str;
    fn queued_at(&self) -> i64;
    fn started_at(&self) -> Option<i64>;
}

pub fn running<E: Queued>(entries: &[E]) -> impl Iterator<Item = &E> {
    entries.iter().filter(|e| e.started_at().is_some())
}

pub fn waiting<E: Queued>(entries: &[E]) -> impl Iterator<Item = &E> {
    entries.iter().filter(|e| e.started_at().is_none())
}

/// The waiting entry that starts next.
pub fn oldest<E: Queued>(entries: &[E]) -> Option<&E> {
    waiting(entries).min_by_key(|e| (e.queued_at(), e.id()))
}

/// Whether `id` may start, given whether the host has room, when the
/// reading was taken, and when the last one started; else why not.
pub fn may_start<E: Queued>(
    entries: &[E],
    last_start: i64,
    id: Uuid,
    room: Result<(), String>,
    read_at: i64,
) -> Result<(), String> {
    match oldest(entries) {
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
