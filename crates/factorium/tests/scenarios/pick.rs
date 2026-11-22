//! A worker's mutants are picked, and its growth judged, by a second agent factorium starts: a
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

/// Whether no secrets file is left where factorium writes them.
fn no_secrets_left(w: &World) -> bool {
    std::fs::read_dir(w.data().secrets()).map_or(true, |mut d| d.next().is_none())
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
        assert!(no_secrets_left(&w), "no secrets file outlives the ask");

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
async fn a_change_slimfit_lists_no_candidates_of_starts_no_picker_and_records_none_with_why() {
    // A change of tests and config alone, as slimfit answers it, and as it
    // refuses one of examples (slimfit#77).
    let refused = r#"{"error": [{"fault": "suite rust: its mutation tool listed no mutants of the change; the command's log ends:\n INFO No mutants to filter", "fix": "f"}]}"#;
    let cases = [r#"{"candidates": {"rust": []}, "next": "n"}"#, refused];
    crate::fixture::each(&cases, |listing| async move {
        let w = World::new();
        w.checkout("web").await;
        w.slimfit(GOAL, listing);
        let picker = w.picker(Harness::ClaudeCode, "", &answered(PICKS));
        w.app.kit_set(&[format!("claude-code={}", picker.program.display())]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = crate::fixture::worker(&w.app).await;
        w.commit(&worktree(&wk), "a_test.rs", "#[test]\nfn a() {}\n");
        w.commit(&worktree(&wk), "slimfit.toml", "[check]\n");
        let answer = w.app.worker_pick_mutants(id(&wk)).await.unwrap();

        assert_eq!(answer["none"], "slimfit lists no candidates", "{answer}");
        assert_eq!(answer["picker"], Value::Null, "nothing pretends a second agent judged it: {answer}");
        assert_eq!(answer["author"], format!("claude-code:{}:default", id(&wk)));
        let next = answer["next"].as_str().unwrap();
        assert!(next.contains("slimfit ratchet;") && !next.contains("--picker"), "{next}");
        assert_eq!(picker.asked(), Default::default(), "no session was started");
        let kept = kept(&w).await;
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0]["text"], "pick-mutants: no picker asked; slimfit lists no candidates");

        // Nothing more to record: the change lands, its gate green.
        w.app.worker_done(id(&wk)).await.unwrap();
        let tick = w.tick().await;
        assert!(tick.landed.iter().any(|l| l["landed"] == "web#1"), "{tick:?}");
    });
}

#[tokio::test]
async fn a_listing_slimfit_refuses_otherwise_is_refused_and_no_picker_is_asked() {
    let (w, wk, picker) = changed(None, "", PICKS).await;
    w.slimfit(GOAL, r#"{"error": [{"fault": "no target found", "fix": "f"}]}"#);
    refused(w.app.worker_pick_mutants(id(&wk)).await, "slimfit");
    assert_eq!(picker.asked(), Default::default());
    assert_eq!(kept(&w).await, Vec::<Value>::new());
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
        let fields = json!({"fields": {"HARNESS": "claude-code", "CLAUDE_CODE_OAUTH_TOKEN": "sk-1"}});
        w.host.tokens.lock().unwrap().insert("agent-account-team".into(), fields.to_string());
        w.app.settings_set(Some("pick_bound"), Some(1)).await.unwrap();
        refused(w.app.worker_pick_mutants(id(&wk)).await, code);
        assert_eq!(kept(&w).await, Vec::<Value>::new(), "{code}: nothing is kept of it");
        assert!(no_secrets_left(&w), "{code}: no secrets file outlives the ask");
        assert!(!worktree(&wk).join(".slimfit").exists(), "{code}");
    });
}

#[tokio::test]
async fn on_a_grouped_listing_a_pick_slimfit_lists_only_under_its_function_is_taken() {
    // slimfit#99: past its budget a listing spells out some candidates of
    // each function; `--function` lists one function's in full.
    const ALSO: &str = "src/a.rs:1:22: replace a -> bool with true";
    let grouped = json!({
        "candidates": {"rust": [{"name": KILLS, "file": "src/a.rs", "function": "a"}]},
        "functions": {"rust": [{"file": "src/a.rs", "function": "a", "mutants": 2, "removed": []},
                               {"file": "src/b.rs", "function": "b", "mutants": 1, "removed": []}]},
        "removed_tests": {"rust": []}
    });
    let full = json!({"candidates": {"rust": [{"name": KILLS}, {"name": ALSO}]}});
    let pick = |name: &str| format!(r#"{{"picks": [{{"name": "{name}", "why": "w"}}]}}"#);
    let cases = [(ALSO, true), ("src/a.rs:9:9: not listed", false)];
    crate::fixture::each(&cases, |(name, taken)| {
        let (grouped, full, text) = (grouped.to_string(), full.to_string(), pick(name));
        async move {
            let (w, wk, picker) = changed(None, "", &text).await;
            let fields = json!({"fields": {"HARNESS": "claude-code", "CLAUDE_CODE_OAUTH_TOKEN": "sk-1"}});
            w.host.tokens.lock().unwrap().insert("agent-account-team".into(), fields.to_string());
            let dir = worktree(&wk).display().to_string();
            {
                let mut said = w.host.slimfit.lock().unwrap();
                said.insert("check".into(), grouped.clone());
                // Only the function the pick's file holds is asked for.
                said.insert(format!("check --repo {dir} --mutants --function src/a.rs: a"), full);
            }
            let answer = w.app.worker_pick_mutants(id(&wk)).await;
            if *taken {
                assert_eq!(answer.unwrap()["picks"], json!([{ "name": name, "why": "w" }]));
            } else {
                refused(answer, "picker_answer");
            }
            // The prompt keeps the grouped listing as slimfit gave it.
            assert!(picker.asked().0.contains(&grouped), "{name}");
        }
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
/// `claude_says`, then a stand-in of `other` on `model`, which runs `then`
/// and answers `picks` as that harness does; slimfit's reviewer goal
/// [`REVIEW_GOAL`]; the host user logged in to
/// `other`; a worker on Claude Code that changed `a.rs`.
async fn beside(
    other: Harness,
    model: &str,
    then: &str,
    picks: &str,
    claude_says: &str,
) -> (World, Value, crate::fixture::Picker) {
    let w = World::new();
    w.checkout("web").await;
    w.slimfit(GOAL, LISTING);
    let claude = w.picker(Harness::ClaudeCode, "", &answered(claude_says));
    w.host.slimfit.lock().unwrap().insert("help reviewer".into(), REVIEW_GOAL.into());
    let theirs =
        Asked { session: "t-1".into(), model: model.into(), text: picks.into(), tokens_in: 900, tokens_out: 40 };
    let picker = w.picker(other, then, &theirs);
    let login = match other {
        Harness::Agy => w.host.home().join(".gemini/antigravity-cli/antigravity-oauth-token"),
        _ => w.host.home().join(format!(".{}", other.key())).join("auth.json"),
    };
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
async fn an_opus_worker_s_mutants_are_picked_on_codex_grok_or_agy_and_the_record_names_it_and_its_model() {
    let cases = [
        (Harness::Codex, "gpt-6.2-sol", "CODEX_HOME"),
        (Harness::Grok, "grok-4.7-build", "GROK_HOME"),
        (Harness::Agy, "gemini-4-neon-high", "HOME"),
    ];
    crate::fixture::each(&cases, |(other, model, user_dir)| async move {
        let (w, wk, picker) = beside(*other, model, "", PICKS, PICKS).await;
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
        let expected: Vec<&str> = match other {
            Harness::Grok => vec!["auth.json", "config.toml"],
            Harness::Agy => {
                vec![".gemini/antigravity-cli/antigravity-oauth-token", ".gemini/config/agents/factorium-ask.md"]
            }
            _ => vec!["auth.json"],
        };
        assert_eq!(held, expected);

        let kept = kept(&w).await;
        assert!(kept[0]["text"].as_str().unwrap().starts_with(&format!("pick-mutants: {picker_id} on ")), "{kept:?}");
    });
}

#[tokio::test]
async fn a_picker_on_another_harness_that_fails_hands_the_pick_to_claude_code_and_the_record_says_so() {
    let cases = [("exit 3", PICKS), ("", "You have hit your usage limit.")];
    crate::fixture::each(&cases, |(then, said)| async move {
        let (w, wk, _) = beside(Harness::Codex, "gpt-6.2-sol", then, said, PICKS).await;
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

const REVIEW_GOAL: &str = "You are given a change's diff, nothing else.";
const AS_IS: &str = r#"{"as_is": "a new function nothing else does"}"#;

#[tokio::test]
async fn a_worker_s_growth_is_judged_on_the_kit_s_other_harness_and_the_record_names_it() {
    let (w, wk, reviewer) = beside(Harness::Codex, "gpt-6.2-sol", "", AS_IS, PICKS).await;
    let answer = w.app.worker_review_growth(id(&wk)).await.unwrap();

    let (reviewer_id, author) = ("codex:t-1:gpt-6.2-sol", format!("claude-code:{}:claude-opus-5-5", id(&wk)));
    assert_eq!(answer["reviewer"], reviewer_id, "{answer}");
    assert_eq!((answer["harness"].as_str(), answer["model"].as_str()), (Some("codex"), Some("gpt-6.2-sol")));
    assert_eq!(answer["as_is"], "a new function nothing else does");
    assert_eq!(answer["fell_back"], json!([]));
    // The command that writes the growth record names the reviewer, its harness and model, and the issue.
    let next = answer["next"].as_str().unwrap();
    let ids = format!("--as-is 'a new function nothing else does' --reviewer '{reviewer_id}' --author '{author}'");
    assert!(next.contains("slimfit ratchet --serves 'web#1' --considered ") && next.contains(&ids), "{next}");

    // Given the goal and the diff, nothing of slimfit's candidates nor of the worker's.
    let (asked, _, env) = reviewer.asked();
    assert!(asked.starts_with(REVIEW_GOAL) && asked.contains("+fn a(x: u8) -> bool { x == 1 }"), "{asked}");
    assert!(!asked.contains(LISTING), "{asked}");
    let names: Vec<&str> = env.lines().filter_map(|l| l.split_once('=')).map(|(n, _)| n).collect();
    assert!(
        names.iter().all(|n| ["HOME", "PATH", "PWD", "SHLVL", "_", "OLDPWD", "CODEX_HOME"].contains(n)),
        "{names:?}"
    );

    let kept = kept(&w).await;
    let text = kept[0]["text"].as_str().unwrap();
    assert!(text.starts_with(&format!("review-growth: {reviewer_id} on ")), "{text}");
    assert!(text.ends_with(", 900 tokens in, 40 out; as-is"), "{text}");
}

#[tokio::test]
async fn a_reviewer_on_another_harness_that_fails_hands_the_review_to_claude_code_and_the_record_says_so() {
    let cut = r#"{"cut": "a.rs: reuse b instead of a"}"#;
    let cases = [
        ("exit 3", AS_IS),
        ("", "You have hit your usage limit."),
        ("", r#"{"as_is": "two\nlines"}"#),
        ("", r#"{"cut": " "}"#),
    ];
    crate::fixture::each(&cases, |(then, said)| async move {
        let (w, wk, _) = beside(Harness::Codex, "gpt-6.2-sol", then, said, cut).await;
        let answer = w.app.worker_review_growth(id(&wk)).await.unwrap();

        assert_eq!(answer["reviewer"], "claude-code:s-1:claude-opus-5-5", "{answer}");
        assert_eq!(answer["fell_back"].as_array().unwrap()[0]["harness"], "codex", "{answer}");
        assert_eq!(answer["cut"], "a.rs: reuse b instead of a");
        let next = answer["next"].as_str().unwrap();
        assert!(
            next.contains("--cut 'a.rs: reuse b instead of a' --answer ")
                && next.contains("--reviewer 'claude-code:s-1:"),
            "{next}"
        );
        let kept = kept(&w).await;
        let text = kept[0]["text"].as_str().unwrap();
        assert!(text.contains("; cut; the codex reviewer failed first, so claude-code reviewed: "), "{text}");
    });
}

#[tokio::test]
async fn a_review_no_reviewer_answers_as_asked_is_refused_and_nothing_is_kept() {
    let (w, wk, _) = beside(Harness::Codex, "gpt-6.2-sol", "exit 3", AS_IS, "It could be smaller.").await;
    refused(w.app.worker_review_growth(id(&wk)).await, "reviewer_answer");
    assert_eq!(kept(&w).await, Vec::<Value>::new());
}

/// What a stand-in on another harness answers when asked again, as it
/// answered first: `t-1`, on `model`.
fn again(model: &str, text: &str) -> Asked {
    Asked { session: "t-1".into(), model: model.into(), text: text.into(), tokens_in: 300, tokens_out: 20 }
}

#[tokio::test]
async fn a_second_agent_that_answers_prose_is_asked_once_more_and_its_json_then_taken_with_no_fallback() {
    let prose = "I'll read the rest of the candidates first.";
    let cases = ["picker", "reviewer"];
    crate::fixture::each(&cases, |role| async move {
        // Each ask takes 0.3s at least.
        let (w, wk, grok) = beside(Harness::Grok, "grok-4.7-build", "sleep 0.3", prose, PICKS).await;
        let json = if *role == "picker" { PICKS } else { AS_IS };
        grok.then_answers(Harness::Grok, &again("grok-4.7-build", json));
        let began = std::time::Instant::now();
        let answer = match *role {
            "picker" => w.app.worker_pick_mutants(id(&wk)).await,
            _ => w.app.worker_review_growth(id(&wk)).await,
        }
        .unwrap();
        // Both asks' time, summed: 0.6s at least, and no more than the whole call took.
        let took_ms = answer["took_ms"].as_u64().unwrap();
        assert!(took_ms >= 600 && u128::from(took_ms) <= began.elapsed().as_millis(), "{answer}");

        // Taken of grok, asked again, both asks' tokens counted.
        assert_eq!(answer[*role], "grok:t-1:grok-4.7-build", "{answer}");
        assert_eq!((answer["fell_back"].clone(), answer["asked_again"].clone()), (json!([]), json!(true)));
        assert_eq!(answer["tokens"], json!({ "in": 1200, "out": 60 }));

        // Asked again in the same session: only what was wrong and the one form.
        let (asked, args) = grok.asked_again();
        let wrong = format!("Your answer was not taken: the {role} answered no JSON object.\n\n# How to answer\n\n");
        assert!(asked.starts_with(&wrong) && asked.contains("Answer one JSON object and nothing else: {"), "{asked}");
        assert!(!asked.contains(GOAL) && !asked.contains("+fn a("), "{asked}");
        assert!(args.contains("--resume\nt-1\n"), "{args}");

        let kept = kept(&w).await;
        let text = kept[0]["text"].as_str().unwrap();
        assert!(
            text.contains(", 1200 tokens in, 60 out; ") && text.ends_with(", answered as asked when asked again"),
            "{text}"
        );
    });
}

#[tokio::test]
async fn a_second_agent_that_answers_prose_twice_hands_the_question_to_the_next_harness() {
    let prose = "No mutant is worth running: the diff edits no production code.";
    let off =
        |role: &str| format!("the {role} answered no JSON object; asked again: the {role} answered no JSON object");
    // Its stand-in knows it is run again by `$n`; one that fails then fails the second ask.
    let cases = [
        ("picker", PICKS, "", off("picker")),
        ("reviewer", AS_IS, "", off("reviewer")),
        (
            "picker",
            PICKS,
            "[ -n \"$n\" ] && exit 3",
            "the picker answered no JSON object; asked again: the grok picker exited Some(3)".into(),
        ),
    ];
    crate::fixture::each(&cases, |(role, claude_says, then, twice)| async move {
        let (w, wk, grok) = beside(Harness::Grok, "grok-4.7-build", then, prose, claude_says).await;
        let answer = match *role {
            "picker" => w.app.worker_pick_mutants(id(&wk)).await,
            _ => w.app.worker_review_growth(id(&wk)).await,
        }
        .unwrap();

        assert!(grok.asked_again().0.starts_with("Your answer was not taken: "), "{:?}", grok.asked_again());
        assert_eq!(answer[*role], "claude-code:s-1:claude-opus-5-5", "{answer}");
        // Claude Code took it at the first ask.
        assert_eq!(answer["asked_again"], false);
        assert!(answer["fell_back"][0]["why"].as_str().unwrap().starts_with(twice.as_str()), "{answer}");
        let kept = kept(&w).await;
        let text = kept[0]["text"].as_str().unwrap();
        let did = if *role == "picker" { "picked" } else { "reviewed" };
        assert!(text.contains(&format!("; the grok {role} failed first, so claude-code {did}: {twice}")), "{text}");
        assert!(!text.contains("when asked again"), "{text}");
    });
}
