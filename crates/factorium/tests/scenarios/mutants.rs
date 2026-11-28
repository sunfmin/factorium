//! A checkout whose slimfit holds the record of its mutants at the base
//! (`[check] mutants_target`): its landings wait for their fast gate alone,
//! and before a promotion's slow gate one issue picks, runs and answers the
//! mutants of everything landed since the base.

use serde_json::{Value, json};

use crate::fixture::{CLAUDE, World, id};

/// slimfit.toml holding the record at the base.
const HELD: &str = "[[suite]]\nname = \"rust\"\npreset = \"rust\"\n\n[check]\nmutants_target = \"origin/main\"\n";

/// What `slimfit check --mutants --target` answers of a range from `from`
/// adding `lines` source lines, holding `records`.
fn listing(from: &str, lines: u64, records: &[&str]) -> String {
    let landing = json!({ "target": "origin/main", "from": from, "lines": lines, "records": records });
    json!({ "landing": landing, "candidates": { "rust": [] }, "next": "" }).to_string()
}

/// What the world's brief of `worker` (`web-1-1`) says.
fn brief(w: &World, worker: &str) -> String {
    std::fs::read_to_string(w.data().brief(worker)).unwrap()
}

const NO_MUTANTS: &str = "pick and run no mutants before `done`";

/// `web` at work on #1, whose change holds the record at the base: what slimfit lists of the
/// range owes one. Answers the base it starts from.
async fn held(w: &World) -> String {
    let wk = w.working(CLAUDE, "a").await;
    let main = w.remote_tip("web", "main");
    w.host.slimfit.lock().unwrap().insert("check".into(), listing(&main, 3, &[]));
    w.done(&wk, "slimfit.toml", HELD).await;
    main
}

/// The issue `web`#`n`, as `issue show` answers it.
async fn shown(w: &World, n: u64) -> Value {
    w.app.issue_show("web", n).await.unwrap()
}

#[tokio::test]
async fn a_promotion_files_its_mutants_issue_waits_for_its_landing_then_promotes_a_tip_that_holds_the_record() {
    let w = World::new();
    let main = held(&w).await;
    let t = w.tick().await;
    // The landing waits for its fast gate alone, though what it adds owes a record.
    assert_eq!(t.landed[0]["landed"], "web#1", "{:?}", t.landed);
    let filed = t.lines.iter().find(|l| l["mutants"].is_string()).unwrap_or_else(|| panic!("{:?}", t.lines));
    assert_eq!(filed["mutants"], "web#2");
    let tip = w.remote_tip("web", "proving");
    assert_eq!(filed["range"], format!("{main}..{tip}"));
    assert_eq!(w.remote_tip("web", "main"), main, "no slow gate yet: the promotion waits for its mutants");
    let ran = w.host.ran.lock().unwrap().join("\n");
    assert!(ran.contains("--mutants --target origin/main"), "slimfit was asked of the range: {ran}");
    let issue = shown(&w, 2).await;
    assert!(issue["title"].as_str().unwrap().starts_with("Mutants of "), "{issue}");
    let body = issue["body"].as_str().unwrap();
    assert!(body.contains(&format!("factorium worker pick-mutants --target {main}")), "{body}");
    assert_eq!(issue["task"]["kind"], "promotion-mutants");
    // Its worker took it at once; its brief does not tell it to skip them, nor did #1's, before the setting.
    let wk = w.worker_on("web", 2).await;
    assert!(!brief(&w, "web-2-1").contains(NO_MUTANTS) && !brief(&w, "web-1-1").contains(NO_MUTANTS));
    let doctor = w.app.doctor().await.unwrap();
    let waits = &doctor["mutants"][0];
    assert_eq!((waits["issue"].clone(), waits["left_s"].clone()), (json!("web#2"), json!(1800)), "{doctor}");

    // Idle while its worker waits for the watch, the promotion is due: it waits for its mutants,
    // files no second issue and runs no slow gate.
    w.app.worker_escalate(id(&wk), "which picks?").await.unwrap();
    w.advance(5);
    let t = w.tick().await;
    assert!(t.lines.is_empty(), "{:?}", t.lines);
    assert_eq!(w.remote_tip("web", "main"), main);

    // Its tests and record land on their fast gate; then the slow gate, and the base holds the record.
    w.host.slimfit.lock().unwrap().insert("check".into(), listing(&main, 3, &[".slimfit/mutants/a.json"]));
    w.app.watch_restart(id(&wk), "pick over the range", false).await.unwrap();
    w.done(&wk, "kills.rs", "x").await;
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#2", "{:?}", t.landed);
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    let landed = t.landed[0]["commit"].as_str().unwrap();
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert_eq!(shown(&w, 2).await["status"], "closed");
    assert!(w.app.doctor().await.unwrap()["mutants"].as_array().unwrap().is_empty());

    // Now the checkout holds them at the base: a worker's brief says it runs none before `done`.
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    assert!(brief(&w, "web-3-1").contains(NO_MUTANTS), "{}", brief(&w, "web-3-1"));
}

#[tokio::test]
async fn past_mutants_bound_or_with_no_worker_to_start_it_the_promotion_goes_on_and_the_issue_stays_open() {
    for how in ["past the bound", "no worker can start it"] {
        let w = World::new();
        let main = held(&w).await;
        if how == "no worker can start it" {
            // No account can start one: stablehand walks no order.
            w.host.stablehand.lock().unwrap().order.clear();
        }
        let t = w.tick().await;
        assert!(t.lines.iter().any(|l| l["mutants"] == "web#2"), "{how}: {:?}", t.lines);
        assert_eq!(w.remote_tip("web", "main"), main, "{how}");
        if how == "past the bound" {
            assert!(t.started.is_some(), "its worker started: {t:?}");
            w.advance(29);
            assert!(w.tick().await.lines.is_empty(), "within mutants_bound it waits");
            w.advance(1);
        }
        let t = w.tick().await;
        assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{how}: {:?}", t.lines);
        assert_eq!(w.remote_tip("web", "main"), w.remote_tip("web", "proving"), "{how}");
        let open = if how == "past the bound" { "working" } else { "ready" };
        assert_eq!(shown(&w, 2).await["status"], open, "{how}: it stays open, its worker on it if one started");
        // A later promotion still owing for its range files none while it is open.
        w.stablehand_keeps(&["claude-code"]);
        w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
        // One start a tick; #2 starts ahead of #3, if it did not start yet.
        w.tick().await;
        w.tick().await;
        let wk = w.worker_on("web", 3).await;
        w.done(&wk, "b", "x").await;
        let mut lines = w.tick().await.lines;
        // #2's worker keeps the checkout busy: the promotion falls due by its bound.
        w.advance(31);
        lines.extend(w.tick().await.lines);
        assert!(lines.iter().any(|l| l["promoted"] == "web"), "{how}: {lines:?}");
        assert!(w.app.issue_show("web", 4).await.is_err(), "{how}: no second mutants issue");
    }
}

#[tokio::test]
async fn a_checkout_whose_slimfit_holds_no_record_at_the_base_promotes_as_before_and_asks_slimfit_nothing() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    w.done(&wk, "slimfit.toml", "[[suite]]\nname = \"rust\"\npreset = \"rust\"\n").await;
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "{:?}", t.lines);
    let ran = w.host.ran.lock().unwrap().join("\n");
    assert!(!ran.contains("--mutants"), "{ran}");
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    assert!(!brief(&w, "web-2-1").contains(NO_MUTANTS), "its workers still run theirs before `done`");
}

#[tokio::test]
async fn the_mutants_issue_starts_ahead_of_an_ordinary_one_filed_before_it() {
    let w = World::new();
    held(&w).await;
    w.app.issue_add("web", "ordinary", "", false, &[]).await.unwrap();
    // The CPU busy, admission starts neither while #1 lands and its promotion files #3.
    w.host.reading.lock().unwrap().cpu_busy = 90;
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["mutants"] == "web#3") && t.started.is_none(), "{t:?}");
    w.host.reading.lock().unwrap().cpu_busy = 10;
    let t = w.tick().await;
    assert_eq!(t.started.expect("one start")["issue"], "web#3", "ahead of web#2");
}

/// `web` at its cap of 2 with #2 and #3 live, #4 ready and ordinary, while
/// #1 lands with the CPU busy and its promotion, due by its bound, files
/// the mutants issue #5.
async fn at_its_cap(w: &World) {
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    w.app.settings_set(Some("workers_per_checkout"), Some(3)).await.unwrap();
    let wk = w.working(CLAUDE, "a").await;
    for title in ["b", "c"] {
        w.app.issue_add("web", title, "", false, &[]).await.unwrap();
        w.tick().await;
    }
    w.app.settings_set(Some("workers_per_checkout"), Some(2)).await.unwrap();
    w.app.issue_add("web", "ordinary", "", false, &[]).await.unwrap();
    w.host.slimfit.lock().unwrap().insert("check".into(), listing(&w.remote_tip("web", "main"), 3, &[]));
    w.done(&wk, "slimfit.toml", HELD).await;
    w.host.reading.lock().unwrap().cpu_busy = 90;
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
    // Its workers keep the checkout busy: the promotion falls due by its bound.
    w.advance(60);
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["mutants"] == "web#5") && t.started.is_none(), "{t:?}");
}

#[tokio::test]
async fn on_a_checkout_at_its_cap_the_mutants_issue_starts_and_an_ordinary_one_still_waits() {
    let w = World::new();
    at_its_cap(&w).await;
    let capped = w.doctor().await["capped"].clone();
    assert_eq!(capped, json!([{ "checkout": "web", "live": 2, "cap": 2, "waiting": ["web#4"] }]), "not web#5");
    w.host.reading.lock().unwrap().cpu_busy = 10;
    let t = w.tick().await;
    assert_eq!(t.started.expect("a start")["issue"], "web#5", "the cap does not hold it: {:?}", t.held);
    // Its worker counts as live: #4 waits for one more to end.
    assert_eq!(w.tick().await.started, None);
    assert_eq!(w.doctor().await["capped"][0]["live"], 3);
}

#[tokio::test]
async fn with_the_cpu_busy_the_mutants_issue_waits_as_an_ordinary_one_does() {
    let w = World::new();
    at_its_cap(&w).await;
    let t = w.tick().await;
    assert!(t.started.is_none() && t.held.as_deref().unwrap_or("").starts_with("cpu"), "{t:?}");
    w.host.reading.lock().unwrap().cpu_busy = 10;
    assert_eq!(w.tick().await.started.expect("a start")["issue"], "web#5");
}
