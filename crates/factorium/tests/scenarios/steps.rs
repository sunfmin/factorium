//! Where an attempt's time went, read from factorium's own records
//! (factorium#308): each heavy command's wait in the queue and its run
//! apart, each landing's steps, and a red's kind and files.

use serde_json::{Value, json};

use crate::fixture::{CLAUDE, POLL, STUCK, World, id, until_file};

/// The steps of `web`#`n`'s first attempt, as `issue show --steps` answers them.
async fn steps_of(w: &World, n: u64) -> Value {
    let shown = w.app.issue_show_steps("web", n).await.unwrap();
    assert_eq!(shown["spent"]["attempts"][0]["attempt"], 1, "{shown}");
    shown["spent"]["attempts"][0]["steps"].clone()
}

/// The ms of `v` at `key`.
fn ms(v: &Value, key: &str) -> i64 {
    v[key].as_i64().unwrap_or_else(|| panic!("{key} in {v}"))
}

#[tokio::test]
async fn a_worker_gate_waiting_behind_another_run_records_its_wait_and_its_run_apart() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    // It asks once the other's run holds the queue.
    let gated = async {
        until_file(&w.dir.join("busy/started")).await;
        w.app.worker_gate(id(&wk)).await
    };
    let busy = async {
        w.while_busy(async {
            w.until_queued("fast gate of web#1").await;
            w.advance(5);
        })
        .await;
        // The busy run ended: the gate asks again, woken by the clock if it sleeps.
        w.until(async || {
            if w.clock.asleep() > 0 {
                w.clock.advance(POLL);
            }
            w.queued().await.is_empty()
        })
        .await;
    };
    let (gated, ()) = tokio::time::timeout(STUCK, async { tokio::join!(gated, busy) }).await.expect("the gate runs");
    assert_eq!(gated.unwrap()["green"], true);

    let heavy = steps_of(&w, 1).await["heavy"].clone();
    let [run] = heavy.as_array().unwrap().as_slice() else { panic!("the other's run is not its own: {heavy}") };
    assert_eq!(run["what"], "fast gate of web#1");
    assert_eq!(run["outcome"], "green");
    let (asked, started, ended) = (ms(run, "asked_at"), ms(run, "started_at"), ms(run, "ended_at"));
    assert!(ms(run, "queued_ms") >= 5 * 60_000, "it waited out the other's run: {run}");
    assert_eq!(ms(run, "queued_ms"), started - asked, "{run}");
    assert_eq!(ms(run, "ran_ms"), ended - started, "its run, apart from its wait: {run}");
}

#[tokio::test]
async fn a_landing_red_on_a_conflict_records_its_kind_its_files_and_when_it_was_told() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    w.push_by_hand("web", "proving", "README", "theirs\n");
    w.done(&wk, "README", "mine\n").await;
    w.tick().await;

    let landing = steps_of(&w, 1).await["landings"][0].clone();
    assert_eq!(landing["outcome"], "red", "{landing}");
    assert_eq!(landing["red"]["kind"], "no_merge", "{landing}");
    assert_eq!(landing["red"]["files"], json!(["README"]), "{landing}");
    assert_eq!(landing["gates"], json!([]), "no gate ran: {landing}");
    let told = &landing["red"]["told"];
    assert_eq!(told["from"]["kind"], "factorium", "{landing}");
    assert!(ms(told, "reached_at") >= ms(&landing, "ended_at"), "it reached the session: {landing}");
}

#[tokio::test]
async fn a_landing_that_goes_green_records_each_step() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    w.app.worker_waiting(id(&wk), "a download", 60_000).await.unwrap();
    w.advance(1);
    w.done(&wk, "a", "x").await;
    let landed = w.tick().await.landed;
    let commit = landed[0]["commit"].as_str().unwrap_or_else(|| panic!("landed: {landed:?}"));

    let steps = steps_of(&w, 1).await;
    let landing = &steps["landings"][0];
    assert_eq!(landing["outcome"], "landed", "{landing}");
    assert_eq!(landing["commit"], commit);
    assert!(ms(&landing["checked"][0], "at") >= ms(landing, "asked_at"), "its merge checked: {landing}");
    let gate = &landing["gates"][0];
    assert_eq!(gate["what"], "fast gate of web", "{landing}");
    assert_eq!(gate["outcome"], "green", "{landing}");
    assert!(ms(gate, "asked_at") <= ms(gate, "started_at") && ms(gate, "started_at") <= ms(gate, "ended_at"));
    assert!(ms(gate, "ended_at") <= ms(landing, "ended_at"), "{landing}");
    let wait = &steps["waits"][0];
    assert_eq!(wait["why"], "a download");
    assert_eq!(ms(wait, "ended_at"), ms(landing, "asked_at"), "`done` ends the wait it declared: {steps}");
}

#[tokio::test]
async fn a_park_behind_what_its_issue_waits_for_records_why() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.app.issue_block("web", 1, 2, Some("needs b's parser"), false).await.unwrap();

    let parks = steps_of(&w, 1).await["parks"].clone();
    assert_eq!(parks[0]["why"], "waits for #2: needs b's parser", "{parks}");
    assert_eq!(parks[0]["ended_at"], Value::Null, "{parks}");
}
