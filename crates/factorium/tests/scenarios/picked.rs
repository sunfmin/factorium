//! A start is what `stablehand pick` answers whole (factorium#291): its
//! engine, model and account, asked once, narrowed by a checkout's pin
//! and an issue's own model; each engine starts by the command stablehand
//! keeps for it.

use factorium_domain::issue::On;

use crate::accounts::this_machine;
use crate::fixture::{CLAUDE, World, worker};

/// The `stablehand pick` lines run, oldest first.
fn picks(w: &World) -> Vec<String> {
    let ran = w.host.ran.lock().unwrap();
    ran.iter().filter_map(|r| r.strip_prefix("stablehand pick ")).map(str::to_string).collect()
}

/// A checkout `web` and a kit of Claude Code and Codex, stablehand walking
/// `ranks` with an account of each engine.
async fn world(ranks: &[&str]) -> World {
    let w = World::new();
    w.checkout("web").await;
    w.kit_of(&[CLAUDE, "codex=codex"]).await;
    w.stablehand_keeps(ranks);
    w
}

#[tokio::test]
async fn a_start_is_the_engine_model_and_account_stablehand_picks_asked_once() {
    let w = world(&["codex@gpt-6.2-sol", "claude-code@opus"]).await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!((wk["harness"].as_str(), wk["account"].as_str()), (Some("codex"), Some("codex")));
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(line.contains("codex") && line.contains("gpt-6.2-sol"), "its model: {line}");
    // Each ask is one pick of the whole order, never one per engine of the kit.
    let picks = picks(&w);
    assert_eq!(picks[0], format!("--machine {}", this_machine()), "{picks:?}");
    assert!(picks.iter().all(|p| !p.contains("--engine")), "{picks:?}");
}

#[tokio::test]
async fn a_pinned_checkout_asks_the_pick_of_its_engine_alone() {
    let w = world(&["codex", "claude-code@opus"]).await;
    w.app.kit_pin("web", Some("claude-code")).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["harness"], "claude-code");
    assert!(w.terminal(wk["terminal"].as_str().unwrap()).line.contains("opus"));
    let pick = format!("--machine {} --engine claude-code", this_machine());
    assert!(picks(&w).contains(&pick), "{:?}", picks(&w));
}

#[tokio::test]
async fn an_issue_s_own_model_is_asked_of_the_pick() {
    let w = world(&["codex", "claude-code@opus"]).await;
    let on = On { model: Some("sonnet".into()), ..On::default() };
    w.app.issue_add_on("web", "t", "", None, &[], on).await.unwrap();
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["model"], "sonnet");
    w.tick().await;
    let wk = worker(&w.app).await;
    let pick = format!("--machine {} --model sonnet", this_machine());
    assert!(picks(&w).contains(&pick), "{:?}", picks(&w));
    // Not in the order: tried on each engine after the rest, codex first.
    assert_eq!(wk["harness"], "codex");
    assert!(w.terminal(wk["terminal"].as_str().unwrap()).line.contains("sonnet"));
}

#[tokio::test]
async fn while_the_pick_answers_none_the_issue_waits_until_its_until() {
    let w = world(&["codex"]).await;
    let until = w.app_now() + 2 * 3_600_000;
    w.host.stablehand.lock().unwrap().limits.insert("codex".into(), Some(until));
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let t = w.tick().await;
    assert!(t.started.is_none(), "{t:?}");
    let held = t.held.unwrap();
    assert!(held.contains(&format!("until {}", factorium_domain::when::utc(until))), "{held}");
    w.advance(60);
    assert!(w.tick().await.started.is_none(), "an hour on, still at its limit");
    w.advance(61);
    assert!(w.tick().await.started.is_some(), "its until passed, it starts");
}

/// A session starts by the command stablehand keeps for the engine it
/// picks (`stablehand command show`), then factorium's own arguments; an
/// engine it keeps none for starts on its bare program, with its stock
/// arguments (factorium#306).
#[tokio::test]
async fn a_session_starts_by_stablehand_s_command_else_its_engine_s_bare_program() {
    let w = World::new();
    w.checkout("web").await;
    w.kit_of(&["claude-code=claude --x"]).await;
    w.stablehand_keeps(&["claude-code", "grok"]);
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    assert!(w.tick().await.started.is_some());
    let line = w.terminal(worker(&w.app).await["terminal"].as_str().unwrap()).line;
    assert!(line.contains("&& claude --x --add-dir"), "stablehand's command, then factorium's: {line}");
    assert!(!line.contains("--dangerously-skip-permissions"), "{line}");
    assert!(w.host.ran.lock().unwrap().iter().any(|r| r == "stablehand command show"));

    // grok first, with no command kept: its bare program.
    w.stablehand_keeps(&["grok", "claude-code"]);
    w.app.issue_add("web", "u", "", false, &[]).await.unwrap();
    assert!(w.tick().await.started.is_some());
    let wk = w.worker_on("web", 2).await;
    assert_eq!(wk["harness"], "grok");
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(line.contains("grok --always-approve"), "its bare program, stock arguments: {line}");
    assert!(w.finding("kit:no_command:grok").await.is_none(), "no kept command is no fault");
}
