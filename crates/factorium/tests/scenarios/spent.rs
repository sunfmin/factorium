//! Where a checkout's issues spent their time (factorium#309), read from
//! each attempt's steps: the parts its time is cut into, the reds and the
//! files that conflicted.

use serde_json::{Value, json};

use crate::fixture::{World, id};

const MIN: i64 = 60_000;

#[tokio::test]
async fn two_issues_one_green_after_a_wait_one_red_on_a_conflict_answer_each_part_by_the_clock() {
    let w = World::new();
    w.two_at_work().await;
    let start = w.app_now();
    w.push_by_hand("web", "proving", "README", "theirs\n");
    let one = w.worker_on("web", 1).await;
    w.app.worker_waiting(id(&one), "a download", 60 * MIN).await.unwrap();
    w.advance(4);
    w.done(&one, "one.txt", "x\n").await;
    w.advance(2);
    let landed = w.tick().await.landed;
    assert!(landed.iter().any(|l| l["landed"] == "web#1"), "{landed:?}");
    let two = w.worker_on("web", 2).await;
    w.done(&two, "README", "mine\n").await;
    w.advance(3);
    w.tick().await;
    w.advance(1);

    let s = w.app.spent_of("web", None, None).await.unwrap();
    let row = |n: u64| s["issues"].as_array().unwrap().iter().find(|r| r["number"] == n).cloned().unwrap();
    let (r1, r2) = (row(1), row(2));
    assert_eq!(r1["split"]["waits"], json!({ "ms": 4 * MIN, "by": { "a download": 4 * MIN } }), "{r1}");
    assert_eq!(r1["split"]["landing"]["green"], 2 * MIN, "{r1}");
    assert_eq!(r2["split"]["landing"]["red"], json!({ "ms": 3 * MIN, "by": { "no_merge": 3 * MIN } }), "{r2}");
    assert_eq!(r2["reds"]["by_kind"], json!({ "no_merge": 1 }), "{r2}");
    for r in [&r1, &r2] {
        let sp = &r["split"];
        let parts: i64 = ["heavy_queued", "heavy_ran", "landing", "waits", "parks"].iter().map(|k| ms(&sp[k])).sum();
        assert_eq!(parts + sp["rest"].as_i64().unwrap(), r["ms"].as_i64().unwrap(), "the parts sum to its time: {r}");
    }
    assert_eq!(r2["ms"], w.app_now() - start, "#2 still works: to now");
    assert_eq!(s["reds"]["files"], json!([{ "file": "README", "landings": 1 }]), "{s}");
    let t = &s["totals"];
    assert_eq!((t["issues"].clone(), t["attempts"].clone()), (json!(2), json!(2)), "{t}");
    assert_eq!(t["wall_ms"], 6 * MIN, "first start to #1's landing: {t}");
    assert_eq!(s["split"]["landing"]["ms"], 5 * MIN, "{s}");

    // One issue, and the time since #2's `done` alone.
    let one_only = w.app.spent_of("web", Some((2, 2)), Some(start + 6 * MIN)).await.unwrap();
    assert_eq!(one_only["totals"]["ms"], 4 * MIN, "{one_only}");
    assert_eq!(one_only["split"]["landing"]["red"]["ms"], 3 * MIN, "{one_only}");
    let no = w.app.spent_of("web", Some((9, 9)), None).await.unwrap_err();
    assert_eq!(no.code, "no_issue", "{no:?}");
}

/// The ms of a part.
fn ms(v: &Value) -> i64 {
    v["ms"].as_i64().unwrap_or_else(|| panic!("ms in {v}"))
}
