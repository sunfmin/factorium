//! The owner's urgent mark on an issue (docs/adr/0015-an-urgent-issue-halts-the-host.md): its start goes before
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
    let at = Reading { read_at: 3 * MIN, settle: 0, order: ORDER, ..Reading::default() };
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
    // #3 waits for #2. #2's worker holds no start of #1: the load holds it, with the load's why.
    let t = w.tick().await;
    assert!(t.started.is_none(), "#3 waits for #2, #1 for the load: {:?}", t.held);
    assert!(t.held.as_deref().unwrap().starts_with("cpu"), "the load holds it, not the halt: {:?}", t.held);
    let found: Vec<_> = w.findings().await.into_iter().map(|f| f["what"].as_str().unwrap().to_string()).collect();
    assert!(found.iter().any(|f| f.starts_with("urgent web#2 goes first")), "the halt stands: {found:?}");
    // The load allows it: #1 starts during the halt, in the ordinary order.
    w.host.reading.lock().unwrap().cpu_busy = 10;
    assert!(starts(&w, 1).await["urgent"].is_null());
    // `--no-urgent` takes the mark off.
    w.app.issue_mark_urgent("web", 3, false).await.unwrap();
    assert_eq!(w.app.issue_show("web", 3).await.unwrap()["urgent"], false);
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

/// Halted by an urgent issue (docs/adr/0015-an-urgent-issue-halts-the-host.md):
/// no entry is passed over for it (ADR 0015, 2026-11-27). An urgent entry that
/// waits starts next, ahead of a landing's gate and the workers' and
/// programs' entries. With room for it nothing is cut short; with none, a
/// worker's or a program's command not urgent that holds what it waits for
/// is, but not the urgent issue's own, nor a line move. With no urgent
/// entry waiting, the rest start by the ordinary order and room.
#[test]
fn halted_an_urgent_entry_starts_next_and_cuts_short_only_with_no_room_for_it() {
    let gate = Entry {
        ask: Ask {
            id: Uuid::from_u128(2),
            what: "fast gate of web".into(),
            lands: Some(Uuid::from_u128(200)),
            ..Ask::default()
        },
        queued_at: MIN,
        ..Entry::default()
    };
    let running = Entry { started_at: Some(0), ..workers(4, 0, false) };
    let (exclusive, urgent) = (workers(1, 0, false), workers(3, 2 * MIN, true));
    let program =
        Entry { ask: Ask { program: true, worker: None, ..workers(7, 0, false).ask }, ..workers(7, 0, false) };
    // Its worker's command, queued before its issue was marked.
    let own = Entry { started_at: Some(0), ..workers(5, 0, false) };
    let cut = Entry {
        ask: Ask { id: Uuid::from_u128(8), what: "cut of api".into(), ..Ask::default() },
        started_at: Some(0),
        ..Entry::default()
    };
    let entries = vec![
        exclusive.clone(),
        gate.clone(),
        urgent.clone(),
        running.clone(),
        own.clone(),
        program.clone(),
        cut.clone(),
    ];
    let (halt, halt_for) = (vec!["web#3".into()], vec![own.ask.worker.unwrap()]);
    let q = Queue { entries, halt, halt_for, ..Queue::default() };
    let now = 300 * MIN;
    let at = Reading { read_at: now, settle: 0, order: ORDER, ..Reading::default() };
    let (room, none): (Result<(), String>, _) = (Ok(()), Err("cpu: 95% busy".to_string()));
    // The urgent entry starts next: the landing's gate and the rest wait behind it.
    assert_eq!(q.may_start(urgent.ask.id, room.clone(), at, now), Ok(()));
    for e in [&gate, &exclusive, &program] {
        assert_eq!(q.may_start(e.ask.id, room.clone(), at, now).unwrap_err(), "waits behind 3");
    }
    let whys: Vec<_> = q.resources(now, ORDER)[0].waiting.iter().map(|h| h.why.clone()).collect();
    let behind = Some("behind `3`, an urgent issue's".to_string());
    assert_eq!(whys, [None, behind.clone(), behind.clone(), behind], "behind urgent only as the next ahead");
    let ends = |q: &Queue, e: &Entry, room: &Result<(), String>| q.ends(e.ask.id, now, i64::MAX, false, room.clone());
    assert_eq!(ends(&q, &running, &room), None, "room for the urgent entry: it starts, nothing is cut short");
    assert_eq!(ends(&q, &running, &none), Some(Ends::Halted), "no room for it: cut short for it");
    assert_eq!(ends(&q, &cut, &none), None, "a line move is never cut short by the halt");
    assert_eq!(ends(&q, &own, &none), None, "the urgent issue's own runs on");
    // A device the urgent entry does not ask for runs on; one it waits for, all slots held, is cut short.
    let device = Entry { started_at: Some(0), ..with_resources(workers(6, 0, false), &["FMGG"]) };
    let q2 = Queue { entries: [q.entries.clone(), vec![device.clone()]].concat(), ..q.clone() };
    assert_eq!(ends(&q2, &device, &none), None, "the urgent entry asks for the host alone");
    let wants = with_resources(workers(9, MIN, true), &["FMGG"]);
    let q3 = Queue { entries: vec![device.clone(), wants], ..q.clone() };
    assert_eq!(ends(&q3, &device, &room), Some(Ends::Halted), "its slot is full for the urgent entry");
    // No urgent entry waits: nothing is cut, and the rest start by the ordinary order and room.
    let quiet =
        Queue { entries: q.entries.iter().filter(|e| e.ask.id != urgent.ask.id).cloned().collect(), ..q.clone() };
    assert_eq!(ends(&quiet, &running, &none), None, "no urgent entry waits: it runs on");
    assert_eq!(quiet.may_start(gate.ask.id, room.clone(), at, now), Ok(()), "no urgent entry waits: the gate starts");
    let idle = Queue { entries: quiet.entries.iter().filter(|e| e.ask.id != gate.ask.id).cloned().collect(), ..quiet };
    assert_eq!(
        idle.may_start(exclusive.ask.id, room.clone(), at, now),
        Ok(()),
        "a command not urgent starts with room"
    );
    assert!(idle.may_start(exclusive.ask.id, none.clone(), at, now).is_err(), "and waits for room as ever");
    let q = Queue { halt: Vec::new(), halt_for: Vec::new(), ..q };
    assert_eq!(ends(&q, &running, &none), None, "lifted, nothing is cut");
    assert_eq!(ids(queue::line(&q.entries, now, ORDER)), [3, 2, 1, 7], "lifted: as before");
}

/// Halted by issue 2 of the fixture checkout app, urgent: a ready issue of
/// its own checkout starts, another checkout's done worker lands, and a
/// third checkout's landing is promoted and cut, all during the halt.
#[tokio::test]
async fn halted_workers_start_and_other_checkouts_land_promote_and_cut() {
    let w = World::new();
    for c in ["app", "api", "web"] {
        w.checkout(c).await;
    }
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    for (c, title) in [("app", "a"), ("app", "b"), ("api", "a"), ("web", "a")] {
        w.app.issue_add(c, title, "", false, &[]).await.unwrap();
    }
    for _ in 0..4 {
        w.tick().await;
    }
    w.app.issue_mark_urgent("app", 2, true).await.unwrap();
    w.app.issue_add("app", "c", "", false, &[]).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.started.as_ref().map(|s| s["issue"].clone()), Some("app#3".into()), "{:?}", t.held);
    let found: Vec<_> = w.findings().await.into_iter().map(|f| f["what"].as_str().unwrap().to_string()).collect();
    let halt = found.iter().find(|f| f.starts_with("urgent app#2 goes first")).unwrap_or_else(|| panic!("{found:?}"));
    assert!(halt.contains("workers start as admission allows"), "{halt}");
    assert!(halt.contains("landings, promotions and cuts go on"), "{halt}");
    w.lands("api", 1, "a").await;
    let cut_tick = w.lands("web", 1, "a").await.tick;
    assert!(cut_tick.lines.iter().any(|l| l["promoted"] == "web"), "promoted during the halt: {:?}", cut_tick.lines);
    let cut = cut_tick
        .lines
        .iter()
        .find(|l| l["cut"] == "web")
        .unwrap_or_else(|| panic!("cut during the halt: {:?}", cut_tick.lines));
    assert_eq!(cut["failed"], Value::Null, "{cut}");
    // Issue 1 of the fixture checkout app, the urgent issue's own checkout, lands too.
    w.lands("app", 1, "a").await;
}

/// Halted by web#1, urgent: api#1, ready, starts on the next tick admission
/// allows, and its worker's heavy command runs at once on a host with room,
/// no urgent command waiting (ADR 0015, 2026-11-27).
#[tokio::test]
async fn halted_another_checkout_s_worker_starts_and_its_heavy_command_runs_while_no_urgent_one_waits() {
    let w = World::new();
    for c in ["web", "api"] {
        w.checkout(c).await;
    }
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_mark_urgent("web", 1, true).await.unwrap();
    w.app.issue_add("api", "a", "", false, &[]).await.unwrap();
    assert_eq!(w.tick().await.started.unwrap()["issue"], "web#1", "the urgent one first");
    let t = w.tick().await;
    assert_eq!(t.started.as_ref().map(|s| s["issue"].clone()), Some("api#1".into()), "{:?}", t.held);
    let found: Vec<_> = w.findings().await.into_iter().map(|f| f["what"].as_str().unwrap().to_string()).collect();
    assert!(found.iter().any(|f| f.starts_with("urgent web#1 goes first")), "the halt stands: {found:?}");
    let api = id(&w.worker_on("api", 1).await);
    let ran = tokio::time::timeout(STUCK, w.run().worker_exclusive(Some(api), &w.dir, "echo next")).await;
    assert_eq!(ran.expect("it runs during the halt").unwrap()["ok"], true);
}

/// Halted by web#2, urgent: #1's command that runs is not cut short while
/// no urgent command waits; once #2's gate waits on a host with no room
/// for it, #1's command is cut short and the gate starts.
#[tokio::test]
async fn halted_a_running_command_is_cut_short_only_once_the_urgent_gate_waits_with_no_room() {
    let w = World::new();
    w.two_at_work().await;
    let (one, two) = (id(&w.worker_on("web", 1).await), w.worker_on("web", 2).await);
    let dir = w.dir.join("running");
    let line = w.holds(&dir);
    let a = w.run();
    let mut running = std::pin::pin!(a.worker_exclusive(Some(one), &w.dir, &line));
    let halted = async {
        crate::fixture::until_file(&dir.join("started")).await;
        w.app.issue_mark_urgent("web", 2, true).await.unwrap();
        w.tick_alone().await;
        // A few polls on, with no urgent command waiting: it runs on.
        for _ in 0..3 {
            w.clock.advance(POLL);
            tokio::time::sleep(LOOK).await;
        }
    };
    tokio::select! { _ = &mut running => panic!("cut short with no urgent command waiting"), () = halted => {} }
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let polls = async {
        loop {
            tokio::time::sleep(LOOK).await;
            w.clock.advance(POLL);
        }
    };
    let cli = w.run();
    let both = async { tokio::join!(&mut running, cli.worker_gate(id(&two))) };
    let (ran, gated) = tokio::time::timeout(STUCK, async {
        tokio::select! { r = both => r, () = polls => unreachable!() }
    })
    .await
    .expect("the running one ends and the gate runs");
    let ran = ran.unwrap();
    assert!(ran["ok"] == false && ran.to_string().contains("an urgent issue's command waits with no room"), "{ran}");
    assert!(!gated.unwrap().is_null(), "the urgent gate ran");
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
async fn admission_held_by_the_load_during_a_halt_is_the_load_s_finding_and_a_full_disk_is_its_own() {
    let w = World::new();
    w.two_at_work().await;
    w.app.issue_mark_urgent("web", 2, true).await.unwrap();
    w.app.issue_add("web", "c", "", false, &[]).await.unwrap();
    let found = async || -> Vec<String> {
        w.findings().await.into_iter().map(|f| f["what"].as_str().unwrap().to_string()).collect()
    };
    w.host.reading.lock().unwrap().cpu_busy = 95;
    assert!(w.tick().await.held.unwrap().starts_with("cpu"), "the load holds it, not the halt");
    w.advance(31);
    w.tick().await;
    let f = found().await;
    assert!(f.iter().any(|f| f.starts_with("urgent web#2 goes first")), "{f:?}");
    assert!(f.iter().any(|f| f.contains("admission held") && f.contains("cpu")), "the load's own finding: {f:?}");
    w.host.reading.lock().unwrap().cpu_busy = 10;
    starts(&w, 3).await;

    // An urgent issue held by a full disk: the disk holds it, not the halt.
    w.app.issue_add("web", "d", "", false, &[]).await.unwrap();
    w.app.issue_mark_urgent("web", 4, true).await.unwrap();
    w.host.reading.lock().unwrap().disk_free = 5 * crate::fixture::GIB;
    assert!(w.tick().await.held.unwrap().starts_with("disk"));
    w.advance(31);
    w.tick().await;
    let f = found().await;
    assert!(f.iter().any(|f| f.starts_with("urgent web#2 goes first")), "{f:?}");
    assert!(f.iter().any(|f| f.contains("admission held") && f.contains("disk")), "{f:?}");
}

/// The hold bound holds for an urgent issue's command too (ADR 0010, ADR
/// 0015, 2026-11-27): running past it while an entry not urgent waits for
/// the host, with room or none, it is ended `Held`, whether marked when it
/// asked or its worker's issue halts the queue now. With no one waiting, or
/// only entries that hold up nothing (one under replaced rules), it runs on.
#[test]
fn an_urgent_command_holding_the_host_past_the_hold_bound_while_one_waits_is_ended_held() {
    let bound = 30 * MIN;
    let marked = Entry { started_at: Some(0), ..workers(1, 0, true) };
    let halting = Entry { started_at: Some(0), ..workers(2, 0, false) };
    let waiter = workers(3, MIN, false);
    let (room, none): (Result<(), String>, _) = (Ok(()), Err("cpu: 95% busy".to_string()));
    for running in [&marked, &halting] {
        let (halt, halt_for) = (vec!["web#3".into()], vec![halting.ask.worker.unwrap()]);
        let q = Queue { entries: vec![running.clone(), waiter.clone()], halt, halt_for, ..Queue::default() };
        assert!(q.urgent(running), "it is the urgent issue's");
        let ends =
            |q: &Queue, now: i64, room: &Result<(), String>| q.ends(running.ask.id, now, bound, false, room.clone());
        for r in [&room, &none] {
            assert_eq!(ends(&q, MIN + bound, r), None, "not yet past the bound since the waiter queued");
            assert_eq!(ends(&q, MIN + bound + 1, r), Some(Ends::Held), "past it, room {r:?}");
        }
        let alone = Queue { entries: vec![running.clone()], ..q.clone() };
        assert_eq!(ends(&alone, 10 * bound, &none), None, "no one waits: it runs on");
        let stale = Entry { ask: Ask { rules: "0".into(), ..waiter.ask.clone() }, ..waiter.clone() };
        let replaced = Queue { entries: vec![running.clone(), stale], rules: "1".into(), ..q.clone() };
        assert_eq!(ends(&replaced, 10 * bound, &none), None, "one under replaced rules holds up nothing");
    }
}

/// Halted by web#2, urgent: its worker's command holds the host, with no
/// room beside it, while #1's command waits. Past the hold bound it is
/// ended as any held command, saying so in its tail, and #1's starts.
#[tokio::test]
async fn an_urgent_worker_s_command_holding_others_up_past_the_hold_bound_is_ended_and_the_waiter_runs() {
    let w = World::new();
    w.two_at_work().await;
    let (one, two) = (id(&w.worker_on("web", 1).await), id(&w.worker_on("web", 2).await));
    w.app.issue_mark_urgent("web", 2, true).await.unwrap();
    w.tick_alone().await;
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let dir = w.dir.join("urgent");
    let line = w.holds(&dir);
    let (a, b) = (w.run(), w.run());
    let others = async {
        crate::fixture::until_file(&dir.join("started")).await;
        let mut waiter = std::pin::pin!(b.worker_exclusive(Some(one), &w.dir, "echo after"));
        let queued = w.until_queued("echo after");
        tokio::select! { _ = &mut waiter => panic!("it ran beside the urgent one"), () = queued => {} }
        w.advance(31);
        let again = async {
            w.gone(&line).await;
            w.poll(1).await;
        };
        tokio::join!(waiter, again).0
    };
    let both = async { tokio::join!(a.worker_exclusive(Some(two), &dir, &line), others) };
    let (ran, after) = tokio::time::timeout(STUCK, both).await.expect("the urgent command is ended");
    let ran = ran.unwrap();
    assert_eq!(ran["ok"], false, "{ran}");
    assert!(ran["tail"].as_str().unwrap().contains("ended: it held others up past the hold bound of 1800s"), "{ran}");
    assert_eq!(after.unwrap()["ok"], true, "the waiter runs once the urgent command is ended");
}
