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

/// Real Orca lists a terminal until its close returns, seconds for an
/// agent's: a doctor run in that time (the watch's) finds the worker still
/// holds it, so the watch never closes it in the worker's stead.
#[tokio::test]
async fn a_worker_s_terminal_is_never_an_orphan_while_it_closes() {
    let w = World::new();
    let wk = one_worker(&w).await;
    let found = w.doctor_while_closing();
    w.commit(Path::new(wk["worktree"].as_str().unwrap()), "a.txt", "a\n");
    w.app.worker_done(id(&wk)).await.unwrap();
    assert!(!w.tick().await.landed.is_empty());
    w.app.issue_add("web", "u", "", false, &[]).await.unwrap();
    w.tick().await;
    w.app.issue_close("web", 2, "not needed").await.unwrap();
    let found = found.lock().unwrap().clone();
    assert_eq!(found.len(), 2, "a doctor ran while each worker's terminal closed");
    for f in &found {
        assert_eq!(orphans(f), Vec::<String>::new(), "the worker still holds it");
    }
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

/// Orca's terminal `handle` open again, as when closing it did not take.
fn reopen(w: &World, handle: &str) {
    let mut t = w.host.orca.terminals.lock().unwrap();
    let term = t.get_mut(handle).unwrap();
    (term.alive, term.closed) = (true, false);
}

fn orphans(found: &Value) -> Vec<String> {
    let found = found["findings"].as_array().unwrap();
    found.iter().filter_map(|f| f["key"].as_str()?.strip_prefix("orphan:").map(String::from)).collect()
}

/// Seen 2026-11-18 (factorium#39): a scratch daemon on its own database
/// named the host daemon's watch terminal its orphan, by its title, and its
/// watch was to close it.
#[tokio::test]
async fn doctor_names_only_terminals_its_own_factorium_opened_that_nothing_holds() {
    let w = World::new();
    let wk = one_worker(&w).await;
    let watch = w.app.doctor().await.unwrap()["watch"]["terminal"].as_str().unwrap().to_string();
    {
        // Another daemon's, on another database: titled as this one titles its own.
        let mut t = w.host.orca.terminals.lock().unwrap();
        for (handle, title) in [("other-watch", "factorium watch"), ("other-web", "web#9"), ("notes", "notes")] {
            t.insert(handle.into(), Terminal { title: title.into(), alive: true, ..Default::default() });
        }
    }
    assert_eq!(orphans(&w.app.doctor().await.unwrap()), Vec::<String>::new(), "nothing of another's, nor held");

    // Its own worker ended, and its own watch stopped, their terminals' close not taken.
    let mine = wk["terminal"].as_str().unwrap().to_string();
    w.app.issue_close("web", 1, "not needed").await.unwrap();
    w.app.stop().await.unwrap();
    reopen(&w, &mine);
    reopen(&w, &watch);
    let found = w.app.doctor().await.unwrap();
    let mut expected = vec![mine.clone(), watch.clone()];
    expected.sort();
    let mut seen = orphans(&found);
    seen.sort();
    assert_eq!(seen, expected);
    let lost = found["findings"].as_array().unwrap().iter().find(|f| f["key"] == format!("orphan:{mine}")).unwrap();
    assert!(lost["fix"].as_str().unwrap().contains(&format!("orca terminal close --terminal {mine}")), "{lost}");

    // A new watch session holds its own terminal; the old one stays an orphan.
    w.advance(5);
    let next = w.tick().await.watch.unwrap()["watch"].as_str().unwrap().to_string();
    let seen = orphans(&w.app.doctor().await.unwrap());
    assert!(seen.contains(&watch) && !seen.contains(&next), "{seen:?}");
}
