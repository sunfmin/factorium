//! Every harness, through what its own module answers and nothing named:
//! a worker of it starts with its own start line and brief, its home holds
//! its guard and where its rules are, and its session files — written by
//! the harness's own stand-in where its reader looks — say whether its turn
//! is open or closed, and whose it is. Adding a harness adds nothing here.

use std::collections::HashMap;
use std::path::PathBuf;

use factorium_harness::{Harness, RULES};

use crate::fixture::{World, worker, write_session};
use factorium::host::Host;

#[tokio::test]
async fn each_harness_starts_with_its_line_and_its_turn_is_read_from_its_own_session_files() {
    assert!(Harness::all().len() >= 6, "every harness is listed");
    crate::fixture::each(Harness::all(), |h| async move {
        let (key, command) = (h.key(), format!("run-{}", h.key()));
        let w = World::new();
        w.checkout("web").await;
        w.kit_of(&[format!("{key}={command}")]).await;
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
        let kept = w.kept_home(key);
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
    });
}

#[tokio::test]
async fn the_kit_pins_only_to_harnesses_factorium_starts_and_names_them_all() {
    let w = World::new();
    w.checkout("web").await;
    let refused = w.app.kit_pin("web", Some("vim")).await.unwrap_err();
    assert_eq!(refused.code, "no_harness");
    for h in Harness::all() {
        assert!(refused.message.contains(h.key()), "{}: {}", h.key(), refused.message);
    }
    assert_eq!(w.app.kit().await.unwrap(), serde_json::json!({ "pins": {} }), "pins only");
    for h in Harness::all() {
        w.kit_of(&[format!("{}={}", h.key(), h.program())]).await;
        w.app.kit_pin("web", Some(h.key())).await.unwrap();
    }
}

/// A worker of `h` whose command stablehand keeps names a stand-in of its
/// program, its line run as its terminal would: the worker, its line, and
/// the directory the program started in, its environment and its
/// arguments.
async fn started(w: &World, h: Harness) -> (serde_json::Value, String, PathBuf, HashMap<String, String>, String) {
    let (program, ran, args) = (w.dir.join("stand-in").join(h.program()), w.dir.join("ran"), w.dir.join("args"));
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    let script = format!(
        "#!/bin/sh\n{{ pwd -P; env -0; }} > '{}'\nprintf '%s\\n' \"$@\" > '{}'\n",
        ran.display(),
        args.display()
    );
    crate::fixture::program(&program, &script);
    w.kit_of(&[format!("{}={}", h.key(), program.display())]).await;
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
    (wk, line, PathBuf::from(dir), env, std::fs::read_to_string(&args).unwrap())
}

#[tokio::test]
async fn each_harness_s_home_holds_its_guard_and_where_its_rules_are_before_its_session_starts() {
    crate::fixture::each(Harness::all(), |&h| async move {
        let key = h.key();
        let w = World::new();
        let repo = w.repo("web");
        w.commit(&repo, RULES, "Always run the linter.\n");
        w.git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
        w.app.checkout_add("web", &repo, "main", "proving", None).await.unwrap();
        let (wk, line, dir, env, args) = started(&w, h).await;
        let kept = w.kept_home(key);
        let account = (key != "claude-code").then_some(kept.as_path());
        let home = h.home(&w.host.home(), account, &w.data().kept(key));
        let at = |path: &str| std::fs::canonicalize(path).unwrap();
        let (worktree, role) = (wk["worktree"].as_str().unwrap(), w.data().worker_role());
        let role = role.to_string_lossy();
        assert!(std::fs::read_to_string(at(&role).join(RULES)).unwrap().contains("You are a factorium worker"));

        // The guard is asked once before each tool call, this factorium as
        // one word whatever its path holds: given its program on its line,
        // or by what its home keeps, the line setting what that names.
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        let guard = format!("'{}' guard {key}", exe.replace('\'', "'\\''"));
        match h.guards(&home) {
            None => assert_eq!(args.matches(&guard).count(), 1, "{key}: its program is given the guard: {args}"),
            Some(guards) => {
                let set = |g: &String| g.strip_prefix('$').and_then(|name| env.get(name)).unwrap_or(g).clone();
                let asked = guards.iter().map(set).filter(|g| g.contains(&guard)).count();
                assert_eq!(asked, 1, "{key}: one guard {guard:?} before each tool call: {guards:?}");
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
        let sessions = [worktree.to_string(), w.data().watch().to_string_lossy().into_owned()];
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
    });
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
        "account.rs: fn this_machine",
        "ask.rs: fn ask_home",
        "ask.rs: fn ask_line",
        "ask.rs: fn asked",
        "ask.rs: fn asked_stand_in",
        "ask.rs: fn asks",
        "ask.rs: fn asks_again",
        "ask.rs: struct Ask",
        "ask.rs: struct Asked",
        "harness.rs: const fn interrupt",
        "harness.rs: const fn key",
        "harness.rs: const fn refused",
        "harness.rs: enum Call",
        "harness.rs: enum Harness",
        "harness.rs: enum Stop",
        "harness.rs: fn all",
        "harness.rs: fn asking",
        "harness.rs: fn command",
        "harness.rs: fn guards",
        "harness.rs: fn home",
        "harness.rs: fn launch",
        "harness.rs: fn not_up",
        "harness.rs: fn program",
        "harness.rs: fn read",
        "harness.rs: fn shell_call",
        "harness.rs: fn stopped",
        "harness.rs: fn told",
        "harness/store.rs: fn prompted",
        "harness/store.rs: fn spent",
        "harness/store.rs: fn turn",
        "harness/store.rs: fn write_session",
        "lib.rs: mod account",
        "lib.rs: use ask::{Ask, Asked}",
        "lib.rs: use harness::{Call, Harness, Stop}",
        "lib.rs: use line::{quoted, secrets_file}",
        "lib.rs: use start::{Home, Kept, Launch, RULES, SKILLS, Start}",
        "lib.rs: use turn::Turn",
        "line.rs: fn quoted",
        "line.rs: fn secrets_file",
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

/// Each harness's start, as it is written: its line, what is typed at it, the
/// files its home keeps, and the hooks its line gives it as the harness reads
/// them, the world's paths, this factorium and the host's `PATH` named. A
/// change to how a harness is started shows here, in `starts.txt`; one that
/// keeps every start as it was leaves it as it is. The repo has rules and the
/// kit names a model, so every part a start may hold is written.
#[tokio::test]
async fn each_harness_starts_as_it_is_written() {
    let sections = crate::fixture::each(Harness::all(), |&h| async move {
        let mut written = String::new();
        let key = h.key();
        let w = World::new();
        let repo = w.repo("web");
        w.commit(&repo, RULES, "Always run the \"linter\" \\ first.\n");
        w.git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
        w.app.checkout_add("web", &repo, "main", "proving", None).await.unwrap();
        w.kit_of(&[format!("{key}=given@model-1")]).await;
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let term = w.terminal(worker(&w.app).await["terminal"].as_str().unwrap());
        written += &format!("== {key}\nline: {}\n", term.line);
        for sent in &term.sent {
            written += &format!("typed: {sent}\n");
        }
        for hook in hooks_given(&term.line) {
            written += &format!("hook: {hook}\n");
        }
        let mut kept = Vec::new();
        let mut files = vec![w.kept_home(key)];
        while let Some(path) = files.pop() {
            let kind = std::fs::symlink_metadata(&path).map(|m| m.file_type());
            if kind.as_ref().is_ok_and(|k| k.is_dir()) {
                files.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            } else if kind.is_ok_and(|k| k.is_file()) {
                kept.push((path.clone(), std::fs::read_to_string(&path).unwrap()));
            }
        }
        kept.sort();
        for (path, text) in kept {
            written += &format!("-- {}\n{text}\n", path.display());
        }
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        written = written.replace(&exe, "<factorium>").replace(&*w.dir.to_string_lossy(), "<world>");
        written = written.replace(&std::env::var("PATH").unwrap_or_default(), "<path>");
        // What the session's PATH puts before the host's is the build's.
        while let Some(at) = written.find("PATH='") {
            let from = at + written[at..].find('/').unwrap();
            let to = from + written[from..].find(":<path>").unwrap();
            written.replace_range(at..to, "PATH=<bin>");
        }
        // A secrets file is named afresh at each start.
        let mut from = 0;
        while let Some(at) = written[from..].find("/secrets/").map(|i| from + i + "/secrets/".len()) {
            let to = at + written[at..].find(".sh").unwrap();
            written.replace_range(at..to, "<id>");
            from = at;
        }
        written
    });
    let written = sections.concat();
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios/starts.txt");
    if std::env::var_os("FACTORIUM_WRITE_STARTS").is_some() {
        std::fs::write(&file, &written).unwrap();
    }
    assert_eq!(written, std::fs::read_to_string(&file).unwrap_or_default(), "a start changed: {}", file.display());
}

/// The hook commands a start `line` hands its program as settings, as the
/// harness reads them: each `command` in a JSON `--settings` or a TOML
/// `--config` key, in order. The program `given` prints its arguments.
fn hooks_given(line: &str) -> Vec<String> {
    let line = format!("given() {{ printf '%s\\0' \"$@\"; }}; {line}");
    let out = std::process::Command::new("sh").args(["-c", &line]).output().unwrap();
    let args: Vec<String> = String::from_utf8_lossy(&out.stdout).split('\0').map(String::from).collect();
    let mut found = Vec::new();
    for pair in args.windows(2) {
        let value: Option<serde_json::Value> = match pair[0].as_str() {
            "--settings" => serde_json::from_str(&pair[1]).ok(),
            "--config" => pair[1].parse::<toml::Table>().ok().and_then(|t| serde_json::to_value(t).ok()),
            _ => None,
        };
        let mut values: Vec<serde_json::Value> = value.into_iter().collect();
        while let Some(value) = values.pop() {
            match value {
                serde_json::Value::Object(map) => {
                    found.extend(map.get("command").and_then(|c| c.as_str()).map(String::from));
                    values.extend(map.into_iter().map(|(_, v)| v).rev());
                }
                serde_json::Value::Array(items) => values.extend(items.into_iter().rev()),
                _ => {}
            }
        }
    }
    found
}
