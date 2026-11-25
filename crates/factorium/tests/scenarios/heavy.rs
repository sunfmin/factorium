//! The heavy queue's resources and the line's moves, by the domain's rule
//! alone (factorium#304: once a never-built unit test of `heavy.rs`).

use factorium_domain::heavy::{Ask, Entry, HOST, Queue, Reading};
use factorium_domain::neutral::queue;
use factum::Uuid;

fn at(read_at: i64) -> Reading {
    Reading { read_at, ..Reading::default() }
}

fn entry(n: u128, queued_at: i64) -> Entry {
    Entry { ask: Ask { id: Uuid::from_u128(n), what: n.to_string(), ..Ask::default() }, queued_at, ..Entry::default() }
}

/// `e` with `f` done to its ask.
fn with(mut e: Entry, f: impl FnOnce(&mut Ask)) -> Entry {
    f(&mut e.ask);
    e
}

#[test]
fn a_line_move_past_its_bound_goes_ahead_of_a_programs_entry_as_of_a_workers() {
    let line = with(entry(3, 2), |a| a.bound_at = Some(5));
    for asker in [with(entry(1, 1), |a| a.program = true), with(entry(1, 1), |a| a.worker = Some(Uuid::from_u128(9)))] {
        let q = Queue { entries: vec![asker, line.clone()], ..Queue::default() };
        assert_eq!(queue::next(&q.entries, 6, queue::Order::default()).map(|e| e.ask.id), Some(line.ask.id));
    }
    // A cut, say, neither a worker's nor a program's: it keeps its place.
    let q = Queue { entries: vec![entry(1, 1), line.clone()], ..Queue::default() };
    assert_eq!(queue::next(&q.entries, 6, queue::Order::default()).map(|e| e.ask.id), Some(Uuid::from_u128(1)));
}

fn on(e: Entry, resources: &[&str]) -> Entry {
    with(e, |a| a.resources = resources.iter().map(|r| r.to_string()).collect())
}

#[test]
fn one_waiting_for_a_full_resource_holds_up_none_but_those_asking_for_it() {
    let holder = Entry { started_at: Some(1), ..on(entry(1, 0), &["FMGG"]) };
    let (second, third) = (on(entry(2, 2), &["FMGG", HOST]), entry(3, 3));
    let q = Queue { entries: vec![holder.clone(), second.clone(), third.clone()], ..Queue::default() };
    let why = q.may_start(second.ask.id, Ok(()), at(10), 10).unwrap_err();
    assert!(why.starts_with("waits for FMGG, held by `1`"), "{why}");
    assert_eq!(q.may_start(third.ask.id, Ok(()), at(10), 10), Ok(()), "the third starts at once: no host entry runs");
    assert_eq!(q.oldest(10).map(|e| e.ask.id), Some(third.ask.id), "admission's wait leaves the second out");
    assert!(!q.overheld(holder.ask.id, 1_000_000, 30), "a named resource is never cut short");
}

#[test]
fn a_workers_command_inside_its_hold_takes_no_slot_of_its_own() {
    let w = Some(Uuid::from_u128(9));
    let hold =
        Entry { started_at: Some(1), ..with(on(entry(1, 0), &["FMGG"]), |a| (a.hold_for, a.worker) = (Some(60), w)) };
    let (mine, theirs) = (with(on(entry(2, 2), &["FMGG"]), |a| a.worker = w), on(entry(3, 1), &["FMGG"]));
    let q = Queue { entries: vec![hold, mine.clone(), theirs.clone()], ..Queue::default() };
    assert_eq!(q.may_start(mine.ask.id, Ok(()), at(10), 10), Ok(()));
    assert!(q.may_start(theirs.ask.id, Ok(()), at(10), 10).is_err());
}
