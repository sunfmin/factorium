//! Every harness, through what its own module answers and nothing named:
//! a worker of it starts with its own start line and brief, its home holds
//! its guard and where its rules are, and its session files — written by
//! the harness's own stand-in where its reader looks — say whether its turn
//! is open or closed, and whose it is. Adding a harness adds nothing here.

use std::collections::HashMap;
use std::path::PathBuf;

use factorium_harness::{Harness, RULES};

use crate::{World, worker};
use factorium::host::Host;

/// Writes the session files a harness's stand-in describes for `cwd`, in
/// the home of its sessions on `account`.
pub fn write_session(w: &World, h: Harness, account: Option<&str>, cwd: &str, open: bool, at: u64) {
    // The brief its session was handed: the watch's, in its home; a worker's,
    // named as its worktree.
    let data = w.dir.join("data");
    let path = std::path::Path::new(cwd);
    let brief = if path == data.join("watch") {
        path.join("BRIEF.md")
    } else {
        data.join("briefs").join(format!("{}.md", path.file_name().unwrap().to_string_lossy()))
    };
    write_session_in(w, h, account, cwd, &brief.to_string_lossy(), open, at);
}

/// The stand-in session of `h` run in `dir` and handed `brief`.
pub fn write_session_in(w: &World, h: Harness, account: Option<&str>, dir: &str, brief: &str, open: bool, at: u64) {
    let kept = w.dir.join("data").join("kept").join(h.key());
    h.write_session(&h.home(&w.host.home(), account, &kept), dir, brief, open, at).unwrap();
}

#[tokio::test]
async fn each_harness_starts_with_its_line_and_its_turn_is_read_from_its_own_session_files() {
    assert!(Harness::all().len() >= 6, "every harness is listed");
    for h in Harness::all() {
        let (key, command) = (h.key(), format!("run-{}", h.key()));
        let w = World::new();
        w.checkout("web").await;
        w.app.kit_set(&[format!("{key}={command}")]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!(wk["harness"], key);
        let term = w.terminal(wk["terminal"].as_str().unwrap());
        assert!(term.line.contains(&command), "{key}: {}", term.line);
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
        // A home its line points it nowhere at is the host user's own: the host keeps none for it.
        let kept = w.dir.join("data").join("kept").join(key);
        assert!(term.line.contains(&*kept.to_string_lossy()) || !kept.exists(), "{key}: {}", term.line);
        let now = (w.app_now() / 1000) as u64;
        write_session(&w, h, None, cwd, true, now);
        w.advance(45);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "{key}: its open turn keeps it from silence");

        // Its turn ends; another worktree's session, ended since, is not its:
        // read as its, it would be idle 15 min, not 35.
        let ended = (w.app_now() / 1000) as u64;
        write_session(&w, h, None, cwd, false, ended);
        write_session(&w, h, None, &format!("{cwd}-other"), false, ended + 20 * 60);
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
async fn the_kit_takes_only_harnesses_factorium_starts_and_names_them_all() {
    let w = World::new();
    w.checkout("web").await;
    let refused = w.app.kit_set(&["vim=vim".into()]).await.unwrap_err();
    assert_eq!(refused.code, "no_harness");
    for h in Harness::all() {
        assert!(refused.message.contains(h.key()), "{}: {}", h.key(), refused.message);
    }
    assert_eq!(w.app.kit_pin("web", Some("vim")).await.unwrap_err().code, "no_harness");
    assert_eq!(w.app.kit().await.unwrap()["entries"], serde_json::json!([]), "nothing set");
    for h in Harness::all() {
        w.app.kit_set(&[format!("{}=x", h.key())]).await.unwrap();
        w.app.kit_pin("web", Some(h.key())).await.unwrap();
    }
}

/// A worker of `h` whose kit names a stand-in program, its line run as its
/// terminal would: the worker, its line, and the directory the program
/// started in and its environment.
async fn started(w: &World, h: Harness) -> (serde_json::Value, String, PathBuf, HashMap<String, String>) {
    let (program, ran) = (w.dir.join("program"), w.dir.join("ran"));
    std::fs::write(&program, format!("#!/bin/sh\n{{ pwd -P; env -0; }} > '{}'\n", ran.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    w.app.kit_set(&[format!("{}={}", h.key(), program.display())]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["harness"], h.key());
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    let out = std::process::Command::new("sh").args(["-c", &line]).output().unwrap();
    assert!(out.status.success(), "{}: {}\n{line}", h.key(), String::from_utf8_lossy(&out.stderr));
    let said = std::fs::read_to_string(&ran).unwrap();
    let (dir, env) = said.split_once('\n').unwrap();
    let env = env.split('\0').filter_map(|v| v.split_once('=')).map(|(k, v)| (k.into(), v.into())).collect();
    (wk, line, PathBuf::from(dir), env)
}

#[tokio::test]
async fn each_harness_s_home_holds_its_guard_and_where_its_rules_are_before_its_session_starts() {
    for &h in Harness::all() {
        let key = h.key();
        let w = World::new();
        let repo = w.repo("web");
        w.commit(&repo, RULES, "Always run the linter.\n");
        w.git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
        w.app.checkout_add("web", &repo, "main", "proving").await.unwrap();
        let (wk, line, dir, env) = started(&w, h).await;
        let kept = w.dir.join("data").join("kept").join(key);
        let home = h.home(&w.host.home(), None, &kept);
        let at = |path: &str| std::fs::canonicalize(path).unwrap();
        let (worktree, role) = (wk["worktree"].as_str().unwrap(), w.dir.join("data").join("role").join("worker"));
        let role = role.to_string_lossy();
        assert!(std::fs::read_to_string(at(&role).join(RULES)).unwrap().contains("You are a factorium worker"));

        // The guard is asked before each tool call: given on its line, or by
        // what its home keeps, the line setting what that names.
        let guard = format!("guard {key}");
        match h.guards(&home) {
            None => assert!(line.contains(&guard), "{key}: its line gives it the guard: {line}"),
            Some(guards) => {
                let set = |g: &String| g.strip_prefix('$').and_then(|name| env.get(name)).unwrap_or(g).clone();
                let asked = guards.iter().map(set).filter(|g| g.contains(&guard)).count();
                assert_eq!(asked, 1, "{key}: one guard before each tool call: {guards:?}");
                // A home it keeps is shared by every session of its harness
                // on its account: its line names where this session's rules are.
                assert_eq!(env.get("FACTORIUM_WORKTREE").map(|d| d.as_str()), Some(worktree), "{key}");
                assert_eq!(env.get("FACTORIUM_ADDED_DIR").map(|d| d.as_str()), Some(&*role), "{key}");
            }
        }
        // Where it starts: its role's directory, whose rules it reads; else
        // the worktree, told where its rules are.
        let told = h.told(&home);
        if let Some(text) = &told {
            assert!(text.contains("FACTORIUM_WORKTREE") && text.contains("FACTORIUM_ADDED_DIR"), "{key}: {text}");
        }
        let in_worktree = dir == at(worktree) && told.is_some();
        assert!(dir == at(&role) || in_worktree, "{key} starts in {}: {line}", dir.display());

        // What the host keeps for it is none of one session's: neither the
        // worker's nor the watch's, which started after it.
        let sessions = [worktree.to_string(), w.dir.join("data").join("watch").to_string_lossy().into_owned()];
        let mut files = vec![kept];
        while let Some(path) = files.pop() {
            let kind = std::fs::symlink_metadata(&path).map(|m| m.file_type());
            if kind.as_ref().is_ok_and(|k| k.is_dir()) {
                files.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            } else if kind.is_ok_and(|k| k.is_file()) {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let mine = sessions.iter().any(|dir| text.contains(dir.as_str())) || text.contains("linter");
                assert!(!mine, "{key}: {} holds one session's:\n{text}", path.display());
            }
        }
    }
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
        "harness.rs: const fn interrupt",
        "harness.rs: const fn key",
        "harness.rs: const fn refused",
        "harness.rs: enum Harness",
        "harness.rs: enum Stop",
        "harness.rs: fn all",
        "harness.rs: fn asking",
        "harness.rs: fn command",
        "harness.rs: fn guards",
        "harness.rs: fn home",
        "harness.rs: fn launch",
        "harness.rs: fn not_up",
        "harness.rs: fn read",
        "harness.rs: fn shell_call",
        "harness.rs: fn stopped",
        "harness.rs: fn told",
        "harness/store.rs: fn prompted",
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
