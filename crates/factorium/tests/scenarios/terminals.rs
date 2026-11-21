//! What factorium opens, factorium closes: a worker's terminal when the
//! worker ends, whatever ends it; the watch's on `factorium stop`. A close
//! counts once Orca no longer lists the terminal; one Orca refuses or
//! ignores is recorded, and doctor names it. Doctor names the factorium
//! terminals nothing holds. Each reads what the CLI prints.

use serde_json::{Value, json};

use crate::fixture::{CLAUDE, Terminal, World, findings, keyed, worker};

fn handle(wk: &Value) -> &str {
    wk["terminal"].as_str().unwrap()
}

async fn closed(w: &World, wk: &Value) -> bool {
    crate::fixture::closed(&w.app, handle(wk)).await
}

#[tokio::test]
async fn a_landed_worker_s_terminal_is_closed() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.done(&wk, "a.txt", "a\n").await;
    assert!(!w.tick().await.landed.is_empty());
    assert!(closed(&w, &wk).await);
}

/// Real Orca lists a terminal until its close returns, seconds for an
/// agent's: a doctor run in that time (the watch's) finds the worker still
/// holds it, so the watch never closes it in the worker's stead.
#[tokio::test]
async fn a_worker_s_terminal_is_never_an_orphan_while_it_closes() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let found = w.doctor_while_closing();
    w.done(&wk, "a.txt", "a\n").await;
    assert!(!w.tick().await.landed.is_empty());
    w.app.issue_add("web", "u", "", false, &[]).await.unwrap();
    w.tick().await;
    w.app.issue_close("web", 2, "not needed").await.unwrap();
    let found = found.lock().unwrap().clone();
    assert_eq!(found.len(), 2, "a doctor ran while each worker's terminal closed");
    for f in &found {
        assert_eq!(orphans(f), Vec::<String>::new(), "the worker still holds it");
    }
    assert!(closed(&w, &wk).await);
}

#[tokio::test]
async fn a_session_that_cannot_be_handed_its_brief_has_its_terminal_closed() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    *w.host.orca.refuse_send.lock().unwrap() = true;
    assert!(w.app.tick().await.is_err());
    let rows = w.app.workers(true).await.unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(closed(&w, &rows[0]).await, "{rows:?}");

    // Its next start fails too, and Orca will not close that terminal: the
    // refusal names the terminal left open.
    *w.host.orca.refuse_close.lock().unwrap() = true;
    let refused = format!("{:?}", w.app.tick().await.unwrap_err());
    let rows = w.app.workers(true).await.unwrap();
    let open = rows.as_array().unwrap().iter().find(|r| r["closed"] == false).expect("one left open");
    assert!(refused.contains(&format!("its terminal {} is not closed", handle(open))), "{refused}");
}

/// A start that fails once its session opened is undone: its terminal is
/// closed and its issue is free again, so the next tick starts it anew.
/// Each event the start records after the session opened is refused in
/// turn: the worker's start (Claude Code, briefed by its launch line), and
/// its brief given to its outbox (Codex, briefed by typing).
#[tokio::test]
async fn a_start_that_fails_after_its_session_opened_closes_it_and_frees_its_issue() {
    for (kit, kind) in [("claude-code=claude", "WorkerStarted"), ("codex=codex", "Given")] {
        let w = World::new();
        w.checkout("web").await;
        w.app.kit_set(&[kit.into()]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.disk_refuses(kind, true);
        assert!(w.app.tick().await.is_err(), "{kind}");
        let opened: Vec<(String, Terminal)> = w.host.orca.terminals.lock().unwrap().clone().into_iter().collect();
        let opened: Vec<_> = opened.into_iter().filter(|(_, t)| t.title == "web#1").collect();
        assert_eq!(opened.len(), 1, "{kind}: {opened:?}");
        assert!(opened[0].1.closed, "{kind}: what the start opened is closed");
        let issue = w.app.issue_show("web", 1).await.unwrap();
        assert_eq!((&issue["status"], &issue["worker"]), (&json!("ready"), &Value::Null), "{kind}: {issue}");
        assert_eq!(w.app.workers(false).await.unwrap(), json!([]), "{kind}");

        w.disk_refuses(kind, false);
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!((&wk["checkout"], &wk["number"]), (&json!("web"), &json!(1)), "{kind}: started anew");
        assert_ne!(handle(&wk), opened[0].0, "{kind}");
    }
}

#[tokio::test]
async fn stop_closes_the_watch_s_terminal_and_the_next_tick_starts_a_new_session() {
    let w = World::new();
    w.kit().await;
    let first = w.tick().await.watch.unwrap()["watch"].as_str().unwrap().to_string();
    let stopped = w.app.stop().await.unwrap();
    assert_eq!(stopped["watch_closed"], first);
    assert!(crate::fixture::closed(&w.app, &first).await);
    assert_eq!(w.app.doctor().await.unwrap()["watch"]["terminal"], Value::Null, "the watch has no session");

    w.advance(5);
    let second = w.tick().await.watch.unwrap()["watch"].as_str().unwrap().to_string();
    assert_ne!(second, first, "a new session");
    assert_eq!(w.app.doctor().await.unwrap()["watch"]["terminal"], second.as_str(), "the watch holds it");
}

/// Orca's terminal `handle` open again, as when closing it did not take.
fn reopen(w: &World, handle: &str) {
    let mut t = w.host.orca.terminals.lock().unwrap();
    let term = t.get_mut(handle).unwrap();
    (term.alive, term.closed) = (true, false);
}

fn orphans(found: &Value) -> Vec<String> {
    keyed(found, "orphan:")
}

/// Seen 2026-11-18 (factorium#39): a scratch daemon on its own database
/// named the host daemon's watch terminal its orphan, by its title, and its
/// watch was to close it.
#[tokio::test]
async fn doctor_names_only_terminals_its_own_factorium_opened_that_nothing_holds() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
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
    let lost = findings(&found).iter().find(|f| f["key"] == format!("orphan:{mine}")).unwrap();
    assert!(lost["fix"].as_str().unwrap().contains(&format!("orca terminal close --terminal {mine}")), "{lost}");

    // A new watch session holds its own terminal; the old one stays an orphan.
    w.advance(5);
    let next = w.tick().await.watch.unwrap()["watch"].as_str().unwrap().to_string();
    let seen = orphans(&w.app.doctor().await.unwrap());
    assert!(seen.contains(&watch) && !seen.contains(&next), "{seen:?}");
}

fn unclosed(found: &Value) -> Vec<(String, String)> {
    let of = |f: &Value| Some((f["key"].as_str()?.strip_prefix("unclosed:")?.to_string(), f["what"].as_str()?.into()));
    findings(found).iter().filter_map(of).collect()
}

/// A close Orca answers and does not carry out leaves the terminal listed:
/// the worker's row says why it is not closed, and doctor names it, once.
#[tokio::test]
async fn a_close_orca_ignores_is_recorded_and_named_by_doctor() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    *w.host.orca.ignore_close.lock().unwrap() = true;
    w.done(&wk, "a.txt", "a\n").await;
    assert!(!w.tick().await.landed.is_empty(), "it lands all the same");
    assert!(!closed(&w, &wk).await);
    let row = w.app.workers(true).await.unwrap()[0].clone();
    assert_eq!((row["status"].clone(), row["closed"].clone()), (json!("ended"), json!(false)));
    assert!(row["close_failed"].as_str().unwrap().contains("lists it still"), "{row}");
    let found = w.app.doctor().await.unwrap();
    let named = unclosed(&found);
    assert_eq!(named.len(), 1, "{found}");
    assert_eq!(named[0].0, handle(&wk));
    assert!(named[0].1.contains("web#1") && named[0].1.contains("lists it still"), "{named:?}");
    assert_eq!(orphans(&found), Vec::<String>::new(), "named once, as unclosed");

    // Closed by hand: Orca no longer lists it, and doctor no longer names it.
    *w.host.orca.ignore_close.lock().unwrap() = false;
    w.host.orca.terminals.lock().unwrap().get_mut(handle(&wk)).unwrap().closed = true;
    assert_eq!(unclosed(&w.app.doctor().await.unwrap()), Vec::new());
}

#[tokio::test]
async fn a_close_orca_refuses_is_recorded_and_named_by_doctor() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    *w.host.orca.refuse_close.lock().unwrap() = true;
    w.app.issue_close("web", 1, "not needed").await.unwrap();
    let row = w.app.workers(true).await.unwrap()[0].clone();
    assert_eq!(row["closed"], false);
    assert!(row["close_failed"].as_str().unwrap().contains("refused"), "{row}");
    let named = unclosed(&w.app.doctor().await.unwrap());
    assert_eq!(named.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(), [handle(&wk)]);
}

#[tokio::test]
async fn a_watch_terminal_left_open_by_stop_is_named_by_doctor() {
    let w = World::new();
    w.kit().await;
    let first = w.tick().await.watch.unwrap()["watch"].as_str().unwrap().to_string();
    *w.host.orca.ignore_close.lock().unwrap() = true;
    assert_eq!(w.app.stop().await.unwrap()["watch_closed"], first);
    let named = unclosed(&w.app.doctor().await.unwrap());
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0].0, first);
    assert!(named[0].1.contains("factorium watch"), "{named:?}");
}

#[tokio::test]
async fn a_landed_worker_whose_issue_close_the_store_refused_has_it_closed_on_the_next_tick() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    let wk = worker(&w.app).await;
    let tree = std::path::PathBuf::from(wk["worktree"].as_str().unwrap());
    w.done(&wk, "a", "x").await;
    // The worker is recorded landed; its issue's close is not, as if the daemon died there.
    w.disk_refuses("IssueClosed", true);
    assert!(w.app.tick().await.is_err(), "the tick stops at the close");
    let issue = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!((&issue["status"], &issue["worker"]), (&json!("ready"), &wk["worker"]), "open, held: {issue}");
    assert_eq!(w.app.workers(false).await.unwrap(), json!([]), "the worker ended landed");

    w.disk_refuses("IssueClosed", false);
    let t = w.app.tick().await.expect("the next tick closes it");
    assert_eq!(t.closed, vec![json!({ "closed": "web#1", "commit": t.closed[0]["commit"] })], "{:?}", t.closed);
    assert!(t.closed[0]["commit"].is_string());
    assert!(t.landed.is_empty(), "not landed again: {:?}", t.landed);
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], "closed");
    assert!(!tree.exists(), "and its worktree goes, as a landed worker's does");
    assert!(w.app.tick().await.unwrap().closed.is_empty(), "once");
}
