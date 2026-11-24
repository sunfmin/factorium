//! Resources, one queue each (factorium#284): seen 2026-11-24, rollmind
//! ordered its issues with `issue block` for one phone, and 13 of its 18
//! ready issues waited on one 2.5 h soak, though most needed the phone only
//! for their last step. `host` is the heavy queue as it was (its scenarios
//! in `queue.rs` and the rest run unchanged); a named one has slots.

use std::path::Path;

use factorium::Exclusive;
use factorium_domain::settings::Resource;
use factum::Uuid;
use serde_json::Value;

use crate::fixture::{CLAUDE, LOOK, POLL, STUCK, World, finding_keys, id, until_file, worker};

/// Declares the host's resources `names`, one slot each.
async fn declare(w: &World, names: &[&str]) {
    for name in names {
        let note = Some(format!("{name}, as the owner names it"));
        w.app.resource_declare(name, Some(Resource { slots: 1, note }), false).await.unwrap();
    }
}

/// Asking for `names`, all at once.
fn on(names: &[&str]) -> Exclusive {
    Exclusive { resources: names.iter().map(|n| n.to_string()).collect(), ..Exclusive::default() }
}

/// Doctor's view of resource `name`.
async fn resource(w: &World, name: &str) -> Value {
    let d = w.doctor().await;
    let all = d["resources"].as_array().cloned().unwrap_or_default();
    all.into_iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("{name} is not shown: {}", d["resources"]))
}

/// The `what` of each of `list`'s holdings.
fn whats(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|h| h["what"].as_str().unwrap().to_string()).collect()
}

/// Until `dir`'s holding command started, moving the clock a poll on each look.
async fn until_started(w: &World, dir: &Path) {
    while !dir.join("started").exists() {
        w.clock.advance(POLL);
        tokio::time::sleep(LOOK).await;
    }
}

/// The finding keyed `key`'s text, as doctor says it.
async fn finding(w: &World, key: &str) -> String {
    let d = w.doctor().await;
    let found = d["findings"].as_array().unwrap().iter().find(|f| f["key"] == key).cloned();
    found.unwrap_or_else(|| panic!("no finding {key}: {}", d["findings"]))["what"].as_str().unwrap().into()
}

#[tokio::test]
async fn two_asking_for_one_slot_run_one_after_the_other_while_a_third_with_none_runs_beside() {
    let w = World::new();
    declare(&w, &["FMGG"]).await;
    // No room on the host: the third starts because nothing else holds the host.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let dirs = [w.dir.join("first"), w.dir.join("second"), w.dir.join("third")];
    let lines = dirs.clone().map(|d| w.holds(&d));
    let clis: [_; 3] = std::array::from_fn(|_| w.run());
    let first = clis[0].worker_exclusive_as(None, &w.dir, &lines[0], on(&["FMGG"]));
    let rest = async {
        until_file(&dirs[0].join("started")).await;
        let second = clis[1].worker_exclusive_as(None, &w.dir, &lines[1], on(&["FMGG"]));
        let third = async {
            w.until(async || w.queued().await.len() == 2).await;
            clis[2].worker_exclusive(None, &w.dir, &lines[2]).await
        };
        let looks = async {
            until_file(&dirs[2].join("started")).await;
            w.until(async || w.clock.asleep() == 3).await;
            w.clock.advance(POLL);
            w.until(async || w.clock.asleep() == 3).await;
            assert!(!dirs[1].join("started").exists(), "the second waits for FMGG");
            let fmgg = resource(&w, "FMGG").await;
            assert_eq!(fmgg["rule"], serde_json::json!({ "slots": 1 }), "{fmgg}");
            assert_eq!(whats(&fmgg["holders"]), [lines[0].clone()], "{fmgg}");
            assert_eq!(whats(&fmgg["waiting"]), [lines[1].clone()], "{fmgg}");
            assert_eq!(whats(&resource(&w, "host").await["holders"]), [lines[2].clone()]);
            let keys = finding_keys(&w.doctor().await);
            assert!(!keys.iter().any(|k| k.starts_with("queue:")), "waiting for FMGG is no stall: {keys:?}");
            std::fs::write(dirs[0].join("release"), "").unwrap();
            w.gone(&lines[0]).await;
            until_started(&w, &dirs[1]).await;
            for d in [&dirs[1], &dirs[2]] {
                std::fs::write(d.join("release"), "").unwrap();
            }
        };
        tokio::join!(second, third, looks)
    };
    let (first, (second, third, ())) =
        tokio::time::timeout(STUCK, async { tokio::join!(first, rest) }).await.expect("all three run");
    for r in [first, second, third] {
        assert_eq!(r.unwrap()["ok"], true);
    }
}

#[tokio::test]
async fn one_asking_for_two_resources_never_holds_one_while_it_waits_for_the_other() {
    let w = World::new();
    declare(&w, &["FMGG", "MAC"]).await;
    let dirs = [w.dir.join("phone"), w.dir.join("both"), w.dir.join("mac")];
    let lines = dirs.clone().map(|d| w.holds(&d));
    let clis: [_; 3] = std::array::from_fn(|_| w.run());
    let phone = clis[0].worker_exclusive_as(None, &w.dir, &lines[0], on(&["FMGG"]));
    let rest = async {
        until_file(&dirs[0].join("started")).await;
        let both = clis[1].worker_exclusive_as(None, &w.dir, &lines[1], on(&["FMGG", "MAC"]));
        let mac = async {
            w.until(async || w.queued().await.len() == 2).await;
            clis[2].worker_exclusive_as(None, &w.dir, &lines[2], on(&["MAC"])).await
        };
        let looks = async {
            // MAC is free, but the one asking for both waits for FMGG: the later one takes MAC.
            until_file(&dirs[2].join("started")).await;
            assert!(!dirs[1].join("started").exists());
            assert_eq!(whats(&resource(&w, "MAC").await["holders"]), [lines[2].clone()]);
            assert_eq!(whats(&resource(&w, "FMGG").await["waiting"]), [lines[1].clone()]);
            std::fs::write(dirs[0].join("release"), "").unwrap();
            w.gone(&lines[0]).await;
            w.clock.advance(POLL);
            w.until(async || w.clock.asleep() == 2).await;
            assert!(!dirs[1].join("started").exists(), "FMGG free, MAC held: it still holds neither");
            assert!(resource(&w, "FMGG").await["holders"].as_array().unwrap().is_empty());
            std::fs::write(dirs[2].join("release"), "").unwrap();
            w.gone(&lines[2]).await;
            until_started(&w, &dirs[1]).await;
            std::fs::write(dirs[1].join("release"), "").unwrap();
        };
        tokio::join!(both, mac, looks)
    };
    let (phone, (both, mac, ())) =
        tokio::time::timeout(STUCK, async { tokio::join!(phone, rest) }).await.expect("all three run");
    for r in [phone, both, mac] {
        assert_eq!(r.unwrap()["ok"], true);
    }
}

#[tokio::test]
async fn a_declared_long_hold_is_not_cut_short_at_the_hold_bound_and_past_its_until_is_a_finding() {
    let w = World::new();
    declare(&w, &["FMGG"]).await;
    let (me, other) = (Uuid::now_v7(), Uuid::now_v7());
    let held = w.app.worker_hold(me, &["FMGG".into()], 160 * 60_000).await.unwrap();
    let entry = held["held"]["entry"].as_str().unwrap().to_string();
    assert_eq!(held["held"]["until"].as_i64().unwrap() - held["held"]["since"].as_i64().unwrap(), 160 * 60_000);
    // The holder's own command runs inside its hold; another's waits for it.
    let cli = w.run();
    let mine = tokio::time::timeout(STUCK, cli.worker_exclusive_as(Some(me), &w.dir, "echo mine", on(&["FMGG"])));
    assert_eq!(mine.await.expect("it runs inside its hold").unwrap()["ok"], true);
    let dir = w.dir.join("other");
    let line = w.holds(&dir);
    let cli = w.run();
    let waits = cli.worker_exclusive_as(Some(other), &w.dir, &line, on(&["FMGG"]));
    let looks = async {
        w.until_queued(&line).await;
        // Past the hold bound, with another waiting for it: it is held on.
        w.advance(31);
        w.tick_alone().await;
        w.poll(1).await;
        w.until(async || w.clock.asleep() == 1).await;
        assert!(!dir.join("started").exists(), "FMGG is held still");
        assert!(!finding_keys(&w.doctor().await).contains(&format!("resource:{entry}")));
        let workers = w.app.workers(false).await.unwrap();
        assert!(workers.as_array().unwrap().is_empty(), "no live worker in this world: {workers}");
        // Past its until: a finding for the watch, and still held.
        w.advance(130);
        let what = finding(&w, &format!("resource:{entry}")).await;
        assert!(what.contains("past the time it declared, by 1 min") && what.contains("1 waiting"), "{what}");
        w.app.worker_release(me, "FMGG").await.unwrap();
        until_started(&w, &dir).await;
        std::fs::write(dir.join("release"), "").unwrap();
    };
    let (waited, ()) = tokio::time::timeout(STUCK, async { tokio::join!(waits, looks) }).await.expect("it runs");
    assert_eq!(waited.unwrap()["ok"], true);
    let again = w.app.worker_release(me, "FMGG").await.expect_err("released already");
    assert_eq!(again.code, "not_held");
}

#[tokio::test]
async fn a_dead_holders_resource_is_given_back() {
    let w = World::new();
    declare(&w, &["FMGG"]).await;
    let (first, second) = (w.dir.join("first"), w.dir.join("second"));
    let lines = [w.holds(&first), w.holds(&second)];
    let (a, b) = (w.run(), w.run());
    let holds = a.worker_exclusive_as(None, &w.dir, &lines[0], on(&["FMGG"]));
    let rest = async {
        until_file(&first.join("started")).await;
        let waits = b.worker_exclusive_as(None, &w.dir, &lines[1], on(&["FMGG"]));
        let looks = async {
            w.until(async || w.queued().await.len() == 2 && w.clock.asleep() == 2).await;
            let entry = w.queued_entry(&lines[0]).await["id"].as_str().unwrap().to_string();
            // The holder's process reads as dead: doctor says so, and the tick gives FMGG back.
            *w.host.gone.lock().unwrap() = true;
            let what = finding(&w, &format!("resource:{entry}")).await;
            assert!(what.contains("its waiter died; the next tick gives it back"), "{what}");
            w.tick_alone().await;
            *w.host.gone.lock().unwrap() = false;
            let what = finding(&w, &format!("resource:{entry}:FMGG")).await;
            assert!(what.contains("was given back") && what.contains("its waiter died"), "{what}");
            until_started(&w, &second).await;
            assert_eq!(whats(&resource(&w, "FMGG").await["holders"]), [lines[1].clone()]);
            std::fs::write(second.join("release"), "").unwrap();
            // An hour on, it is no finding.
            w.advance(61);
            assert!(!finding_keys(&w.doctor().await).contains(&format!("resource:{entry}:FMGG")));
        };
        tokio::join!(waits, looks).0
    };
    let (held, waited) = tokio::time::timeout(STUCK, async { tokio::join!(holds, rest) }).await.expect("both end");
    assert_eq!(held.expect_err("its entry was given back").code, "unqueued");
    assert_eq!(waited.unwrap()["ok"], true);
}

#[tokio::test]
async fn a_hold_whose_worker_ended_is_given_back_and_workers_show_holds() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    declare(&w, &["FMGG"]).await;
    let me = id(&worker(&w.app).await);
    w.app.worker_hold(me, &["FMGG".into()], 160 * 60_000).await.unwrap();
    let shown = worker(&w.app).await;
    assert_eq!(shown["holds"][0]["resource"], "FMGG", "{shown}");
    assert_eq!(shown["holds"][0]["hold"], true, "{shown}");
    w.app.issue_close("web", 1, "left as it is").await.unwrap();
    w.tick_alone().await;
    assert!(resource(&w, "FMGG").await["holders"].as_array().unwrap().is_empty(), "given back");
    let keys = finding_keys(&w.doctor().await);
    assert!(keys.iter().any(|k| k.starts_with("resource:") && k.ends_with(":FMGG")), "{keys:?}");
}

#[tokio::test]
async fn a_resource_the_host_does_not_declare_is_refused_and_host_is_not_held() {
    let w = World::new();
    declare(&w, &["FMGG"]).await;
    let refused = w.run().worker_exclusive_as(None, &w.dir, "true", on(&["FMGX"])).await.expect_err("not declared");
    assert_eq!(refused.code, "unknown_resource", "{refused}");
    assert!(
        refused.message.contains("[\"FMGG\"]") && refused.message.contains("--resource FMGX --slots 1"),
        "{refused}"
    );
    let host = w.app.worker_hold(Uuid::now_v7(), &["host".into()], 60_000).await.expect_err("host is not held");
    assert_eq!(host.code, "host_hold");
    let declared = w.app.resource_declare("host", Some(Resource { slots: 1, note: None }), false).await;
    assert_eq!(declared.expect_err("host is not declared").code, "host_resource");
    assert!(w.queued().await.is_empty(), "nothing was queued");
    let cli = w.run();
    let ran = tokio::time::timeout(STUCK, cli.worker_exclusive_as(None, &w.dir, "echo both", on(&["host", "FMGG"])));
    assert_eq!(ran.await.expect("host and FMGG at once").unwrap()["ok"], true);
}
