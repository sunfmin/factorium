//! Every harness: a worker of it starts with its own start line and brief,
//! and its session files — written by the harness's own stand-in — say
//! whether its turn is open.

use factorium_harness::Harness;

use crate::{World, worker};

/// Writes the session files a harness's stand-in describes for `cwd`.
pub fn write_session(w: &World, h: Harness, cwd: &str, open: bool, at: u64) {
    let kept = w.dir.join("data").join("kept").join(h.key());
    h.write_session(&w.host.home, &kept, cwd, open, at).unwrap();
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
        write_session(&w, h, cwd, true, now);
        w.advance(45);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "{key}: its open turn keeps it from silence");
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
        "harness.rs: fn launch",
        "harness.rs: fn read",
        "harness.rs: fn shell_call",
        "harness.rs: fn stopped",
        "harness.rs: fn turn",
        "harness.rs: fn write_session",
        "lib.rs: mod account",
        "lib.rs: use harness::{Harness, Stop}",
        "lib.rs: use start::{Launch, RULES, SKILLS, Start}",
        "lib.rs: use turn::Turn",
        "start.rs: const RULES",
        "start.rs: const SKILLS",
        "start.rs: struct Launch",
        "start.rs: struct Start",
        "turn.rs: enum Turn",
    ];
    assert_eq!(offered, expected);
}
