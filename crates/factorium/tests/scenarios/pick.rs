//! A worker's mutants are picked by a second agent factorium starts: a
//! fresh session asked headless, not the worker's, on another harness of the
//! kit where one can be asked, given slimfit's goal, the change's diff and
//! slimfit's candidates and nothing of the worker's. What it took is kept on
//! the issue; a picker that fails, passes its bound or answers off the form
//! is refused, and nothing is kept of it.

use factorium::host::Host;
use factorium_harness::{Asked, Harness};
use serde_json::{Value, json};

use crate::fixture::{World, id, refused, worktree};

const GOAL: &str = "You are given a change's diff and its mutants.";
const KILLS: &str = "src/a.rs:1:9: replace == with != in a";
const LISTING: &str = r#"{"candidates": {"rust": [{"name": "src/a.rs:1:9: replace == with != in a"}]}}"#;

fn answered(text: &str) -> Asked {
    Asked { session: "s-1".into(), model: "claude-opus-5-5".into(), text: text.into(), tokens_in: 1200, tokens_out: 80 }
}

/// A checkout `web` whose kit is `first` then the stand-in Claude Code
/// picker, which runs `then` and answers `text`; a worker on issue 1, on
/// the kit's first harness, that changed `a.rs`. Answers the world, the
/// worker and the picker.
async fn changed(first: Option<&str>, then: &str, text: &str) -> (World, Value, crate::fixture::Picker) {
    let w = World::new();
    w.checkout("web").await;
    w.slimfit(GOAL, LISTING);
    let picker = w.picker(Harness::ClaudeCode, then, &answered(text));
    let claude = format!("claude-code={}", picker.program.display());
    let kit: Vec<String> = first.map(String::from).into_iter().chain([claude]).collect();
    w.app.kit_set(&kit).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = crate::fixture::worker(&w.app).await;
    w.commit(&worktree(&wk), "a.rs", "fn a(x: u8) -> bool { x == 1 }\n");
    (w, wk, picker)
}

/// The comments on issue 1 by factorium.
async fn kept(w: &World) -> Vec<Value> {
    let shown = w.app.issue_show("web", 1).await.unwrap();
    shown["comments"].as_array().unwrap().iter().filter(|c| c["by"] == "factorium").cloned().collect()
}

#[tokio::test]
async fn a_worker_s_mutants_are_picked_by_a_fresh_session_on_the_kit_s_other_harness() {
    let picks = r#"{"picks": [{"name": "src/a.rs:1:9: replace == with != in a", "why": "the one decision"}]}"#;
    let cases = [Some("pi=pi"), None];
    crate::fixture::each(&cases, |first| async move {
        let (w, wk, picker) = changed(*first, "", picks).await;
        let on = if first.is_some() { "pi" } else { "claude-code" };
        assert_eq!(wk["harness"], on);
        // The worker's own harness runs on an account: so does its picker, its secret by a file.
        let account = if first.is_some() { "the host's own login" } else { "team" };
        let fields = json!({"fields": {"HARNESS": "claude-code", "CLAUDE_CODE_OAUTH_TOKEN": "sk-it's"}});
        if first.is_none() {
            w.host.tokens.lock().unwrap().insert("agent-account-team".into(), fields.to_string());
        }
        let answer = w.app.worker_pick_mutants(id(&wk)).await.unwrap();

        // Run on Claude Code: the other harness, or the worker's own anew.
        let picker_id = "claude-code:s-1:claude-opus-5-5";
        assert_eq!(answer["picker"], picker_id, "{answer}");
        assert_eq!(answer["author"], format!("{on}:{}:default", id(&wk)));
        assert_eq!(answer["picks"], json!([{ "name": KILLS, "why": "the one decision" }]));
        assert_eq!(answer["tokens"], json!({ "in": 1200, "out": 80 }));
        let next = answer["next"].as_str().unwrap();
        assert!(next.contains(&format!("slimfit check --mutants '{KILLS}' --why 'the one decision'")), "{next}");
        assert!(next.contains(&format!("slimfit ratchet --picker '{picker_id}' --author")), "{next}");

        // Asked the goal, the diff and the candidates, headless, with
        // nothing of the worker's session in its environment.
        let (asked, args, env) = picker.asked();
        assert!(asked.starts_with(GOAL), "{asked}");
        assert!(asked.contains("+fn a(x: u8) -> bool { x == 1 }") && asked.contains(LISTING), "{asked}");
        assert!(args.lines().any(|a| a == "-p"), "{args}");
        let names: Vec<&str> = env.lines().filter_map(|l| l.split_once('=')).map(|(n, _)| n).collect();
        let theirs = ["HOME", "PATH", "PWD", "SHLVL", "_", "OLDPWD", "CLAUDE_CODE_OAUTH_TOKEN"];
        assert_eq!(env.contains("CLAUDE_CODE_OAUTH_TOKEN=sk-it's\n"), first.is_none(), "{env}");
        assert!(!args.contains("sk-"), "{args}");
        assert!(names.iter().all(|n| theirs.contains(n)), "only its home and PATH: {names:?}");

        let kept = kept(&w).await;
        assert_eq!(kept.len(), 1, "{kept:?}");
        let text = kept[0]["text"].as_str().unwrap();
        assert!(text.starts_with(&format!("pick-mutants: {picker_id} on {account}, ")), "{text}");
        let secs = answer["took_ms"].as_u64().unwrap() as f64 / 1000.0;
        assert!(text.ends_with(&format!(", {secs:.1}s, 1200 tokens in, 80 out; 1 picked")), "{text}");
    });
}

#[tokio::test]
async fn a_picker_that_says_none_is_worth_one_answers_the_record_of_that() {
    let (w, wk, _) = changed(None, "", "```json\n{\"none\": \"only a log line\"}\n```").await;
    let answer = w.app.worker_pick_mutants(id(&wk)).await.unwrap();
    assert_eq!(answer["none"], "only a log line");
    assert!(answer["next"].as_str().unwrap().contains("--no-mutants 'only a log line'"), "{answer}");
}

#[tokio::test]
async fn a_picker_that_fails_passes_its_bound_or_answers_off_the_form_is_refused_and_nothing_is_kept() {
    let bad = |name: &str| format!(r#"{{"picks": [{{"name": "{name}", "why": "w"}}]}}"#);
    let six: Vec<String> = (0..6).map(|_| format!(r#"{{"name": "{KILLS}", "why": "w"}}"#)).collect();
    let cases: Vec<(String, String, &str)> = vec![
        ("exit 3".into(), bad(KILLS), "picker_failed"),
        ("sleep 30".into(), bad(KILLS), "picker_timed_out"),
        (String::new(), bad("src/a.rs:9:9: not listed"), "picker_answer"),
        (String::new(), format!(r#"{{"picks": [{}]}}"#, six.join(",")), "picker_answer"),
        (String::new(), r#"{"picks": [], "none": "two\nlines"}"#.into(), "picker_answer"),
        (String::new(), "I would pick the comparison.".into(), "picker_answer"),
        (String::new(), format!(r#"{{"picks": [{{"name": "{KILLS}", "why": " "}}]}}"#), "picker_answer"),
        (String::new(), "} I would pick { nothing".into(), "picker_answer"),
    ];
    crate::fixture::each(&cases, |(then, text, code)| async move {
        let (w, wk, _) = changed(None, then, text).await;
        w.app.settings_set(Some("pick_bound"), Some(1)).await.unwrap();
        refused(w.app.worker_pick_mutants(id(&wk)).await, code);
        assert_eq!(kept(&w).await, Vec::<Value>::new(), "{code}: nothing is kept of it");
        assert!(!worktree(&wk).join(".slimfit").exists(), "{code}");
    });
}

#[tokio::test]
async fn a_kit_with_no_harness_that_can_be_asked_refuses_to_pick() {
    let w = World::new();
    let wk = w.working("pi=pi", "t").await;
    w.slimfit(GOAL, LISTING);
    refused(w.app.worker_pick_mutants(id(&wk)).await, "no_harness");
}

const PICKS: &str = r#"{"picks": [{"name": "src/a.rs:1:9: replace == with != in a", "why": "the one decision"}]}"#;

/// A checkout `web` whose kit is the stand-in Claude Code picker, answering
/// [`PICKS`], then a stand-in picker of `other` on `model`, which runs `then`
/// and answers `picks` as that harness does; the host user logged in to
/// `other`; a worker on Claude Code that changed `a.rs`.
async fn beside(other: Harness, model: &str, then: &str, picks: &str) -> (World, Value, crate::fixture::Picker) {
    let w = World::new();
    w.checkout("web").await;
    w.slimfit(GOAL, LISTING);
    let claude = w.picker(Harness::ClaudeCode, "", &answered(PICKS));
    let theirs = Asked { session: "t-1".into(), model: model.into(), text: picks.into(), tokens_in: 900, tokens_out: 40 };
    let picker = w.picker(other, then, &theirs);
    let login = w.host.home().join(format!(".{}", other.key())).join("auth.json");
    std::fs::create_dir_all(login.parent().unwrap()).unwrap();
    std::fs::write(&login, "{}").unwrap();
    let kit = [
        format!("claude-code={}@claude-opus-5-5", claude.program.display()),
        format!("{}={}@{model}", other.key(), picker.program.display()),
    ];
    w.app.kit_set(&kit).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = crate::fixture::worker(&w.app).await;
    w.commit(&worktree(&wk), "a.rs", "fn a(x: u8) -> bool { x == 1 }\n");
    (w, wk, picker)
}

#[tokio::test]
async fn an_opus_worker_s_mutants_are_picked_on_codex_or_grok_and_the_record_names_it_and_its_model() {
    let cases = [(Harness::Codex, "gpt-6.2-sol", "CODEX_HOME"), (Harness::Grok, "grok-4.7-build", "GROK_HOME")];
    crate::fixture::each(&cases, |(other, model, user_dir)| async move {
        let (w, wk, picker) = beside(*other, model, "", PICKS).await;
        assert_eq!(wk["harness"], "claude-code");
        let answer = w.app.worker_pick_mutants(id(&wk)).await.unwrap();

        let picker_id = format!("{}:t-1:{model}", other.key());
        assert_eq!(answer["picker"], picker_id, "{answer}");
        assert_eq!((answer["harness"].as_str(), answer["model"].as_str()), (Some(other.key()), Some(*model)));
        assert_eq!(answer["author"], format!("claude-code:{}:claude-opus-5-5", id(&wk)));
        assert_eq!(answer["fell_back"], json!([]));
        assert_eq!(answer["tokens"], json!({ "in": 900, "out": 40 }));

        // Asked on stdin, in a user directory of its own holding its login
        // and nothing of the host's, with nothing of the worker's session.
        let (asked, _, env) = picker.asked();
        assert!(asked.starts_with(GOAL) && asked.contains(LISTING), "{asked}");
        let names: Vec<&str> = env.lines().filter_map(|l| l.split_once('=')).map(|(n, _)| n).collect();
        let theirs = ["HOME", "PATH", "PWD", "SHLVL", "_", "OLDPWD", user_dir];
        assert!(names.iter().all(|n| theirs.contains(n)), "{names:?}");
        let held: Vec<String> = picker.user_dir().lines().map(String::from).collect();
        let expected: Vec<&str> = if *other == Harness::Grok { vec!["auth.json", "config.toml"] } else { vec!["auth.json"] };
        assert_eq!(held, expected);

        let kept = kept(&w).await;
        assert!(kept[0]["text"].as_str().unwrap().starts_with(&format!("pick-mutants: {picker_id} on ")), "{kept:?}");
    });
}

#[tokio::test]
async fn a_picker_on_another_harness_that_fails_hands_the_pick_to_claude_code_and_the_record_says_so() {
    let cases = [("exit 3", PICKS), ("", "You have hit your usage limit.")];
    crate::fixture::each(&cases, |(then, said)| async move {
        let (w, wk, _) = beside(Harness::Codex, "gpt-6.2-sol", then, said).await;
        let answer = w.app.worker_pick_mutants(id(&wk)).await.unwrap();

        assert_eq!(answer["picker"], "claude-code:s-1:claude-opus-5-5", "{answer}");
        let fell = answer["fell_back"].as_array().unwrap();
        assert_eq!(fell.len(), 1, "{answer}");
        assert_eq!(fell[0]["harness"], "codex");
        assert!(!fell[0]["why"].as_str().unwrap().is_empty(), "{answer}");
        let kept = kept(&w).await;
        let text = kept[0]["text"].as_str().unwrap();
        assert!(text.contains("; the codex picker failed first, so claude-code picked: "), "{text}");
    });
}
