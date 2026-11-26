//! The owner's urgent mark on an issue (docs/adr/0014-an-urgent-issue-halts-the-host.md): its start goes before
//! every other ready issue, held only by the memory and disk floors; its
//! worker's heavy commands go ahead of those not urgent, behind a line move
//! past its bound; its landing takes the next turn of its checkout's line,
//! never cutting a running gate short. Blocks still hold it.

use factorium_domain::admission::{self, Reading as Read};
use factorium_domain::heavy::{Ask, Ends, Entry, Heavy, HeavyEvent, Queue, Reading};
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

fn with_resources(mut e: Entry, names: &[&str]) -> Entry {
    e.ask.resources = names.iter().map(|n| n.to_string()).collect();
    e
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
    let whys: Vec<_> =
        q.resources(4 * MIN, ORDER)[0].waiting.iter().map(|h| h.why.clone().unwrap_or_default()).collect();
    assert_eq!(whys[2], "behind `4`, a line move past its bound", "{whys:?}");
    let urgent_only =
        Queue { entries: q.entries.iter().filter(|e| e.ask.urgent).cloned().collect(), ..Queue::default() };
    let whys: Vec<_> = urgent_only.resources(4 * MIN, ORDER)[0].waiting.iter().map(|h| h.why.clone()).collect();
    assert_eq!(whys, [None, Some("behind `3`, queued before it".into())], "among urgent ones, the ordinary why");
}

#[test]
fn an_urgent_entry_goes_ahead_of_no_line_job_that_does_not_yield_and_a_mark_moves_only_its_own() {
    let gate = Entry {
        ask: Ask { id: Uuid::from_u128(1), what: "fast gate of web".into(), ..Ask::default() },
        ..Entry::default()
    };
    let u = workers(2, MIN, true);
    let mut q = Queue { entries: vec![gate, u, workers(3, MIN / 2, false)], ..Queue::default() };
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
    assert!(t.started.is_none(), "#3 waits for #2, #1 for the halt #2's worker begins: {:?}", t.held);
    assert_eq!(t.held.as_deref(), Some("halt: behind urgent web#2"));
    // `--no-urgent` puts each back in the ordinary order: #1 goes first once the load allows.
    w.app.issue_mark_urgent("web", 3, false).await.unwrap();
    w.app.issue_mark_urgent("web", 2, false).await.unwrap();
    assert_eq!(w.app.issue_show("web", 3).await.unwrap()["urgent"], false);
    assert!(w.tick().await.held.unwrap().starts_with("cpu"), "the halt lifted, the load holds it");
    w.host.reading.lock().unwrap().cpu_busy = 10;
    assert!(starts(&w, 1).await["urgent"].is_null());
}

#[tokio::test]
async fn an_urgent_landing_lands_next_after_the_gate_already_running_before_the_others() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    // #5 keeps working all along, ahead of the rest in the line of workers: never one that waits to land.
    for title in ["a", "b", "c", "d", "e"] {
        w.app.issue_add("web", title, "", false, &[]).await.unwrap();
    }
    for _ in 0..5 {
        w.tick().await;
    }
    for n in 1..=3 {
        w.done(&w.worker_on("web", n).await, &format!("f{n}"), "x").await;
        w.advance(1);
    }
    // #1's gate starts beside the ticks and ends green; then #4, urgent, asks to land.
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
    assert_eq!(order, ["web#1", "web#4", "web#2", "web#3"], "a gate that ended keeps its green, then the urgent");
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

/// Halted by an urgent issue (docs/adr/0014-an-urgent-issue-halts-the-host.md): what is not urgent waits, past
/// the starve bound and a line move's bound too; a command not urgent that
/// runs is cut short once an urgent entry waits for what it holds, the
/// urgent issue's worker's own is not, however it was queued; lifted, the
/// rest start in their old order.
#[test]
fn halted_nothing_but_an_urgent_entry_starts_and_what_runs_not_urgent_is_cut_short() {
    let mut gate = Entry {
        ask: Ask { id: Uuid::from_u128(2), what: "fast gate of web".into(), ..Ask::default() },
        queued_at: MIN,
        ..Entry::default()
    };
    gate.ask.bound_at = Some(0);
    let running = Entry { started_at: Some(0), ..workers(4, 0, false) };
    let (exclusive, urgent) = (workers(1, 0, false), workers(3, 2 * MIN, true));
    // Its worker's command, queued before its issue was marked.
    let own = Entry { started_at: Some(0), ..workers(5, 0, false) };
    let entries = vec![exclusive.clone(), gate.clone(), urgent.clone(), running.clone(), own.clone()];
    let (halt, halt_for) = (vec!["web#3".into()], vec![own.ask.worker.unwrap()]);
    let q = Queue { entries, halt, halt_for, ..Queue::default() };
    let now = 300 * MIN;
    let at = Reading { read_at: now, settle: 0, order: ORDER };
    assert_eq!(q.may_start(urgent.ask.id, Ok(()), at, now), Ok(()));
    for e in [&exclusive, &gate] {
        assert_eq!(q.may_start(e.ask.id, Ok(()), at, now).unwrap_err(), "behind urgent web#3");
    }
    let whys: Vec<_> = q.resources(now, ORDER)[0].waiting.iter().map(|h| h.why.clone()).collect();
    assert_eq!(whys[1..], [Some("behind urgent web#3".to_string()), Some("behind urgent web#3".to_string())]);
    assert_eq!(q.ends(running.ask.id, now, i64::MAX, false), Some(Ends::Halted), "cut short for it");
    // Nothing urgent waiting for what it holds, it runs on: a device for the host, or no urgent ask.
    let device = Entry { started_at: Some(0), ..with_resources(workers(6, 0, false), &["FMGG"]) };
    let q2 = Queue { entries: [q.entries.clone(), vec![device.clone()]].concat(), ..q.clone() };
    assert_eq!(q2.ends(device.ask.id, now, i64::MAX, false), None, "the urgent entry asks for the host alone");
    let quiet =
        Queue { entries: q.entries.iter().filter(|e| e.ask.id != urgent.ask.id).cloned().collect(), ..q.clone() };
    assert_eq!(quiet.ends(running.ask.id, now, i64::MAX, false), None, "no urgent entry waits: it runs on");
    assert_eq!(q.ends(own.ask.id, now, i64::MAX, false), None, "the urgent issue's own runs on");
    let q = Queue { halt: Vec::new(), halt_for: Vec::new(), ..q };
    assert_eq!(q.ends(running.ask.id, now, i64::MAX, false), None, "lifted, nothing is cut");
    assert_eq!(ids(queue::line(&q.entries, now, ORDER)), [2, 3, 1], "lifted: the move past its bound, then as before");
}

#[tokio::test]
async fn halted_a_ready_issue_and_a_promotion_owed_by_its_bound_wait_until_the_urgent_one_lands() {
    let w = World::new();
    w.two_at_work().await;
    w.lands("web", 1, "a").await;
    w.app.issue_mark_urgent("web", 2, true).await.unwrap();
    w.app.issue_add("web", "c", "", false, &[]).await.unwrap();
    w.advance(61);
    let t = w.tick().await;
    assert!(t.started.is_none() && t.held.as_deref() == Some("halt: behind urgent web#2"), "{:?}", t.held);
    assert!(!t.lines.iter().any(|l| l["promoted"] == "web"), "the promotion waits: {:?}", t.lines);
    let found: Vec<_> = w.findings().await.into_iter().map(|f| f["what"].as_str().unwrap().to_string()).collect();
    assert!(found.iter().any(|f| f.starts_with("urgent web#2 halts")), "{found:?}");
    assert!(!found.iter().any(|f| f.contains("not promoted")), "the halt is no stall: {found:?}");
    w.lands("web", 2, "b").await;
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "lifted, it promotes: {:?}", t.lines);
    assert_eq!(t.started.expect("#3 starts")["issue"], "web#3");
}

#[tokio::test]
async fn halted_a_worker_s_command_waits_behind_the_urgent_gate_and_one_running_is_cut_short() {
    let w = World::new();
    w.two_at_work().await;
    let (one, two) = (id(&w.worker_on("web", 1).await), w.worker_on("web", 2).await);
    let dir = w.dir.join("running");
    let line = w.holds(&dir);
    let (a, b) = (w.run(), w.run());
    let mut running = std::pin::pin!(a.worker_exclusive(Some(one), &w.dir, &line));
    let mut next = std::pin::pin!(b.worker_exclusive(Some(one), &w.dir, "echo next"));
    let body = async {
        crate::fixture::until_file(&dir.join("started")).await;
        // The halt begins while #1's command runs: it is cut short.
        w.app.issue_mark_urgent("web", 2, true).await.unwrap();
        w.tick_alone().await;
        // Asked once the halt began, #1's next command waits behind it.
        let halted = async {
            let host = |d: &Value| rows(&d["resources"]).iter().find(|r| r["name"] == "host").cloned().unwrap();
            w.until(async || rows(&host(&w.doctor().await)["waiting"]).len() == 1).await;
            let waiting = host(&w.doctor().await)["waiting"][0].clone();
            assert_eq!(waiting["why"], "behind urgent web#2", "{waiting}");
            let polls = async {
                loop {
                    tokio::time::sleep(LOOK).await;
                    w.clock.advance(POLL);
                }
            };
            let cli = w.run();
            let gate = cli.worker_gate(id(&two));
            let gated = tokio::select! { g = gate => g.unwrap(), () = polls => unreachable!() };
            assert!(!gated.is_null(), "the urgent gate ran beside the halt");
        };
        tokio::select! { _ = &mut next => panic!("it started during the halt"), () = halted => {} }
    };
    let (ran, ()) =
        tokio::time::timeout(STUCK, async { tokio::join!(&mut running, body) }).await.expect("the running one ends");
    let ran = ran.unwrap();
    assert!(ran["ok"] == false && ran.to_string().contains("an urgent issue halts the host"), "{ran}");
    w.lands("web", 2, "b").await;
    w.tick().await;
    let polls = async {
        loop {
            tokio::time::sleep(LOOK).await;
            w.clock.advance(POLL);
        }
    };
    let ran = tokio::time::timeout(STUCK, async {
        tokio::select! { r = next => r, () = polls => unreachable!() }
    });
    assert_eq!(ran.await.expect("lifted, it runs").unwrap()["ok"], true);
}

#[tokio::test]
async fn an_urgent_start_goes_ahead_of_no_ready_issue_whose_block_is_unmet() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    for title in ["a", "b", "c"] {
        w.app.issue_add("web", title, "", false, &[]).await.unwrap();
    }
    w.app.issue_block("web", 1, 3, Some("it needs c's API"), false).await.unwrap();
    w.app.issue_mark_urgent("web", 2, true).await.unwrap();
    let started = starts(&w, 2).await;
    assert_eq!(started["urgent"]["ahead_of"], serde_json::json!([]), "#1 could not go: {started}");
}

#[tokio::test]
async fn halted_a_landing_not_urgent_waits_while_the_urgent_worker_works_and_is_no_stall() {
    let w = World::new();
    w.two_at_work().await;
    w.app.issue_mark_urgent("web", 2, true).await.unwrap();
    w.tick().await;
    w.done(&w.worker_on("web", 1).await, "a", "x").await;
    let t = w.tick().await;
    assert!(!t.landed.iter().any(|l| l["landed"] == "web#1"), "{:?}", t.landed);
    w.advance(11);
    let t = w.tick().await;
    assert!(!t.landed.iter().any(|l| l["landed"] == "web#1"), "still halted: {:?}", t.landed);
    let found: Vec<_> = w.findings().await.into_iter().map(|f| f["what"].as_str().unwrap().to_string()).collect();
    assert!(!found.iter().any(|f| f.contains("to land")), "the halt is no stall: {found:?}");
    w.lands("web", 2, "b").await;
    assert!(w.tick().await.landed.iter().any(|l| l["landed"] == "web#1"), "lifted, #1 lands");
}
