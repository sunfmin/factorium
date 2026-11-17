//! What factorium opens, factorium closes: a worker's terminal when the
//! worker ends, whatever ends it; the watch's on `factorium stop`; and
//! doctor names the factorium terminals nothing holds.

use std::path::Path;

use serde_json::Value;

use crate::{Terminal, World, id, worker};

/// A checkout, a kit, and one worker started on a new issue.
async fn one_worker(w: &World) -> Value {
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    worker(&w.app).await
}

fn closed(w: &World, wk: &Value) -> bool {
    w.terminal(wk["terminal"].as_str().unwrap()).closed
}

#[tokio::test]
async fn a_landed_worker_s_terminal_is_closed() {
    let w = World::new();
    let wk = one_worker(&w).await;
    w.commit(Path::new(wk["worktree"].as_str().unwrap()), "a.txt", "a\n");
    w.app.worker_done(id(&wk)).await.unwrap();
    assert!(!w.tick().await.landed.is_empty());
    assert!(closed(&w, &wk));
}

#[tokio::test]
async fn closing_or_parking_its_issue_closes_the_worker_s_terminal() {
    let w = World::new();
    let wk = one_worker(&w).await;
    w.app.issue_close("web", 1, "not needed").await.unwrap();
    assert!(closed(&w, &wk));

    w.app.issue_add("web", "u", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.app.issue_park("web", 2, "later").await.unwrap();
    assert!(closed(&w, &wk));
}

#[tokio::test]
async fn a_used_up_account_closes_the_worker_s_terminal() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let screen = ["■ You've hit your usage limit reached.", "Increase your limits to continue using Codex."];
    w.set_screen(wk["terminal"].as_str().unwrap(), &screen);
    assert_eq!(w.tick().await.seen[0]["ended"], "quota");
    assert!(closed(&w, &wk));
}

#[tokio::test]
async fn the_watch_restarting_a_worker_whose_session_is_gone_closes_its_terminal() {
    let w = World::new();
    let wk = one_worker(&w).await;
    w.kill_terminal(wk["terminal"].as_str().unwrap());
    w.tick().await;
    assert!(!closed(&w, &wk), "its program ended; its tab stays until closed");
    assert_eq!(w.app.watch_restart(id(&wk), "go on", false).await.unwrap()["ended"], true);
    assert!(closed(&w, &wk));
}

#[tokio::test]
async fn a_session_that_cannot_be_handed_its_brief_has_its_terminal_closed() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    *w.host.orca.refuse_send.lock().unwrap() = true;
    assert!(w.app.tick().await.is_err());
    let open: Vec<Terminal> =
        w.host.orca.terminals.lock().unwrap().values().filter(|t| t.title == "web#1" && !t.closed).cloned().collect();
    assert!(open.is_empty(), "{open:?}");
}

#[tokio::test]
async fn stop_closes_the_watch_s_terminal_and_the_next_tick_starts_a_new_session() {
    let w = World::new();
    w.kit().await;
    let first = w.tick().await.watch.unwrap()["watch"].as_str().unwrap().to_string();
    let stopped = w.app.stop().await.unwrap();
    assert_eq!(stopped["watch_closed"], first);
    assert!(w.terminal(&first).closed);
    assert_eq!(w.app.doctor().await.unwrap()["watch"]["terminal"], Value::Null, "the watch has no session");

    w.advance(5);
    let second = w.tick().await.watch.unwrap()["watch"].as_str().unwrap().to_string();
    assert_ne!(second, first, "a new session");
    assert!(!w.terminal(&second).closed);
}

#[tokio::test]
async fn doctor_names_a_factorium_terminal_no_worker_or_watch_holds() {
    let w = World::new();
    let wk = one_worker(&w).await;
    {
        let mut t = w.host.orca.terminals.lock().unwrap();
        for (handle, title) in [("lost-1", "web#9"), ("lost-2", "factorium watch"), ("mine", "notes"), ("x", "api#1")] {
            let term = Terminal { title: title.into(), alive: true, ..Default::default() };
            t.insert(handle.into(), term);
        }
    }
    let found = w.app.doctor().await.unwrap()["findings"].as_array().unwrap().clone();
    let keys: Vec<&str> = found.iter().filter_map(|f| f["key"].as_str()).collect();
    assert!(keys.contains(&"orphan:lost-1") && keys.contains(&"orphan:lost-2"), "{keys:?}");
    for not in ["orphan:mine", "orphan:x", &format!("orphan:{}", wk["terminal"].as_str().unwrap())] {
        assert!(!keys.contains(&not), "{not} in {keys:?}");
    }
    let lost = found.iter().find(|f| f["key"] == "orphan:lost-1").unwrap();
    assert!(lost["fix"].as_str().unwrap().contains("orca terminal close --terminal lost-1"), "{lost}");
    let watch = w.app.doctor().await.unwrap()["watch"]["terminal"].clone();
    assert!(!keys.contains(&format!("orphan:{}", watch.as_str().unwrap()).as_str()), "the watch's own");
}
