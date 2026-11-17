//! Every harness: a worker of it starts with its own start line and brief,
//! and its session files — written by the harness's own stand-in where its
//! reader looks — say whether its turn is open or closed, and whose it is.

use factorium_harness::Harness;

use crate::{SHELL, UP, World, id, refused, worker};
use factorium::host::Host;

/// Writes the session files a harness's stand-in describes for `cwd`, in
/// the home of its sessions on `account`.
pub fn write_session(w: &World, h: Harness, account: Option<&str>, cwd: &str, open: bool, at: u64) {
    let kept = w.dir.join("data").join("kept").join(h.key());
    h.write_session(&h.home(&w.host.home(), account, &kept), cwd, open, at).unwrap();
}

const ALL: [(&str, &str); 6] =
    [("claude-code", "claude"), ("codex", "codex"), ("pi", "pi"), ("dsh", "dsh"), ("kimi", "kimi"), ("grok", "grok")];

#[tokio::test]
async fn each_harness_starts_with_its_line_and_its_turn_is_read_from_its_own_session_files() {
    for (key, command) in ALL {
        let w = World::new();
        w.checkout("web").await;
        w.app.kit_set(&[format!("{key}={command}")]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!(wk["harness"], key);
        let term = w.terminal(wk["terminal"].as_str().unwrap());
        assert!(term.line.contains(command), "{key}: {}", term.line);
        assert!(term.line.contains("FACTORIUM_WORKER"), "{key}: the worker is named in its environment");
        let briefed = term.line.contains("web-1-1.md") || term.sent.iter().any(|s| s.contains("web-1-1.md"));
        assert!(briefed, "{key}: handed its brief");
        if !term.sent.is_empty() {
            // A typed brief waits for the session to be up: sent sooner, the shell takes it.
            let handle = wk["terminal"].as_str().unwrap();
            let ran = w.host.ran.lock().unwrap().clone();
            let at = |verb: &str| {
                ran.iter().position(|r| r.starts_with(&format!("orca terminal {verb} --terminal {handle} ")))
            };
            assert!(
                at("wait").is_some() && at("wait") < at("send"),
                "{key}: the brief is typed once its session is up"
            );
        }

        let h = Harness::read(key).unwrap();
        let cwd = wk["worktree"].as_str().unwrap();
        let now = (w.app_now() / 1000) as u64;
        write_session(&w, h, None, cwd, true, now);
        w.advance(45);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "{key}: its open turn keeps it from silence");

        // Its turn ends; another worktree's session, still open, is not its.
        let ended = (w.app_now() / 1000) as u64;
        write_session(&w, h, None, cwd, false, ended);
        write_session(&w, h, None, &format!("{cwd}-other"), true, ended + 60);
        w.advance(25);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "{key}: idle since its turn ended, within the bound");
        w.advance(10);
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!(wk["status"], "waiting", "{key}: its closed turn, past the bound, is silence: {wk}");
    }
}

#[tokio::test]
async fn an_account_secret_stays_off_the_command_line_and_its_file_is_gone_once_read() {
    let w = World::new();
    w.host.tokens.lock().unwrap().insert(
        "agent-account-team".into(),
        r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk-ant-very-secret"}}"#.into(),
    );
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let secrets = w.dir.join("data").join("secrets");
    *w.host.orca.refuse_open.lock().unwrap() = true;
    let _ = w.app.tick().await;
    assert_eq!(std::fs::read_dir(&secrets).unwrap().count(), 0, "a terminal that never opened leaves no secret");
    *w.host.orca.refuse_open.lock().unwrap() = false;
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["account"], "team");
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(!line.contains("sk-ant-very-secret"), "{line}");
    let bin = std::env::current_exe().unwrap().parent().unwrap().to_string_lossy().into_owned();
    assert!(
        line.contains(&format!("PATH='{bin}:")) || line.contains(&format!("PATH={bin}:")),
        "factorium by name: {line}"
    );
    // What the shell does with the line's start: read the secret, remove the file.
    let parts: Vec<&str> = line.splitn(3, " && ").collect();
    let file = parts[0].strip_prefix(". ").unwrap().trim_matches('\'').to_string();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let shell = format!("{} && {} && printenv CLAUDE_CODE_OAUTH_TOKEN", parts[0], parts[1]);
    let out = std::process::Command::new("sh").args(["-c", &shell]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "sk-ant-very-secret");
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
        tokens.insert(
            "agent-account-team".into(),
            r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk-ant-very-secret"}}"#.into(),
        );
    }

    let doctor = w.app.doctor().await.unwrap();
    let skipped = doctor["accounts_skipped"].as_array().unwrap();
    let why = |secret: &str| {
        let s = skipped.iter().find(|s| s["secret"] == secret).unwrap_or_else(|| panic!("{secret}: {doctor}"));
        s["why"].as_str().unwrap().to_string()
    };
    assert_eq!(skipped.len(), 3, "the token account is usable: {doctor}");
    assert!(why("agent-account-away").contains("its login is on `some-other-mac`"), "{doctor}");
    let home = w.host.home().join(".agent-accounts").join("out");
    let out = why("agent-account-out");
    assert!(out.contains("not logged in here"), "{out}");
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
    let home = w.dir.join("data").join("watch");
    let at = (w.app_now() / 1000) as u64;
    write_session(&w, Harness::Codex, Some("night"), &home.to_string_lossy(), false, at);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"]["n"], 1, "its idle turn, in the login's home, is read");
}

#[tokio::test]
async fn a_codex_worker_is_told_the_repos_rules_it_does_not_read_from_its_start_directory() {
    let w = World::new();
    let repo = w.repo("web");
    w.commit(&repo, "CLAUDE.md", "Always run the linter.\n");
    w.git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
    w.app.checkout_add("web", &repo, "main", "proving").await.unwrap();
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(line.contains("Always run the linter."), "{line}");
    assert!(line.contains(&format!("{}/CLAUDE.md", wk["worktree"].as_str().unwrap())), "{line}");
}

/// What a session's home must hold before its line runs, by harness: the
/// file, and words in it that say it asks the guard. Claude Code and Codex
/// are given their hooks on the line.
const KEPT: [(&str, &str, &str); 6] = [
    ("pi", "factorium.js", "FACTORIUM_GUARD"),
    ("dsh", "start.sh", "factorium.patch.yml"),
    ("dsh", "factorium.patch.yml", "factoriumGuard"),
    ("dsh", "factorium.mjs", "FACTORIUM_GUARD"),
    ("kimi", "config.toml", "guard kimi"),
    ("grok", "config.toml", "guard grok"),
];

#[tokio::test]
async fn each_harness_s_home_holds_its_guard_and_where_its_rules_are_before_its_session_starts() {
    for key in ["pi", "dsh", "kimi", "grok"] {
        let w = World::new();
        let repo = w.repo("web");
        w.commit(&repo, "CLAUDE.md", "Always run the linter.\n");
        w.git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
        w.app.checkout_add("web", &repo, "main", "proving").await.unwrap();
        // The host user's own Kimi config: its model routing is kept, the rest is not.
        let user = w.host.home().join(".kimi-code");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("config.toml"), "default_model = \"k2\"\nmerge_all_available_skills = true\n")
            .unwrap();
        w.app.kit_set(&[format!("{key}={key}")]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!(wk["harness"], key);
        let home = w.dir.join("data").join("kept").join(key);
        for (_, file, says) in KEPT.iter().filter(|(k, ..)| *k == key) {
            let text = std::fs::read_to_string(home.join(file)).unwrap_or_else(|e| panic!("{key}: {file}: {e}"));
            assert!(text.contains(says), "{key}: {file} says {says:?}:\n{text}");
        }
        // A home is shared by every session of its harness on its account:
        // what it holds names where this session's rules are, and holds none
        // of them.
        let worktree = wk["worktree"].as_str().unwrap();
        let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
        let role = w.dir.join("data").join("role").join("worker");
        // DSH's line is quoted once more, as one word of its start script's.
        let set = |name: &str, value: &str| {
            line.contains(&format!("{name}='{value}'")) || line.contains(&format!("{name}='\\''{value}'\\''"))
        };
        assert!(set("FACTORIUM_WORKTREE", worktree), "{key}: {line}");
        assert!(set("FACTORIUM_ADDED_DIR", &role.to_string_lossy()), "{key}: {line}");
        assert!(std::fs::read_to_string(role.join("CLAUDE.md")).unwrap().contains("You are a factorium worker"));
        let told = match key {
            "kimi" => Some("SYSTEM.md"),
            "grok" => Some("rules/told.md"),
            _ => None,
        };
        if let Some(told) = told {
            let text = std::fs::read_to_string(home.join(told)).unwrap();
            assert!(text.contains("FACTORIUM_WORKTREE") && text.contains("FACTORIUM_ADDED_DIR"), "{key}: {text}");
        }
        for file in ["config.toml", "SYSTEM.md", "rules/told.md"] {
            let text = std::fs::read_to_string(home.join(file)).unwrap_or_default();
            // Neither the worker's nor the watch's, which started after it.
            let sessions = [worktree.to_string(), w.dir.join("data").join("watch").to_string_lossy().into_owned()];
            let mine = sessions.iter().any(|dir| text.contains(dir.as_str())) || text.contains("linter");
            assert!(!mine, "{key}: {file} holds one session's:\n{text}");
        }
        if key == "kimi" {
            let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
            assert!(config.contains("k2"), "{config}");
            assert!(!config.contains("merge_all_available_skills"), "{config}");
        }
    }
}

#[tokio::test]
async fn dsh_starts_only_through_its_guard_files() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&["dsh=dsh".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let home = w.dir.join("data").join("kept").join("dsh");
    let start = home.join("start.sh");
    let (role, home_text) = (w.dir.join("data").join("role").join("worker"), home.to_string_lossy());
    let run = || {
        let args = [&start.to_string_lossy(), "echo", &home_text, &role.to_string_lossy(), "/the/worktree", "kept"];
        std::process::Command::new("sh").args(args).output().unwrap()
    };
    let out = run();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(said.contains(&format!("--patch {home_text}/factorium.patch.yml /the/worktree")), "{said}");
    std::fs::remove_file(home.join("factorium.mjs")).unwrap();
    assert!(!run().status.success(), "no guard plugin, no session");
}

/// What the harness crate offers, item by item: what factorium uses of it
/// and no more (a `pub` beneath it is public; the rest is `pub(crate)`).
/// Adding to it is adding a caller first.
#[test]
fn the_harness_crate_offers_only_what_factorium_uses() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../harness/src");
    let mut files = vec![src.clone()];
    let mut offered = Vec::new();
    while let Some(p) = files.pop() {
        if p.is_dir() {
            files.extend(std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
        } else if p.extension().is_some_and(|x| x == "rs") {
            let at = p.strip_prefix(&src).unwrap().to_string_lossy().into_owned();
            for line in std::fs::read_to_string(&p).unwrap().lines() {
                let Some(item) = line.trim_start().strip_prefix("pub ") else { continue };
                let item =
                    if item.starts_with("use ") { item } else { item.split(['(', '{', ':', '<', '=']).next().unwrap() };
                // A public struct's fields are its own.
                if item.contains(' ') {
                    offered.push(format!("{at}: {}", item.trim().trim_end_matches(';')));
                }
            }
        }
    }
    offered.sort();
    let expected = [
        "account.rs: fn here",
        "account.rs: struct Here",
        "account.rs: struct Skipped",
        "account.rs: struct Usable",
        "account.rs: type Mytokens",
        "harness.rs: const fn key",
        "harness.rs: const fn refused",
        "harness.rs: enum Harness",
        "harness.rs: enum Stop",
        "harness.rs: fn command",
        "harness.rs: fn home",
        "harness.rs: fn launch",
        "harness.rs: fn not_up",
        "harness.rs: fn read",
        "harness.rs: fn shell_call",
        "harness.rs: fn stopped",
        "harness/store.rs: fn turn",
        "harness/store.rs: fn write_session",
        "lib.rs: mod account",
        "lib.rs: use harness::{Harness, Stop}",
        "lib.rs: use start::{Home, Kept, Launch, RULES, SKILLS, Start}",
        "lib.rs: use turn::Turn",
        "start.rs: const RULES",
        "start.rs: const SKILLS",
        "start.rs: enum Kept",
        "start.rs: struct Home",
        "start.rs: struct Launch",
        "start.rs: struct Start",
        "turn.rs: enum Turn",
    ];
    assert_eq!(offered, expected);
}

#[tokio::test]
async fn a_kit_command_that_names_only_a_program_keeps_its_harness_stock_arguments() {
    for (key, program, stock) in
        [("grok", "/opt/grok/bin/grok", "--always-approve"), ("kimi", "/opt/kimi/kimi", "--auto")]
    {
        let w = World::new();
        w.checkout("web").await;
        let kit = w.app.kit_set(&[format!("{key}={program}")]).await.unwrap();
        let runs = format!("{program} {stock}");
        assert_eq!(kit["entries"][0]["command"], runs.as_str(), "{key}: kit set answers what will run");
        assert_eq!(w.app.kit().await.unwrap()["entries"][0]["command"], runs.as_str(), "{key}: so does kit show");
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let term = w.terminal(worker(&w.app).await["terminal"].as_str().unwrap());
        assert!(term.line.contains(&format!("&& {runs} ")), "{key}: {}", term.line);

        // A command with arguments of its own is taken as given.
        let given = format!("{program} --model-picker");
        let kit = w.app.kit_set(&[format!("{key}={given}")]).await.unwrap();
        assert_eq!(kit["entries"][0]["command"], given.as_str(), "{key}");
    }
}

/// A kimi worker whose terminal opens at its shell's prompt.
async fn at_shell(w: &World) -> serde_json::Value {
    w.checkout("web").await;
    w.app.kit_set(&["kimi=kimi".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    *w.host.orca.opens_at_shell.lock().unwrap() = true;
    w.tick().await;
    worker(&w.app).await
}

#[tokio::test]
async fn a_brief_is_typed_once_the_harness_is_up_never_at_its_shell() {
    let w = World::new();
    let wk = at_shell(&w).await;
    let handle = wk["terminal"].as_str().unwrap();
    assert!(w.terminal(handle).sent.is_empty(), "the shell's prompt is not typed at");
    w.advance(1);
    w.tick().await;
    assert!(w.terminal(handle).sent.is_empty(), "still the shell's");
    let told = w.app.watch_tell(id(&wk), "hello").await;
    refused(told, "not_up");
    assert!(w.terminal(handle).sent.is_empty(), "nor is the watch's word");

    w.set_screen(handle, &[SHELL, "Welcome to Kimi Code!", UP]);
    w.advance(1);
    w.tick().await;
    let sent = w.terminal(handle).sent;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].contains("web-1-1.md"), "{sent:?}");
    w.advance(1);
    w.tick().await;
    assert_eq!(w.terminal(handle).sent.len(), 1, "handed once");
    assert_eq!(worker(&w.app).await["status"], "working");
}

#[tokio::test]
async fn a_harness_that_never_comes_up_or_exits_sends_its_worker_to_the_watch() {
    let w = World::new();
    let wk = at_shell(&w).await;
    let handle = wk["terminal"].as_str().unwrap();
    w.advance(4);
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "it may yet come up");
    w.advance(2);
    w.tick().await;
    let waiting = worker(&w.app).await;
    assert_eq!(waiting["status"], "waiting");
    let why = waiting["why"].as_str().unwrap();
    assert!(why.contains("not up") && why.contains("shell's"), "{why}");
    assert!(w.terminal(handle).sent.is_empty());

    // Up, handed its brief, then its program exits back to the shell.
    let w = World::new();
    let wk = at_shell(&w).await;
    let handle = wk["terminal"].as_str().unwrap();
    w.set_screen(handle, &[UP]);
    w.tick().await;
    assert_eq!(w.terminal(handle).sent.len(), 1);
    w.set_screen(handle, &[UP, "zsh: parse error near `do'", SHELL]);
    w.advance(6);
    w.tick().await;
    assert!(worker(&w.app).await["why"].as_str().unwrap().contains("not up"));
    refused(w.app.watch_restart(id(&wk), "go on").await, "not_up");
    assert_eq!(w.terminal(handle).sent.len(), 1, "nothing more is typed at its shell");
    assert_eq!(worker(&w.app).await["status"], "waiting");
}
