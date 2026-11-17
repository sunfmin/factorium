//! Every harness: a worker of it starts with its own start line and brief,
//! and its session files — written by the harness's own stand-in where its
//! reader looks — say whether its turn is open or closed, and whose it is.

use factorium_harness::Harness;

use crate::{World, worker};

/// Writes the session files a harness's stand-in describes for `cwd`, in
/// the home of its sessions on `account`.
pub fn write_session(w: &World, h: Harness, account: Option<&str>, cwd: &str, open: bool, at: u64) {
    let kept = w.dir.join("data").join("kept").join(h.key());
    h.write_session(&h.home(&w.host.home, account, &kept), cwd, open, at).unwrap();
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
async fn a_login_account_s_sessions_start_in_its_home_and_their_turns_are_read_there() {
    let w = World::new();
    let account = w.host.home.join(".agent-accounts").join("night");
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
    crate::commit(&repo, "CLAUDE.md", "Always run the linter.\n");
    crate::git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
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
        crate::commit(&repo, "CLAUDE.md", "Always run the linter.\n");
        crate::git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
        w.app.checkout_add("web", &repo, "main", "proving").await.unwrap();
        // The host user's own Kimi config: its model routing is kept, the rest is not.
        let user = w.host.home.join(".kimi-code");
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
        "account.rs: fn usable",
        "account.rs: struct Usable",
        "account.rs: trait Source",
        "harness.rs: const fn key",
        "harness.rs: const fn refused",
        "harness.rs: enum Harness",
        "harness.rs: enum Stop",
        "harness.rs: fn home",
        "harness.rs: fn launch",
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
