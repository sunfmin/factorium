//! A resource's queue, by the domain's rule alone: the shortest declared
//! hold first, none starved past `starve_bound`, a hold with a window only
//! inside it, and the host's queue in its old order where none declares a
//! hold (factorium#286).

use factorium_domain::heavy::{Entry, HOST, Queue, Reading, Window};
use factorium_domain::neutral::queue::{self, Order};
use factum::Uuid;

const MIN: i64 = 60_000;
const ORDER: Order = Order { starve: 240 * MIN, default: 30 * MIN };

fn entry(n: u128, queued_at: i64) -> Entry {
    Entry { id: Uuid::from_u128(n), what: n.to_string(), queued_at, ..Entry::default() }
}

fn on(e: Entry, resources: &[&str]) -> Entry {
    Entry { resources: resources.iter().map(|r| r.to_string()).collect(), ..e }
}

/// A worker's hold of `name` for `min` minutes, asked at `queued_at`.
fn hold(n: u128, queued_at: i64, min: i64, name: &str) -> Entry {
    let worker = Some(Uuid::from_u128(100 + n));
    Entry { hold_for: Some(min * MIN), worker, ..on(entry(n, queued_at), &[name]) }
}

fn starts(q: &Queue, id: Uuid, now: i64) -> Result<(), String> {
    q.may_start(id, Ok(()), Reading { read_at: now, settle: 0, order: ORDER }, now)
}

/// Why each waiting for resource `name` at `now` is not next, in the order they start.
fn whys(q: &Queue, name: &str, now: i64) -> Vec<Option<String>> {
    let r = q.resources(now, ORDER).into_iter().find(|r| r.name == name).unwrap();
    r.waiting.into_iter().map(|h| h.why).collect()
}

#[test]
fn a_five_minute_hold_asked_after_a_160_minute_one_goes_first() {
    let (long, short) = (hold(1, 0, 160, "FMGG"), hold(2, MIN, 5, "FMGG"));
    let q = Queue { entries: vec![long.clone(), short.clone()], ..Queue::default() };
    assert_eq!(starts(&q, short.id, 2 * MIN), Ok(()));
    assert_eq!(starts(&q, long.id, 2 * MIN).unwrap_err(), "waits for FMGG behind `2`, a 5 min hold");
    // A command declares none: it is read as the default, between the two.
    let command = on(entry(3, 2 * MIN), &["FMGG"]);
    let q = Queue { entries: vec![long.clone(), command.clone()], ..Queue::default() };
    assert_eq!(starts(&q, command.id, 3 * MIN), Ok(()));
    let shown = &q.resources(3 * MIN, ORDER)[0];
    assert_eq!(shown.waiting.iter().map(|h| h.entry).collect::<Vec<_>>(), [command.id, long.id]);
    assert_eq!(shown.waiting[1].why.as_deref(), Some("behind `3`, a 30 min hold (the default)"));
    assert_eq!((shown.waiting[1].declared, shown.waiting[0].why.clone()), (Some(160 * MIN), None));
    assert_eq!(whys(&q, HOST, 3 * MIN), Vec::<Option<String>>::new(), "none of them asks for the host");
    // Hours and minutes, as the owner reads them.
    let longer = hold(4, 0, 200, "FMGG");
    let q = Queue { entries: vec![long, longer], ..Queue::default() };
    assert_eq!(whys(&q, "FMGG", MIN), [None, Some("behind `1`, a 2 h 40 min hold".into())]);
}

#[test]
fn a_long_hold_past_the_starve_bound_goes_ahead_of_short_ones() {
    let long = hold(1, 0, 160, "FMGG");
    let shorts = [hold(2, 200 * MIN, 5, "FMGG"), hold(3, 230 * MIN, 1, "FMGG")];
    let q = Queue { entries: [vec![long.clone()], shorts.to_vec()].concat(), ..Queue::default() };
    assert_eq!(starts(&q, shorts[1].id, 240 * MIN), Ok(()), "at the bound, the shortest");
    let past = 240 * MIN + 1;
    assert_eq!(starts(&q, long.id, past), Ok(()), "past it, the long one");
    assert_eq!(starts(&q, shorts[1].id, past).unwrap_err(), "waits for FMGG behind `1`, waiting past the starve bound");
}

#[test]
fn a_hold_with_a_window_starts_inside_it_only_and_joins_the_line_as_it_opens() {
    // 22:00-07:00 at UTC+9: 13:00 to 22:00 UTC.
    let window = Some(Window { from: 22 * 60, to: 7 * 60, offset: 9 * 60 });
    let night = Entry { window, ..hold(1, 0, 600, "FMGG") };
    let q = Queue { entries: vec![night.clone()], ..Queue::default() };
    let (noon_utc, opens, closes) = (12 * 60 * MIN, 13 * 60 * MIN, 22 * 60 * MIN);
    assert_eq!(starts(&q, night.id, noon_utc).unwrap_err(), "waits for its window from 22:00");
    let shown = &q.resources(noon_utc, ORDER)[0];
    assert_eq!(shown.waiting[0].why.as_deref(), Some("waits for its window from 22:00"));
    assert_eq!(shown.waiting[0].window.as_deref(), Some("22:00-07:00"));
    assert_eq!(starts(&q, night.id, opens), Ok(()), "inside, at its opening");
    assert_eq!(starts(&q, night.id, closes - MIN), Ok(()), "inside, at 06:59");
    assert!(starts(&q, night.id, closes).is_err(), "closed again at 07:00");
    // Waiting all day before its window: it joined the line as it opened, so it is not starved.
    let short = hold(2, opens + MIN, 5, "FMGG");
    let q = Queue { entries: vec![night.clone(), short.clone()], ..Queue::default() };
    assert_eq!(starts(&q, short.id, opens + 2 * MIN), Ok(()));
    assert_eq!(starts(&q, night.id, opens + 241 * MIN), Ok(()), "past the bound from its opening");
}

#[test]
fn the_hosts_queue_keeps_its_order_where_none_declares_a_hold() {
    // Ten hours apart, the first past the starve bound, a line move past its bound among them.
    let (a, b) = (Entry { worker: Some(Uuid::from_u128(9)), ..entry(1, 0) }, entry(2, 600 * MIN));
    let line = Entry { bound_at: Some(MIN), ..entry(3, 601 * MIN) };
    let q = Queue { entries: vec![b.clone(), line.clone(), a.clone()], ..Queue::default() };
    let order = |o| queue::line(&q.entries, 602 * MIN, o).iter().map(|e| e.id).collect::<Vec<_>>();
    assert_eq!(order(ORDER), order(Order::default()));
    assert_eq!(order(ORDER), [line.id, a.id, b.id]);
    assert_eq!(whys(&q, HOST, 602 * MIN)[1].as_deref(), Some("behind `3`, a line move past its bound"));
    // At its bound, not past it: a line move is first only as the oldest.
    let (due, then) = (Entry { bound_at: Some(MIN), ..entry(4, 0) }, entry(5, MIN / 2));
    let q = Queue { entries: vec![due, then], ..Queue::default() };
    assert_eq!(whys(&q, HOST, MIN), [None, Some("behind `4`, queued before it".into())]);
}

#[test]
fn a_window_reads_two_times_of_day_that_differ() {
    use factorium_domain::when::window;
    assert_eq!(window("22:00-07:00"), Ok((1320, 420)));
    assert_eq!(window("9:30 - 10:05"), Ok((570, 605)));
    for bad in ["22:00", "22:00-22:00", "24:00-01:00", "10:60-11:00", "a-b"] {
        assert!(window(bad).is_err(), "{bad}");
    }
}
