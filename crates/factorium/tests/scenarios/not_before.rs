//! An issue can wait until a set time, not only for other issues
//! (factorium#229): no worker takes it before then, doctor names the time,
//! and the first tick at or after it treats the wait as met, as it does a
//! met block — starting the issue, or sending its parked worker back.

use serde_json::{Value, json};

use factorium_domain::issue::Blocker;
use factorium_domain::when;

use crate::fixture::{CLAUDE, World, id};

/// The world's clock, `minutes` from now, in ms.
fn after(w: &World, minutes: i64) -> i64 {
    w.app_now() + minutes * 60_000
}

/// What doctor lists of ready issues waiting for their time.
async fn timed(w: &World) -> Value {
    w.run().doctor().await.unwrap()["not_before"].clone()
}

#[test]
fn a_time_is_read_with_its_offset_and_shown_in_utc() {
    let at = when::parse("2026-11-23T14:34:37+09:00").unwrap();
    assert_eq!(when::utc(at), "2026-11-23T05:34:37Z");
    assert_eq!(when::parse("2026-11-23 05:34:37.250Z"), Ok(at + 250), "a space for the T, to the ms");
    assert_eq!(when::utc(at + 250), "2026-11-23T05:34:37Z", "shown to the second");
    assert_eq!(when::utc(i64::MAX), "9999-12-30T22:00:00Z", "past the range, its end");
    assert_eq!(when::parse(&at.to_string()), Ok(at), "ms since the epoch");
    assert_eq!(when::utc(1_800_000_000_000), "2027-01-15T08:00:00Z");
    assert_eq!(when::utc(when::parse("2024-02-29T23:59:59-00:30").unwrap()), "2024-03-01T00:29:59Z");
    let bad = [
        "tomorrow",
        "2026-11-23T14:34:37",
        "2026-11-23T14:34:37+xx:00",
        "2026-11-23T14:34:37 Z",
        "99999999999999999",
        "2026-02-30T00:00:00Z",
        "2026-13-01T00:00:00Z",
        "2026-11-23T24:00:00Z",
        "",
    ];
    for bad in bad {
        assert!(when::parse(bad).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn an_issue_not_before_a_time_starts_on_the_first_tick_at_it() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    let at = after(&w, 60);
    let shown = w.app.issue_add("web", "recount", "", false, &[Blocker::NotBefore(at)]).await.unwrap();
    assert_eq!(shown["not_before"], json!({ "ms": at, "utc": when::utc(at) }));

    assert!(w.tick().await.started.is_none(), "its time has not come");
    assert_eq!(timed(&w).await, json!([{ "issue": "web#1", "until": at, "utc": when::utc(at) }]));
    w.app.issue_add("web", "later", "", false, &[]).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.started.as_ref().map(|s| s["issue"].clone()), Some(json!("web#2")), "the one after it starts: {t:?}");

    w.clock.advance(60 * 60_000 - 1);
    assert!(w.tick().await.started.is_none(), "a ms before its time");
    w.clock.advance(1);
    assert_eq!(timed(&w).await, json!([]), "at its time it waits no more");
    let t = w.tick().await;
    assert_eq!(t.started.as_ref().map(|s| s["issue"].clone()), Some(json!("web#1")), "{t:?}");
    assert_eq!(timed(&w).await, json!([]));
}

#[tokio::test]
async fn a_time_is_set_changed_and_undone_on_an_existing_issue() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let block = |at: i64, undo: bool| w.app.issue_block("web", 1, Blocker::NotBefore(at), undo);
    let (first, second) = (after(&w, 30), after(&w, 90));
    assert_eq!(block(first, false).await.unwrap()["not_before"]["ms"], first);
    assert_eq!(block(second, false).await.unwrap()["not_before"]["ms"], second, "it replaces the one before");
    assert_eq!(timed(&w).await[0]["until"], second);
    assert_eq!(block(0, true).await.unwrap()["not_before"], json!(null), "undone, whatever time is named");
    assert_eq!(timed(&w).await, json!([]));
    assert_eq!(block(0, true).await.unwrap()["not_before"], json!(null), "undone again, nothing changes");
}

#[tokio::test]
async fn a_waiting_worker_parked_until_a_time_goes_back_to_work_once_it_comes() {
    let w = World::new();
    let wk = w.working(CLAUDE, "measure").await;
    w.watch_session(false);
    w.app.worker_escalate(id(&wk), "needs 24h of counts").await.unwrap();
    let at = after(&w, 30);
    w.app.issue_block("web", 1, Blocker::NotBefore(at), false).await.unwrap();
    let d = w.run().doctor().await.unwrap();
    let name = format!("not before {}", when::utc(at));
    assert_eq!(d["parked"][0]["waits_for"], json!([name]), "{d}");
    assert_eq!(d["not_before"], json!([]), "a held issue is doctor's parked worker");

    w.advance(29);
    assert!(w.tick().await.resumed.is_empty(), "its time has not come");
    w.advance(1);
    let t = w.tick().await;
    assert_eq!(t.resumed.len(), 1, "{t:?}");
    assert_eq!(w.worker_on("web", 1).await["status"], "working");
    let said: Vec<String> = (w.comments("web", 1).await.iter())
        .filter(|c| c["by"] == "factorium")
        .map(|c| c["text"].as_str().unwrap().to_string())
        .collect();
    assert!(said.iter().any(|s| s.contains(&format!("{name} (its time came)"))), "{said:?}");
}
