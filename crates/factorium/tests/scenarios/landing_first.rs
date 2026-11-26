//! A landing's fast gate in the heavy-command queue (docs/adr/0009-admission-and-the-heavy-queue.md): it goes
//! ahead of workers' and programs' waiting entries that are not urgent,
//! behind a line move past its bound and an urgent issue's entry, oldest
//! first among landings' gates; a running entry is never cut short for it.

use factorium_domain::heavy::{Ask, Entry, Queue, Reading};
use factorium_domain::neutral::queue::{self, Order};
use factum::Uuid;
use serde_json::Value;

use crate::fixture::{LOOK, POLL, STUCK, World, id};

const MIN: i64 = 60_000;
const ORDER: Order = Order { starve: 240 * MIN, default: 30 * MIN };

fn entry(n: u128, what: &str, queued_at: i64, f: impl FnOnce(&mut Ask)) -> Entry {
    let mut ask = Ask { id: Uuid::from_u128(n), what: what.into(), ..Ask::default() };
    f(&mut ask);
    Entry { ask, queued_at, ..Entry::default() }
}

/// A worker's waiting entry `n`, queued at `queued_at`.
fn workers(n: u128, queued_at: i64, urgent: bool) -> Entry {
    entry(n, &n.to_string(), queued_at, |a| (a.worker, a.urgent) = (Some(Uuid::from_u128(100 + n)), urgent))
}

/// The fast gate of `checkout`'s landing of a worker, queued at `queued_at`.
fn landing(n: u128, checkout: &str, queued_at: i64) -> Entry {
    entry(n, &format!("fast gate of {checkout}"), queued_at, |a| a.lands = Some(Uuid::from_u128(100 + n)))
}

/// A line move past its bound: a promotion's slow gate.
fn overdue(n: u128, queued_at: i64) -> Entry {
    entry(n, "slow gate of web", queued_at, |a| a.bound_at = Some(0))
}

fn ids(line: Vec<&Entry>) -> Vec<u128> {
    line.iter().map(|e| e.ask.id.as_u128()).collect()
}

#[test]
fn the_line_ranks_past_its_bound_then_urgent_then_a_landings_gate_then_the_rest() {
    let slow = entry(7, "slow gate of api", MIN / 2, |_| {});
    let (rest, later) = (workers(1, 0, false), workers(2, MIN, false));
    let (web, api) = (landing(3, "web", 2 * MIN), landing(6, "api", 5 * MIN / 2));
    let (urgent, late) = (workers(4, 3 * MIN, true), overdue(5, 4 * MIN));
    let entries = vec![rest, later, web.clone(), urgent, late, api, slow];
    // Among landings' gates, oldest first; a slow gate not past its bound keeps its own place.
    assert_eq!(ids(queue::line(&entries, 5 * MIN, ORDER)), [5, 4, 3, 6, 1, 7, 2]);
    // A landing's gate queued before them all still goes behind the move past its bound and the urgent one.
    let first = Entry { queued_at: -MIN, ..web };
    let entries: Vec<_> =
        entries.into_iter().map(|e| if e.ask.id == first.ask.id { first.clone() } else { e }).collect();
    assert_eq!(ids(queue::line(&entries, 5 * MIN, ORDER)), [5, 4, 3, 6, 1, 7, 2]);
    // An urgent landing's gate ranks as urgent: among the urgent, the ordinary order.
    let entries: Vec<_> = entries
        .into_iter()
        .map(|e| if e.ask.id.as_u128() == 6 { Entry { ask: Ask { urgent: true, ..e.ask }, ..e } } else { e })
        .collect();
    assert_eq!(ids(queue::line(&entries, 5 * MIN, ORDER)), [5, 6, 4, 3, 1, 7, 2]);
}

#[test]
fn a_landings_gate_queued_after_workers_starts_next_behind_an_urgent_entry_and_a_move_past_its_bound() {
    let (a, b, gate) = (workers(1, 0, false), workers(2, MIN, false), landing(3, "web", 2 * MIN));
    let q = Queue { entries: vec![a.clone(), b, gate.clone()], ..Queue::default() };
    let now = 3 * MIN;
    let at = Reading { read_at: now, settle: 0, order: ORDER };
    assert_eq!(q.may_start(gate.ask.id, Ok(()), at, now), Ok(()));
    assert_eq!(q.may_start(a.ask.id, Ok(()), at, now).unwrap_err(), "waits behind fast gate of web");
    let whys: Vec<_> = q.resources(now, ORDER)[0].waiting.iter().map(|h| h.why.clone()).collect();
    assert_eq!(whys[0], None, "{whys:?}");
    assert_eq!(whys[1].as_deref(), Some("behind `fast gate of web`, a landing's gate"));
    // A running entry is never cut short for it.
    let running = Entry { started_at: Some(0), ..workers(8, 0, false) };
    let busy = Queue { entries: [q.entries.clone(), vec![running.clone()]].concat(), ..Queue::default() };
    assert_eq!(busy.ends(running.ask.id, now, i64::MAX, false), None);
    // An urgent worker's entry and a line move past its bound go ahead of it.
    let (urgent, late) = (workers(4, 4 * MIN, true), overdue(5, 4 * MIN));
    let q = Queue { entries: [q.entries.clone(), vec![urgent.clone(), late.clone()]].concat(), ..Queue::default() };
    let now = 5 * MIN;
    let at = Reading { read_at: now, settle: 0, order: ORDER };
    assert_eq!(q.may_start(late.ask.id, Ok(()), at, now), Ok(()));
    let without =
        |id: Uuid| Queue { entries: q.entries.iter().filter(|e| e.ask.id != id).cloned().collect(), ..q.clone() };
    assert_eq!(without(late.ask.id).may_start(urgent.ask.id, Ok(()), at, now), Ok(()));
    assert_eq!(q.may_start(gate.ask.id, Ok(()), at, now).unwrap_err(), "waits behind slow gate of web");
    let whys: Vec<_> = without(late.ask.id).resources(now, ORDER)[0].waiting.iter().map(|h| h.why.clone()).collect();
    assert_eq!(whys[1].as_deref(), Some("behind `4`, an urgent issue's"), "{whys:?}");
}

/// The host's queue as doctor shows it: each waiting entry's `what` and `why`.
async fn host_waiting(w: &World) -> Vec<(String, Value)> {
    let r = w.doctor().await["resources"].clone();
    let host = r.as_array().unwrap().iter().find(|r| r["name"] == "host").cloned().unwrap();
    let waiting = host["waiting"].as_array().cloned().unwrap_or_default();
    waiting.iter().map(|h| (h["what"].as_str().unwrap().to_string(), h["why"].clone())).collect()
}

#[tokio::test]
async fn a_landings_gate_starts_ahead_of_a_workers_command_that_waited_longer() {
    let w = World::new();
    w.two_at_work().await;
    let one = id(&w.worker_on("web", 1).await);
    // The worker's command fails unless the landing's gate ran before it.
    let gated = w.dir.join("gated");
    let toml = format!("[gate]\nfast = \"touch {}\"\n", gated.display());
    let line = format!("test -e {}", gated.display());
    let cli = w.run();
    let mut first = std::pin::pin!(cli.worker_exclusive(Some(one), &w.dir, &line));
    w.while_busy(async {
        tokio::select! { _ = &mut first => panic!("it ran beside a busy host"), () = w.until_queued(&line) => {} }
        w.clock.advance(1_000);
        let lands = async {
            w.done(&w.worker_on("web", 2).await, "factorium.toml", &toml).await;
            assert!(w.tick().await.landed.is_empty(), "the landing waits its turn");
            let waiting = host_waiting(&w).await;
            assert_eq!(waiting[0], ("fast gate of web".to_string(), Value::Null), "{waiting:?}");
            let why = "behind `fast gate of web`, a landing's gate";
            assert_eq!(waiting[1], (line.clone(), Value::from(why)), "{waiting:?}");
        };
        tokio::select! { _ = &mut first => panic!("it ran beside a busy host"), () = lands => {} }
    })
    .await;
    // Still no room beside what runs: with nothing running, only the first of the line starts.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let landed = async {
        let t = w.tick().await;
        assert!(t.landed.iter().any(|l| l["landed"] == "web#2"), "{:?}", t.landed);
    };
    let ran = async {
        let polls = async {
            loop {
                tokio::time::sleep(LOOK).await;
                w.clock.advance(POLL);
            }
        };
        tokio::select! { r = &mut first => r, () = polls => unreachable!() }
    };
    let ((), ran) = tokio::time::timeout(STUCK, async { tokio::join!(landed, ran) }).await.expect("both go");
    assert_eq!(ran.unwrap()["ok"], true, "the landing's gate ran first");
}
