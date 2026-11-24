//! The kit's choice of engine, model, account and command, by the domain's
//! rule alone (factorium#304: once a never-built unit test of `kit.rs`).

use std::collections::BTreeMap;

use factorium_domain::kit::{Ask, Choice, Kit, Picked, Unpicked};

fn kit(engines: &[&str]) -> Kit {
    Kit { commands: engines.iter().map(|e| (e.to_string(), format!("{e}-cmd"))).collect(), ..Kit::default() }
}

fn on(engine: &str, model: Option<&str>, account: &str) -> Picked {
    Picked::On { engine: engine.into(), model: model.map(Into::into), account: account.into() }
}

fn unpicked(engine: &str, account: Option<&str>, reason: &str) -> Unpicked {
    Unpicked { account: account.map(Into::into), why: format!("{}: {reason}", account.unwrap_or(engine)) }
}

/// A start asks its pin's engine and its issue's model, and starts what
/// the pick answers whole with the kit's command for it.
#[test]
fn a_start_is_the_pick_s_engine_model_and_account_on_the_kit_s_command() {
    let k = Kit { pins: BTreeMap::from([("web".into(), "codex".into())]), ..kit(&["codex", "claude-code"]) };
    assert_eq!(k.ask("web", Some("o3")), Ask { engines: vec!["codex".into()], model: Some("o3".into()) });
    assert_eq!(k.ask("app", None), Ask::default());
    let c = k.choose("app", None, 0, Ok(on("codex", Some("o3"), "x"))).unwrap();
    assert_eq!(
        c,
        Choice {
            harness: "codex".into(),
            command: "codex-cmd".into(),
            model: Some("o3".into()),
            account: Some("x".into())
        }
    );
    let none =
        Picked::None { until: Some("2030-01-01T00:00:00Z".into()), why: vec![unpicked("codex", Some("x"), "limit")] };
    assert_eq!(
        k.choose("app", None, 0, Ok(none)).unwrap_err(),
        "stablehand picks none until 2030-01-01T00:00:00Z (x: limit)"
    );
    let why = k.choose("app", None, 0, Ok(on("grok", None, "g"))).unwrap_err();
    assert!(why.contains("`factorium kit set grok=<command>`"), "{why}");
}

/// Without stablehand, claude-code alone on the host's own login, where
/// the pin and that login's limit allow.
#[test]
fn without_stablehand_claude_code_alone_starts_on_the_host_s_login() {
    let k = kit(&["codex", "claude-code"]);
    let fell = k.choose("web", Some("opus"), 0, Err("down".into())).unwrap();
    assert_eq!((fell.harness.as_str(), fell.model.as_deref(), fell.account), ("claude-code", Some("opus"), None));
    let spent = Kit { limits: BTreeMap::from([("claude-code".into(), 10)]), ..k.clone() };
    assert!(spent.choose("web", None, 0, Err("down".into())).unwrap_err().contains("at its limit"));
    let pinned = Kit { pins: BTreeMap::from([("web".into(), "codex".into())]), ..k.clone() };
    assert!(pinned.choose("web", None, 0, Err("down".into())).unwrap_err().starts_with("down; without stablehand"));
    assert!(kit(&["codex"]).choose("web", None, 0, Err("down".into())).is_err());
}

/// A named account starts unless its pick passed it over.
#[test]
fn a_named_account_starts_unless_stablehand_passed_it_over() {
    let k = Kit { pins: BTreeMap::from([("web".into(), "codex".into())]), ..kit(&["codex", "claude-code"]) };
    let other = on("claude-code", Some("opus"), "y");
    let c = k.choose_on("app", "x", "claude-code", None, &other).unwrap();
    assert_eq!((c.account.as_deref(), c.model.as_deref()), (Some("x"), Some("opus")));
    let passed = Picked::None { until: None, why: vec![unpicked("claude-code", Some("x"), "limit")] };
    assert_eq!(
        k.choose_on("app", "x", "claude-code", None, &passed).unwrap_err(),
        "account x takes no session now: x: limit"
    );
    assert!(k.choose_on("web", "x", "claude-code", None, &other).unwrap_err().contains("pinned to codex"));
    assert!(k.choose_on("app", "x", "grok", None, &other).unwrap_err().contains("kit set grok"));
}

/// The picker asks the other engines that ask first, then the worker's.
#[test]
fn a_picker_runs_on_another_engine_that_asks_else_on_the_worker_s() {
    let k = kit(&["a", "b", "c"]);
    let none = || Picked::None { until: None, why: Vec::new() };
    let asked = |worker: &str, asks: &[&str], first: Picked| {
        let mut seen = Vec::new();
        let mut answers = vec![first, on(worker, None, "w")].into_iter();
        let asks = |h: &str| asks.contains(&h);
        let c = k.choose_picker(worker, asks, 0, |a| {
            seen.push(a.engines.clone());
            Ok(answers.next().unwrap())
        });
        (c.map(|c| c.harness), seen)
    };
    let (c, seen) = asked("a", &["a", "b"], on("b", None, "x"));
    assert_eq!((c, seen), (Ok("b".into()), vec![vec!["b".to_string()]]));
    let (c, seen) = asked("a", &["a", "b", "c"], none());
    assert_eq!((c, seen), (Ok("a".into()), vec![vec!["b".to_string(), "c".into()], vec!["a".into()]]));
    let (c, _) = asked("a", &[], none());
    assert!(c.unwrap_err().starts_with("no harness of the kit that can pick mutants"));
}
