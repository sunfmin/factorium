//! Where a checkout's issues spent their time, read from
//! each attempt's steps: the parts its time is cut into, the reds and the
//! files that conflicted.

use factorium_domain::settings::Resource;
use serde_json::{Value, json};

use crate::fixture::{CLAUDE, LOOK, POLL, STUCK, World, id, until_file};

const MIN: i64 = 60_000;

#[tokio::test]
async fn two_issues_one_green_after_a_wait_one_red_twice_on_a_conflict_answer_each_part_by_the_clock() {
    let w = World::new();
    w.two_at_work().await;
    let start = w.app_now();
    let (one, two) = (w.worker_on("web", 1).await, w.worker_on("web", 2).await);
    for wk in [&one, &two] {
        w.claude_session(wk["worktree"].as_str().unwrap(), false, (start / 1000) as u64);
    }
    w.push_by_hand("web", "proving", "README", "theirs\n");
    w.app.worker_waiting(id(&one), "a download", 60 * MIN).await.unwrap();
    w.advance(4);
    w.done(&one, "one.txt", "x\n").await;
    w.advance(2);
    let landed = w.tick().await.landed;
    assert!(landed.iter().any(|l| l["landed"] == "web#1"), "{landed:?}");
    // #2 red on README at 9, and again at 12.
    w.done(&two, "README", "mine\n").await;
    w.advance(3);
    w.tick().await;
    w.advance(1);
    w.done(&two, "README", "mine again\n").await;
    w.advance(2);
    w.tick().await;
    w.advance(1);

    let s = w.app.spent_of("web", None, None).await.unwrap();
    let row = |n: u64| s["issues"].as_array().unwrap().iter().find(|r| r["number"] == n).cloned().unwrap();
    let (r1, r2) = (row(1), row(2));
    assert_eq!(r1["split"]["waits"], json!({ "ms": 4 * MIN, "by": { "a download": 4 * MIN } }), "{r1}");
    assert_eq!(r1["split"]["landing"]["green"], 2 * MIN, "{r1}");
    assert_eq!(r2["split"]["landing"]["red"], json!({ "ms": 5 * MIN, "by": { "no_merge": 5 * MIN } }), "{r2}");
    assert_eq!(r2["reds"]["by_kind"], json!({ "no_merge": 2 }), "{r2}");
    for r in [&r1, &r2] {
        let sp = &r["split"];
        let parts: i64 = ["heavy_queued", "heavy_ran", "landing", "waits", "parks"].iter().map(|k| ms(&sp[k])).sum();
        assert_eq!(parts + sp["rest"].as_i64().unwrap(), r["ms"].as_i64().unwrap(), "the parts sum to its time: {r}");
    }
    assert_eq!(r2["ms"], w.app_now() - start, "#2 still works: to now");
    assert_eq!(s["reds"]["files"], json!([{ "file": "README", "landings": 2 }]), "{s}");
    let t = &s["totals"];
    assert_eq!((t["issues"].clone(), t["attempts"].clone()), (json!(2), json!(2)), "{t}");
    assert_eq!(t["tokens"], json!({ "in": 30, "out": 22 }), "both sessions' tokens: {t}");
    assert_eq!(t["wall_ms"], 6 * MIN, "first start to #1's landing: {t}");
    assert_eq!(s["split"]["landing"]["ms"], 7 * MIN, "{s}");

    // One issue, and the time since #2's first `done` alone.
    let one_only = w.app.spent_of("web", Some((2, 2)), Some(start + 6 * MIN)).await.unwrap();
    assert_eq!(one_only["totals"]["ms"], 7 * MIN, "{one_only}");
    assert_eq!(one_only["split"]["landing"]["red"]["ms"], 5 * MIN, "{one_only}");
    // Since 10: #1 ended before it, and #2's first red came before it.
    let late = w.app.spent_of("web", None, Some(start + 10 * MIN)).await.unwrap();
    assert_eq!((late["totals"]["issues"].clone(), late["reds"]["count"].clone()), (json!(1), json!(1)), "{late}");
    assert_eq!(late["totals"]["wall_ms"], Value::Null, "nothing landed since: {late}");
    let no = w.app.spent_of("web", Some((9, 9)), None).await.unwrap_err();
    assert_eq!(no.code, "no_issue", "{no:?}");
}

#[tokio::test]
async fn a_worker_gate_queued_behind_another_run_is_a_gate_s_wait() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
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

    let queued = w.app.spent_of("web", Some((1, 1)), None).await.unwrap()["split"]["heavy_queued"].clone();
    let by = queued["by"].as_object().unwrap();
    assert_eq!(by.keys().collect::<Vec<_>>(), ["gate"], "{queued}");
    assert!(ms(&queued) >= 5 * MIN, "it waited out the other's run: {queued}");
}

#[tokio::test]
async fn a_hold_then_a_command_by_hand_then_a_park_are_each_its_own_part() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    let name = "PHONE".to_string();
    w.app.resource_declare(&name, Some(Resource { slots: 1, note: None }), false).await.unwrap();
    // Held for 2.
    w.app.worker_hold(id(&wk), std::slice::from_ref(&name), 60 * MIN, None).await.unwrap();
    w.advance(2);
    w.app.worker_release(id(&wk), &name).await.unwrap();
    // A command by hand runs for 3.
    let dir = w.dir.join("by-hand");
    let line = w.holds(&dir);
    let ran = async {
        while !dir.join("started").exists() {
            tokio::time::sleep(LOOK).await;
        }
        w.advance(3);
        std::fs::write(dir.join("release"), "").unwrap();
    };
    let (ran, ()) =
        tokio::time::timeout(STUCK, async { tokio::join!(w.app.worker_exclusive(Some(id(&wk)), &w.dir, &line), ran) })
            .await
            .expect("the command runs");
    assert_eq!(ran.unwrap()["ok"], true);
    // Parked behind #2 for 4, to now.
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.app.issue_block("web", 1, 2, Some("needs b's parser"), false).await.unwrap();
    w.advance(4);

    let split = w.app.spent_of("web", Some((1, 1)), None).await.unwrap()["split"].clone();
    assert_eq!(
        split["heavy_ran"],
        json!({ "ms": 5 * MIN, "by": { "hold": 2 * MIN, "exclusive": 3 * MIN } }),
        "{split}"
    );
    assert_eq!(
        split["parks"],
        json!({ "ms": 4 * MIN, "by": { "waits for #2: needs b's parser": 4 * MIN } }),
        "{split}"
    );
}

#[tokio::test]
async fn a_landing_still_open_is_neither_green_nor_red() {
    let w = World::new();
    let wk = w.working(CLAUDE, "a").await;
    w.done(&wk, "a", "x").await;
    w.advance(2);

    let landing = w.app.spent_of("web", Some((1, 1)), None).await.unwrap()["split"]["landing"].clone();
    assert_eq!(
        landing,
        json!({ "ms": 2 * MIN, "green": 0, "red": { "ms": 0, "by": {} }, "other": 2 * MIN }),
        "{landing}"
    );
}

/// The ms of a part.
fn ms(v: &Value) -> i64 {
    v["ms"].as_i64().unwrap_or_else(|| panic!("ms in {v}"))
}
