//! Accounts: which a harness's sessions can start on here, how each logs in,
//! and how its secret reaches its session.

use factorium_harness::Harness;

use crate::harnesses::write_session;
use crate::one_harness::quoted_word;
use crate::{World, worker};
use factorium::host::Host;

#[tokio::test]
async fn an_account_is_usable_only_by_the_way_its_harness_logs_in() {
    let w = World::new();
    let machine = this_machine();
    let accounts = [
        ("token", r#"{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk"}"#.to_string()),
        ("bound", format!(r#"{{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk","MACHINE":"{machine}"}}"#)),
        ("tokenless", r#"{"HARNESS":"claude-code"}"#.to_string()),
        ("pi", r#"{"HARNESS":"pi","CLAUDE_CODE_OAUTH_TOKEN":"sk"}"#.to_string()),
        ("login", format!(r#"{{"HARNESS":"codex","MACHINE":"{machine}"}}"#)),
        ("login-token", format!(r#"{{"HARNESS":"codex","MACHINE":"{machine}","CLAUDE_CODE_OAUTH_TOKEN":"sk"}}"#)),
        ("nowhere", r#"{"HARNESS":"codex"}"#.to_string()),
    ];
    for (name, fields) in &accounts {
        let home = w.host.home().join(".agent-accounts").join(name);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("auth.json"), "{}").unwrap();
        w.host.tokens.lock().unwrap().insert(format!("agent-account-{name}"), format!(r#"{{"fields":{fields}}}"#));
    }
    let listed = w.app.accounts().await.unwrap();
    let mut names: Vec<&str> = listed.as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["login", "token"], "{listed}");
}

#[tokio::test]
async fn an_account_secret_stays_off_the_command_line_and_its_file_is_gone_once_read() {
    // A data directory a shell would split, or end a quote in: the secrets
    // file's path is one word of the line all the same.
    let w = World::on("it's a dir $HOME/factorium.db");
    // A secret a shell would split, expand or end a quote in: one value all the same.
    let secret = "sk-ant-it's $HOME `x` \"very\" secret";
    let fields = serde_json::json!({"fields": {"HARNESS": "claude-code", "CLAUDE_CODE_OAUTH_TOKEN": secret}});
    w.host.tokens.lock().unwrap().insert("agent-account-team".into(), fields.to_string());
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let secrets = w.data().secrets();
    *w.host.orca.refuse_open.lock().unwrap() = true;
    let refused = w.app.tick().await.unwrap_err();
    assert!(refused.message.contains("orca terminal create: terminal_create_failed"), "not the kit's: {refused:?}");
    // Refused, not unknown: its worktree is not given to Orca as a repo.
    assert!(
        !w.host.ran.lock().unwrap().iter().any(|r| r.starts_with("orca repo add --path") && r.contains("worktrees"))
    );
    assert_eq!(std::fs::read_dir(&secrets).unwrap().count(), 0, "a terminal that never opened leaves no secret");
    *w.host.orca.refuse_open.lock().unwrap() = false;
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["account"], "team");
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(!line.contains("sk-ant-"), "{line}");
    let bin = std::env::current_exe().unwrap().parent().unwrap().to_string_lossy().into_owned();
    assert!(
        line.contains(&format!("PATH='{bin}:")) || line.contains(&format!("PATH={bin}:")),
        "factorium by name: {line}"
    );
    // The kit's `claude` is named by name: no directory of its own, so no
    // empty entry, which would search the session's working directory.
    assert!(!line.contains(&format!("{bin}::")), "{line}");
    // What the shell does with the line's start: read the secret, remove the file.
    let parts: Vec<&str> = line.splitn(3, " && ").collect();
    let file = quoted_word(parts[0], 2);
    assert!(file.starts_with(&secrets.to_string_lossy().into_owned()), "{file}");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let shell = format!("{} && {} && printenv CLAUDE_CODE_OAUTH_TOKEN", parts[0], parts[1]);
    let out = std::process::Command::new("sh").args(["-c", &shell]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n'), secret);
    assert!(!std::path::Path::new(&file).exists());
}

/// This machine's name, as an account's `MACHINE` names it.
fn this_machine() -> String {
    let out = std::process::Command::new("hostname").arg("-s").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[tokio::test]
async fn doctor_says_why_each_account_cannot_be_used_here_and_sessions_start_on_the_rest() {
    let w = World::new();
    let codex = |machine: &str| format!(r#"{{"fields":{{"HARNESS":"codex","MACHINE":"{machine}"}}}}"#);
    {
        let mut tokens = w.host.tokens.lock().unwrap();
        tokens.insert("agent-account-away".into(), codex("some-other-mac"));
        tokens.insert("agent-account-out".into(), codex(&this_machine()));
        tokens.insert("agent-account-odd".into(), r#"{"fields":{"HARNESS":"codex"}}"#.into());
        tokens.insert("agent-account-blank".into(), codex(" "));
        let claude = r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"#;
        tokens.insert("agent-account-tokenless".into(), format!(r#"{claude}"  "}}}}"#));
        tokens.insert("agent-account-extra".into(), format!(r#"{claude}"t","EXTRA":"x"}}}}"#));
        tokens.insert(
            "agent-account-team".into(),
            r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk-ant-very-secret"}}"#.into(),
        );
    }

    // `out`'s home is there, but it never logged in.
    std::fs::create_dir_all(w.host.home().join(".agent-accounts").join("out")).unwrap();
    let doctor = w.app.doctor().await.unwrap();
    let skipped = doctor["accounts_skipped"].as_array().unwrap();
    let why = |secret: &str| {
        let s = skipped.iter().find(|s| s["secret"] == secret).unwrap_or_else(|| panic!("{secret}: {doctor}"));
        s["why"].as_str().unwrap().to_string()
    };
    assert_eq!(skipped.len(), 6, "the token account is usable: {doctor}");
    assert_eq!(why("agent-account-blank"), "its `MACHINE` is empty");
    assert_eq!(why("agent-account-tokenless"), "its `CLAUDE_CODE_OAUTH_TOKEN` is empty");
    assert_eq!(why("agent-account-extra"), "`EXTRA` is no field of a claude-code account");
    assert!(why("agent-account-away").contains("its login is on `some-other-mac`"), "{doctor}");
    let home = w.host.home().join(".agent-accounts").join("out");
    let out = why("agent-account-out");
    assert!(out.contains("not logged in here: no `auth.json` in"), "{out}");
    assert!(out.contains(&format!("CODEX_HOME='{}' codex", home.display())), "a login line for a person: {out}");
    assert!(why("agent-account-odd").contains("MACHINE"), "{doctor}");
    assert!(!doctor.to_string().contains("sk-ant-very-secret"));
    assert_eq!(doctor["findings"], serde_json::json!([]), "an account here or not is no finding for the watch");
    let accounts = w.app.accounts().await.unwrap();
    let names: Vec<&str> = accounts.as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["team"]);

    // Logged in, it is usable here and a session starts on it.
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("auth.json"), "{}").unwrap();
    let doctor = w.app.doctor().await.unwrap();
    assert!(doctor["accounts_skipped"].as_array().unwrap().iter().all(|s| s["secret"] != "agent-account-out"));
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    assert_eq!(worker(&w.app).await["account"], "out");
}

#[tokio::test]
async fn doctor_says_when_mytokens_lists_no_account() {
    let w = World::new();
    w.host.missing.lock().unwrap().push("mytokens".into());
    let doctor = w.app.doctor().await.unwrap();
    assert_eq!(doctor["accounts_skipped"][0]["secret"], "mytokens", "{doctor}");
}

#[tokio::test]
async fn a_login_account_s_sessions_start_in_its_home_and_their_turns_are_read_there() {
    let w = World::new();
    let account = w.host.home().join(".agent-accounts").join("night");
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(account.join("auth.json"), "{}").unwrap();
    let fields = format!(r#"{{"fields":{{"HARNESS":"codex","MACHINE":"{}"}}}}"#, this_machine());
    w.host.tokens.lock().unwrap().insert("agent-account-night".into(), fields);
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["account"], "night");
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    // One home, the login's: nothing sets it back to another.
    assert_eq!(line.matches("CODEX_HOME=").count(), 1, "{line}");
    let home = account.display();
    assert!(line.contains(&format!("CODEX_HOME='{home}'")) || line.contains(&format!("CODEX_HOME={home} ")), "{line}");

    let cwd = wk["worktree"].as_str().unwrap();
    let now = (w.app_now() / 1000) as u64;
    write_session(&w, Harness::Codex, Some("night"), cwd, true, now);
    w.advance(45);
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "its open turn, in the login's home, is read");

    // The watch on the same account: its turn is read there too.
    w.app.worker_escalate(crate::id(&wk), "which API version?").await.unwrap();
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"], serde_json::json!(null), "its session is not up yet");
    let home = w.data().watch();
    let at = (w.app_now() / 1000) as u64;
    write_session(&w, Harness::Codex, Some("night"), &home.to_string_lossy(), false, at);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"]["n"], 1, "its idle turn, in the login's home, is read");
}

#[tokio::test]
async fn only_a_well_formed_account_secret_is_an_account() {
    let w = World::new();
    let token = |t: &str| format!(r#"{{"fields":{{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"{t}"}}}}"#);
    let secrets = [
        ("agent-account-one", token("a")),
        ("agent-account-two", token("b")),
        // Not an account's secret, though it holds one's fields.
        ("deploy-key", token("c")),
        // No plain directory name.
        ("agent-account-.hidden", token("d")),
        ("agent-account-x:y", token("e")),
        // An empty token is none.
        ("agent-account-empty", token("")),
        // pi takes no account, wherever its login is.
        ("agent-account-pi", format!(r#"{{"fields":{{"HARNESS":"pi","MACHINE":"{}"}}}}"#, this_machine())),
        // A login on another machine, though logged in under this home.
        ("agent-account-away", r#"{"fields":{"HARNESS":"codex","MACHINE":"elsewhere-host"}}"#.into()),
    ];
    let away = w.host.home().join(".agent-accounts").join("away");
    std::fs::create_dir_all(&away).unwrap();
    std::fs::write(away.join("auth.json"), "{}").unwrap();
    w.host.tokens.lock().unwrap().extend(secrets.iter().map(|(n, j)| (n.to_string(), j.clone())));
    let names: Vec<_> = w.app.accounts().await.unwrap().as_array().unwrap().iter().map(|a| a["name"].clone()).collect();
    assert_eq!(names, [serde_json::json!("one"), serde_json::json!("two")]);
}

#[tokio::test]
async fn a_used_up_account_rests_an_hour_and_no_worker_starts_on_it_meanwhile() {
    let w = World::new();
    let account = w.host.home().join(".agent-accounts").join("night");
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(account.join("auth.json"), "{}").unwrap();
    let fields = format!(r#"{{"fields":{{"HARNESS":"codex","MACHINE":"{}"}}}}"#, this_machine());
    w.host.tokens.lock().unwrap().insert("agent-account-night".into(), fields);
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["account"], "night");
    w.set_screen(
        wk["terminal"].as_str().unwrap(),
        &["■ You've hit your usage limit reached.", "Increase your limits to continue using Codex."],
    );
    let at = w.app_now();
    let t = w.tick().await;
    assert_eq!(t.seen[0]["ended"], "quota");
    assert!(t.started.is_none() && t.held.as_deref().unwrap().contains("every account is used up"), "{t:?}");
    let night = w.app.accounts().await.unwrap()[0].clone();
    assert_eq!((night["left"].as_f64(), night["limited_until"].as_i64()), (Some(0.0), Some(at + 3_600_000)));
    w.advance(59);
    assert!(w.tick().await.started.is_none(), "still resting");
    w.advance(1);
    assert_eq!(w.app.accounts().await.unwrap()[0]["left"], 100.0);
    assert!(w.tick().await.started.is_some(), "rested");
}

#[tokio::test]
async fn the_watch_starts_on_its_account_s_secret() {
    let w = World::new();
    w.host.tokens.lock().unwrap().insert(
        "agent-account-team".into(),
        r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk-ant-very-secret"}}"#.into(),
    );
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    w.app.worker_escalate(crate::id(&worker(&w.app).await), "which API version?").await.unwrap();
    w.advance(5);
    w.tick().await;
    let t = w.host.orca.terminals.lock().unwrap();
    let watch = t.values().find(|t| t.title == "factorium watch").expect("the watch's terminal");
    let reads = watch.line.starts_with(". ") && watch.line.contains(&format!("{}/", w.data().secrets().display()));
    assert!(reads && !watch.line.contains("sk-ant"), "it reads its secret from a file first");
}
