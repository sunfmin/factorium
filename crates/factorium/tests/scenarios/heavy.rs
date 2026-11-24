//! The heavy queue's resources and the line's moves, by the domain's rule
//! alone (factorium#304: once a never-built unit test of `heavy.rs`).

use std::collections::BTreeMap;

use factorium_domain::heavy::{Entry, HOST, Queue, Reading};
use factorium_domain::neutral::queue;
use factum::Uuid;

fn at(read_at: i64) -> Reading {
    Reading { read_at, ..Reading::default() }
}

fn entry(n: u128, queued_at: i64) -> Entry {
    Entry { id: Uuid::from_u128(n), what: n.to_string(), queued_at, ..Entry::default() }
}

#[test]
fn a_line_move_past_its_bound_goes_ahead_of_a_programs_entry_as_of_a_workers() {
    let line = Entry { bound_at: Some(5), ..entry(3, 2) };
    for asker in [Entry { program: true, ..entry(1, 1) }, Entry { worker: Some(Uuid::from_u128(9)), ..entry(1, 1) }] {
        let q = Queue { entries: vec![asker, line.clone()], ..Queue::default() };
        assert_eq!(queue::next(&q.entries, 6, queue::Order::default()).map(|e| e.id), Some(line.id));
    }
    // A cut, say, neither a worker's nor a program's: it keeps its place.
    let q = Queue { entries: vec![entry(1, 1), line.clone()], ..Queue::default() };
    assert_eq!(queue::next(&q.entries, 6, queue::Order::default()).map(|e| e.id), Some(Uuid::from_u128(1)));
}

fn on(e: Entry, resources: &[&str]) -> Entry {
    Entry { resources: resources.iter().map(|r| r.to_string()).collect(), ..e }
}

#[test]
fn one_waiting_for_a_full_resource_holds_up_none_but_those_asking_for_it() {
    let holder = Entry { started_at: Some(1), ..on(entry(1, 0), &["FMGG"]) };
    let (second, third) = (on(entry(2, 2), &["FMGG", HOST]), entry(3, 3));
    let q = Queue { entries: vec![holder.clone(), second.clone(), third.clone()], ..Queue::default() };
    let why = q.may_start(second.id, Ok(()), at(10), 10).unwrap_err();
    assert!(why.starts_with("waits for FMGG, held by `1`"), "{why}");
    assert_eq!(q.may_start(third.id, Ok(()), at(10), 10), Ok(()), "the third starts at once: no host entry runs");
    assert_eq!(q.oldest(10).map(|e| e.id), Some(third.id), "admission's wait leaves the second out");
    assert!(!q.overheld(holder.id, 1_000_000, 30), "a named resource is never cut short");
}

#[test]
fn a_workers_command_inside_its_hold_takes_no_slot_of_its_own() {
    let w = Some(Uuid::from_u128(9));
    let hold = Entry { started_at: Some(1), hold_for: Some(60), worker: w, ..on(entry(1, 0), &["FMGG"]) };
    let (mine, theirs) = (Entry { worker: w, ..on(entry(2, 2), &["FMGG"]) }, on(entry(3, 1), &["FMGG"]));
    let q = Queue { entries: vec![hold, mine.clone(), theirs.clone()], ..Queue::default() };
    assert_eq!(q.may_start(mine.id, Ok(()), at(10), 10), Ok(()));
    assert!(q.may_start(theirs.id, Ok(()), at(10), 10).is_err());
}

#[test]
fn a_resource_of_two_slots_takes_two_holders() {
    let slots = BTreeMap::from([("MAC".to_string(), 2)]);
    let one = Entry { started_at: Some(1), ..on(entry(1, 0), &["MAC"]) };
    let (two, three) = (on(entry(2, 1), &["MAC"]), on(entry(3, 2), &["MAC"]));
    let q = Queue { entries: vec![one.clone(), two.clone(), three.clone()], slots: slots.clone(), ..Queue::default() };
    assert_eq!(q.may_start(two.id, Ok(()), at(10), 10), Ok(()));
    assert!(q.may_start(three.id, Ok(()), at(10), 10).is_err(), "the oldest first");
    let two = Entry { started_at: Some(5), ..two };
    let q = Queue { entries: vec![one, two, three.clone()], slots, ..Queue::default() };
    assert!(q.may_start(three.id, Ok(()), at(10), 10).unwrap_err().starts_with("waits for MAC, held by"));
}
