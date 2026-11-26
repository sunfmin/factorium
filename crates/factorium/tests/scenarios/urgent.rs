//! The owner's urgent mark on an issue (SPEC §1.3): its start goes before
//! every other ready issue, held only by the memory and disk floors; its
//! worker's heavy commands go ahead of those not urgent, behind a line move
//! past its bound; its landing takes the next turn of its checkout's line,
//! never cutting a running gate short. Blocks still hold it.

use factorium_domain::admission::{self, Reading as Read};
use factorium_domain::heavy::{Ask, Entry, Heavy, HeavyEvent, Queue, Reading};
use factorium_domain::neutral::queue::{self, Order};
use factorium_domain::settings::Settings;
use factum::Aggregate;
use factum::Uuid;
use serde_json::Value;

use crate::fixture::{LOOK, POLL, STUCK, World, id};

const MIN: i64 = 60_000;

/// The rows of a list the CLI answered.
fn rows(list: &Value) -> &[Value] {
    list.as_array().map_or(&[], Vec::as_slice)
}
const ORDER: Order = Order { starve: 240 * MIN, default: 30 * MIN };

/// A worker's waiting entry `n`, queued at `queued_at`.
fn workers(n: u128, queued_at: i64, urgent: bool) -> Entry {
    let ask = Ask {
        id: Uuid::from_u128(n),
        what: n.to_string(),
        worker: Some(Uuid::from_u128(100 + n)),
        urgent,
        ..Ask::default()
    };
    Entry { ask, queued_at, ..Entry::default() }
}

fn ids(line: Vec<&Entry>) -> Vec<u128> {
    line.iter().map(|e| e.ask.id.as_u128()).collect()
}

#[test]
fn on_a_host_held_at_the_queue_wait_bound_only_an_urgent_start_goes_and_only_the_floors_hold_it() {
    let s = Settings::default();
    let q = Queue { entries: vec![workers(1, 0, false)], ..Queue::default() };
    let now = 301_000;
    let room = Read { cpu_busy: 95, memory_free: 60, disk_free: 500 << 30, workers_cpu: 0 };
    let held = admission::admit(&room, &q, now, &s).unwrap_err();
    assert!(held.starts_with("cpu"), "{held}");
    let why = admission::holds(&room, &q, now, &s);
    assert!(why.iter().any(|w| w.starts_with("queue: `1` has waited 301s")), "{why:?}");
    assert_eq!(admission::admit_urgent(&room, &s), Ok(()), "the load holds no urgent start");
    let low = Read { memory_free: 5, ..room };
    assert!(admission::admit_urgent(&low, &s).unwrap_err().starts_with("memory"));
    assert_eq!(admission::admit_urgent(&Read { memory_free: 15, ..room }, &s), Ok(()), "at the floor, it starts");
    let full = Read { disk_free: 1 << 30, ..room };
    assert!(admission::admit_urgent(&full, &s).unwrap_err().starts_with("disk"));
}

#[test]
fn an_urgent_entry_goes_ahead_of_workers_that_waited_longer_behind_a_line_move_past_its_bound() {
    let (a, b, u) = (workers(1, 0, false), workers(2, MIN, false), workers(3, 2 * MIN, true));
    let q = Queue { entries: vec![a.clone(), b.clone(), u.clone()], ..Queue::default() };
    assert_eq!(ids(queue::line(&q.entries, 3 * MIN, ORDER)), [3, 1, 2]);
    let at = Reading { read_at: 3 * MIN, settle: 0, order: ORDER };
    assert_eq!(q.may_start(u.ask.id, Ok(()), at, 3 * MIN), Ok(()));
    assert_eq!(q.may_start(a.ask.id, Ok(()), at, 3 * MIN).unwrap_err(), "waits behind 3");
    let host = &q.resources(3 * MIN, ORDER)[0];
    assert_eq!(host.waiting[1].why.as_deref(), Some("behind `3`, an urgent issue's"));
    assert!(host.waiting[0].urgent && !host.waiting[1].urgent);
    // A line move past its bound still goes first; among urgent ones, the ordinary order.
    let mut late = Entry {
        ask: Ask { id: Uuid::from_u128(4), what: "4".into(), ..Ask::default() },
        queued_at: 3 * MIN,
        ..Entry::default()
    };
    late.ask.bound_at = Some(0);
    let v = workers(5, 3 * MIN, true);
    let q = Queue { entries: vec![a, b, u, late, v], ..Queue::default() };
    assert_eq!(ids(queue::line(&q.entries, 4 * MIN, ORDER)), [4, 3, 5, 1, 2]);
}

#[test]
fn an_urgent_entry_goes_ahead_of_no_line_job_that_does_not_yield_and_a_mark_moves_only_its_own() {
    let gate = Entry {
        ask: Ask { id: Uuid::from_u128(1), what: "fast gate of web".into(), ..Ask::default() },
        ..Entry::default()
    };
    let u = workers(2, MIN, true);
    let mut q = Queue { entries: vec![gate, u, workers(3, 0, false)], ..Queue::default() };
    assert_eq!(ids(queue::line(&q.entries, 2 * MIN, ORDER)), [1, 2, 3], "a line's gate queued first stays first");
    // Its issue's mark taken away while it waits: that entry alone goes back to the ordinary order.
    Heavy::evolve(&mut q, HeavyEvent::HeavyMarked { id: Uuid::from_u128(2), urgent: false });
    assert_eq!(q.entries.iter().map(|e| e.ask.urgent).collect::<Vec<_>>(), [false, false, false]);
    Heavy::evolve(&mut q, HeavyEvent::HeavyMarked { id: Uuid::from_u128(3), urgent: true });
    assert_eq!(q.entries.iter().map(|e| e.ask.urgent).collect::<Vec<_>>(), [false, false, true]);
}

/// The tick's start of `web`#`n`, as it answered it.
async fn starts(w: &World, n: u64) -> Value {
    let t = w.tick().await;
    let started = t.started.unwrap_or_else(|| panic!("web#{n} starts: {:?}", t.held));
    assert_eq!(started["issue"], format!("web#{n}"), "{started}");
    started
}

#[tokio::test]
async fn an_urgent_ready_issue_starts_first_past_the_load_and_a_blocked_one_waits_for_its_block() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    for title in ["a", "b", "c"] {
        w.app.issue_add("web", title, "", false, &[]).await.unwrap();
    }
    // #3 is urgent, but waits for #2.
    w.app.issue_block("web", 3, 2, Some("it needs b's API"), false).await.unwrap();
    assert_eq!(w.app.issue_mark_urgent("web", 3, true).await.unwrap()["urgent"], true);
    assert_eq!(rows(&w.app.issue_list(Some("web"), false).await.unwrap())[2]["urgent"], true);
    w.app.issue_mark_urgent("web", 2, true).await.unwrap();
    w.host.reading.lock().unwrap().cpu_busy = 95;
    let started = starts(&w, 2).await;
    assert_eq!(started["urgent"]["ahead_of"], serde_json::json!(["web#1"]), "{started}");
    assert!(started["urgent"]["past"][0].as_str().unwrap().starts_with("cpu"), "{started}");
    let t = w.tick().await;
    assert!(t.started.is_none(), "#3 waits for #2, #1 for the load: {:?}", t.held);
    assert!(t.held.unwrap().starts_with("cpu"));
    // `--no-urgent` puts it back in the ordinary order: #1 goes first once the load allows.
    w.app.issue_mark_urgent("web", 3, false).await.unwrap();
    assert_eq!(w.app.issue_show("web", 3).await.unwrap()["urgent"], false);
    w.host.reading.lock().unwrap().cpu_busy = 10;
    assert!(starts(&w, 1).await["urgent"].is_null());
}

#[tokio::test]
async fn an_urgent_landing_lands_next_after_the_gate_already_running_before_the_others() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    for title in ["a", "b", "c", "d"] {
        w.app.issue_add("web", title, "", false, &[]).await.unwrap();
    }
    for _ in 0..4 {
        w.tick().await;
    }
    for n in 1..=3 {
        w.done(&w.worker_on("web", n).await, &format!("f{n}"), "x").await;
        w.advance(1);
    }
    // #1's gate starts beside the ticks; then #4, urgent, asks to land.
    let t = w.tick_alone().await;
    assert!(t.landed.is_empty() && w.app.moves_run(), "#1's gate runs beside the ticks: {t:?}");
    w.app.issue_mark_urgent("web", 4, true).await.unwrap();
    let four = w.worker_on("web", 4).await;
    assert_eq!(four["urgent"], true, "{four}");
    w.done(&four, "f4", "x").await;
    let landing = w.doctor().await["landing"].clone();
    let line: Vec<_> =
        rows(&landing).iter().map(|l| (l["issue"].clone(), l["place"].clone(), l["urgent"].clone())).collect();
    assert_eq!(line[0], (Value::from("web#4"), Value::from(1), Value::from(true)), "{landing}");
    tokio::time::timeout(STUCK, w.app.moves_ended()).await.expect("#1's gate ends");
    let mut order = Vec::new();
    let mut went = Value::Null;
    while order.len() < 4 {
        let t = w.tick().await;
        for l in &t.landed {
            if let Some(n) = l["landed"].as_str() {
                order.push(n.to_string());
            }
            if !l["urgent"].is_null() {
                went = l["urgent"].clone();
            }
        }
    }
    assert_eq!(order, ["web#1", "web#4", "web#2", "web#3"], "never cut short, then next");
    assert_eq!(went["ahead_of"], serde_json::json!(["web#2", "web#3"]), "{went}");
}

#[tokio::test]
async fn an_urgent_worker_s_gate_goes_ahead_of_an_entry_not_urgent_that_waited_longer() {
    let w = World::new();
    w.two_at_work().await;
    w.app.issue_mark_urgent("web", 2, true).await.unwrap();
    let (one, two) = (id(&w.worker_on("web", 1).await), id(&w.worker_on("web", 2).await));
    let (a, b) = (w.run(), w.run());
    let mut first = std::pin::pin!(a.worker_exclusive(Some(one), &w.dir, "echo first"));
    let mut gate = std::pin::pin!(b.worker_gate(two));
    w.while_busy(async {
        tokio::select! { _ = &mut first => panic!("it ran beside a busy host"), () = w.until_queued("echo first") => {} }
        w.clock.advance(1_000);
        let waits = async {
            w.until(async || {
                let r = w.doctor().await["resources"].clone();
                let host = rows(&r).iter().find(|r| r["name"] == "host").cloned().unwrap();
                rows(&host["waiting"]).len() == 2
            })
            .await;
            let r = w.doctor().await;
            let host = rows(&r["resources"]).iter().find(|r| r["name"] == "host").cloned().unwrap();
            let waiting = rows(&host["waiting"]);
            assert_eq!((waiting[0]["worker"].as_str(), waiting[0]["urgent"].as_bool()), (Some(&*two.to_string()), Some(true)), "{host}");
            assert!(waiting[1]["why"].as_str().unwrap().ends_with("an urgent issue's"), "{host}");
            let heavy = rows(&r["heavy"]).iter().find(|e| e["worker"] == two.to_string()).cloned().unwrap();
            assert_eq!(heavy["urgent"], true, "{heavy}");
        };
        tokio::select! {
            _ = &mut first => panic!("it ran beside a busy host"),
            _ = &mut gate => panic!("the gate ran beside a busy host"),
            () = waits => {}
        }
    })
    .await;
    // Each waiter asks again a poll on, the clock moved for them.
    let both = async { tokio::join!(first, gate) };
    let polls = async {
        loop {
            tokio::time::sleep(LOOK).await;
            w.clock.advance(POLL);
        }
    };
    let ran = async {
        tokio::select! { r = both => r, () = polls => unreachable!() }
    };
    let (ran, gated) = tokio::time::timeout(STUCK, ran).await.expect("both run");
    assert_eq!(ran.unwrap()["ok"], true);
    assert!(gated.is_ok(), "{gated:?}");
}
