//! The built binary: arguments parse, answers are JSON, refusals exit 1,
//! the database is where the environment says, and the guard keeps each
//! session to its own commands.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let d = std::env::temp_dir().join("factorium-binary").join(uuid::Uuid::now_v7().to_string());
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sh(dir: &Path, line: &str) {
    let ok = Command::new("sh")
        .args(["-c", line])
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(ok.success(), "{line}");
}

/// The programs through which the binary reaches the host's own service and
/// sessions: each a stub that fails, so no test stops, opens or reads the
/// real ones (a test's `stop` once booted out the host's daemon).
const HOST_PROGRAMS: [&str; 4] = ["launchctl", "orca", "mytokens", "brew"];

/// The binary on `dir`'s database (`XDG_DATA_HOME`) and an empty `HOME` (a
/// fresh host), in no role, with [`HOST_PROGRAMS`] stubbed out.
fn bin(dir: &Dir) -> Command {
    let stubs = dir.0.join("stubs");
    if !stubs.exists() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&stubs).unwrap();
        for p in HOST_PROGRAMS {
            std::fs::write(stubs.join(p), "#!/bin/sh\nexit 1\n").unwrap();
            std::fs::set_permissions(stubs.join(p), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap_or_default());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_factorium"));
    cmd.env("XDG_DATA_HOME", dir.0.join("data")).env_remove("FACTORIUM_DB").env("PATH", path);
    cmd.env("HOME", dir.0.join("home")).env_remove("FACTORIUM_WORKER").env_remove("FACTORIUM_WATCH");
    cmd
}

/// Runs [`bin`] with `env` added; answers its exit success and its printed
/// JSON.
fn run_with(dir: &Dir, env: &[(&str, &str)], args: &[&str]) -> (bool, Value) {
    let mut cmd = bin(dir);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let json = serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text:?}"));
    (out.status.success(), json)
}

fn run(dir: &Dir, args: &[&str]) -> (bool, Value) {
    run_with(dir, &[], args)
}

/// With no `--db` or `FACTORIUM_DB`, the database lives under `XDG_DATA_HOME`,
/// else under `HOME`'s `.local/share` (a Mac's usual case), else here.
#[test]
fn the_default_database_is_under_xdg_data_home_else_under_home_s_local_share() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |name: &str| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v).into())
    };
    let both = env(&[("XDG_DATA_HOME", "/x/data"), ("HOME", "/x/home")]);
    assert_eq!(factorium::default_db(both), Path::new("/x/data/factorium/factorium.db"));
    let home = env(&[("HOME", "/x/home")]);
    assert_eq!(factorium::default_db(home), Path::new("/x/home/.local/share/factorium/factorium.db"));
    assert_eq!(factorium::default_db(env(&[])), Path::new("./factorium/factorium.db"));
}

#[test]
fn the_cli_records_in_the_xdg_database_and_answers_json() {
    let dir = Dir::new();
    sh(
        &dir.0,
        "git init -q -b main seed && cd seed && git commit -q --allow-empty -m start && cd .. \
         && git clone -q --bare seed remote.git && git clone -q remote.git web",
    );
    let repo = dir.0.join("web");

    let (ok, added) = run(&dir, &["checkout", "add", "web", repo.to_str().unwrap()]);
    assert!(ok, "{added}");
    assert!(dir.0.join("data/factorium/factorium.db").exists());

    let (ok, issue) = run(&dir, &["issue", "add", "web", "Fix it", "--body", "how"]);
    assert!(ok);
    assert_eq!((issue["number"].clone(), issue["body"].clone()), (json!(1), json!("how")));
    let (ok, list) = run(&dir, &["issue", "list", "web"]);
    assert!(ok);
    assert_eq!(list[0]["title"], "Fix it");
    let (_, checkouts) = run(&dir, &["checkout", "list"]);
    assert_eq!(checkouts[0]["proving"], "proving");
    // A default role named, then taken away, through the CLI.
    let dirs = dir.0.to_str().unwrap();
    assert_eq!(run(&dir, &["checkout", "edit", "web", "--role", dirs]).1["role"], json!(dirs));
    assert_eq!(run(&dir, &["checkout", "edit", "web", "--no-role"]).1["role"], json!(null));
    // Its idle work turned off on this host, then on again with the role named and taken away.
    let off = run(&dir, &["checkout", "edit", "web", "--no-idle", "--role", dirs]).1;
    assert_eq!((off["idle_off"].clone(), off["role"].clone()), (json!(true), json!(dirs)), "{off}");
    assert_eq!(run(&dir, &["checkout", "list"]).1[0]["idle_off"], json!(true));
    let on = run(&dir, &["checkout", "edit", "web", "--idle", "--no-role"]).1;
    assert_eq!((on["idle_off"].clone(), on["role"].clone()), (json!(false), json!(null)), "{on}");
    let bare = bin(&dir).args(["checkout", "edit", "web"]).output().unwrap();
    assert!(!bare.status.success(), "an edit names what it changes");
    let (ok, settings) = run(&dir, &["settings", "silent_bound"]);
    assert!(ok);
    assert_eq!(settings["value"], 1800);
    assert_eq!(settings["unit"], "seconds");

    // An issue's life by the owner's hand, each step answering the issue.
    let issue = |args: &[&str]| {
        let (ok, answer) = run(&dir, &[&["issue"], args].concat());
        assert!(ok, "{args:?}: {answer}");
        answer
    };
    issue(&["add", "web", "Then this", "--waiting-for-human"]);
    assert_eq!(issue(&["block", "web", "2", "--by", "1"])["blocked_by"], json!([1]));
    // A time it waits for: set with its offset, shown in UTC, undone with no time named.
    let until = issue(&["block", "web", "2", "--until", "2026-11-23T14:34:37+09:00"]);
    assert_eq!(until["not_before"]["utc"], "2026-11-23T05:34:37Z", "{until}");
    assert_eq!(issue(&["block", "web", "2", "--until", "--undo"])["not_before"], json!(null));
    let timed = issue(&["add", "web", "Recount", "--not-before", "2026-11-23T05:34:37Z", "--waiting-for-human"]);
    assert_eq!(timed["not_before"]["ms"], until["not_before"]["ms"], "{timed}");
    let (ok, answer) = run(&dir, &["issue", "block", "web", "2", "--until"]);
    assert_eq!((ok, answer["error"]["code"].clone()), (false, json!("no_time")), "{answer}");
    // A role named at edit, changed with the title, then taken away.
    let role = dir.0.to_str().unwrap();
    let named = issue(&["edit", "web", "3", "--role", role, "--title", "Recount it"]);
    assert_eq!((named["role"].clone(), named["title"].clone()), (json!(role), json!("Recount it")), "{named}");
    assert_eq!(issue(&["edit", "web", "3", "--no-role"])["role"], json!(null));
    issue(&["close", "web", "3", "--reason", "only shown"]);
    let wall = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let before = wall();
    let note = issue(&["comment", "web", "1", "a note"])["comments"][0].clone();
    assert_eq!(note["text"], "a note");
    // The binary's clock is the system's.
    let at = note["at"].as_i64().unwrap();
    assert!((before..=wall()).contains(&at), "{at} not within the call");
    assert_eq!(issue(&["wait-for-human", "web", "1", "--why", "later"])["status"], "waiting_for_human");
    assert_eq!(issue(&["ready", "web", "1"])["status"], "ready");
    assert_eq!(issue(&["start-over", "web", "1", "--why", "again"])["discarded"], false, "it had no worker");
    let closed = issue(&["close", "web", "2", "--reason", "dup"]);
    assert_eq!((closed["status"].clone(), closed["reason"].clone()), (json!("closed"), json!("dup")));
    assert_eq!(run(&dir, &["workers"]), (true, json!([])));
    assert_eq!(run(&dir, &["outbox"]).1["error"]["code"], "no_session", "the watch has no session yet");
}

/// A worker's id, as its session's environment holds it.
const WORKER: &str = "00000000-0000-0000-0000-000000000001";

#[test]
fn a_refusal_exits_one_with_its_code() {
    let dir = Dir::new();
    let (ok, answer) = run(&dir, &["issue", "show", "web", "1"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "no_issue");
    // The owner's commands of the line and the queue reach the app: refused here.
    let owner: [&[&str]; 6] = [
        &["promote", "web"],
        &["cut", "web"],
        &["idle", "web"],
        &["hold", "web", "--why", "x"],
        &["unqueue", WORKER],
        &["discard", "/nowhere"],
    ];
    for args in owner {
        let (ok, answer) = run(&dir, args);
        assert!(!ok, "{args:?}");
        let code = match args[0] {
            "unqueue" => "no_entry",
            "discard" => "not_orphaned",
            _ => "no_checkout",
        };
        assert_eq!(answer["error"]["code"], code, "{args:?}");
    }
}

#[test]
fn a_worker_runs_only_worker_commands_and_only_the_watch_runs_watch_commands() {
    let dir = Dir::new();
    let worker = [("FACTORIUM_WORKER", WORKER)];
    let (ok, answer) = run_with(&dir, &worker, &["issue", "list"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "guarded");
    let (ok, answer) = run_with(&dir, &worker, &["issue", "add", "web", "x"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "guarded", "{answer}");
    // `worker file` reaches the app, its body read from the file it names.
    let body = dir.0.join("body.md");
    std::fs::write(&body, "what web#1 left").unwrap();
    let file = ["worker", "file", "web", "x", "--body-file", body.to_str().unwrap(), "--blocked-by", "web#1@cut"];
    let (ok, answer) = run_with(&dir, &worker, &file);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "no_worker", "{answer}");
    for command in ["done", "pick-mutants", "review-growth"] {
        let (ok, answer) = run_with(&dir, &worker, &["worker", command]);
        assert!(!ok);
        assert_eq!(answer["error"]["code"], "no_worker", "{command}: a worker command runs for a worker");
    }
    // A comment's text comes from --text, else from stdin; empty, it is refused before the worker is looked up.
    let (ok, answer) = run_with(&dir, &worker, &["worker", "comment", "--text", " "]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "no_text", "{answer}");
    let mut cmd = bin(&dir);
    cmd.args(["worker", "comment"]).env("FACTORIUM_WORKER", WORKER);
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"before 12.3s, after 4.1s").unwrap();
    let out = child.wait_with_output().unwrap();
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!out.status.success());
    assert_eq!(answer["error"]["code"], "no_worker", "stdin's text reached the app: {answer}");
    let (ok, answer) = run(&dir, &["watch", "next"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "guarded");
    let watch = [("FACTORIUM_WATCH", "1")];
    let (ok, _) = run_with(&dir, &watch, &["watch", "next"]);
    assert!(ok);
    // The watch's other commands reach the app: refused here, with no round or worker.
    for (args, code) in [
        (&["watch", "report", "--text", "x"][..], "no_round"),
        (&["watch", "restart", WORKER], "no_worker"),
        (&["watch", "tell", WORKER, "x"], "no_worker"),
    ] {
        let (ok, answer) = run_with(&dir, &watch, args);
        assert!(!ok, "{args:?}");
        assert_eq!(answer["error"]["code"], code, "{args:?}: {answer}");
    }
}

#[test]
fn neither_the_watch_nor_a_worker_stops_the_daemon() {
    let dir = Dir::new();
    for role in [("FACTORIUM_WATCH", "1"), ("FACTORIUM_WORKER", WORKER)] {
        let (ok, answer) = run_with(&dir, &[role], &["stop"]);
        assert!(!ok);
        assert_eq!(answer["error"]["code"], "guarded", "{answer}");
        let why = answer["error"]["message"].as_str().unwrap();
        assert!(why.contains("swaps to a newly installed binary on its own"), "{why}");
    }
}

#[test]
fn the_guard_hook_refuses_a_worker_an_owner_command_and_a_push() {
    let by_tool = |tool: &str, line: &str, role: Option<(&str, &str)>| -> Option<i32> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_factorium"));
        cmd.args(["guard", "claude-code"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd.env_remove("FACTORIUM_WORKER").env_remove("FACTORIUM_WATCH");
        if let Some((name, value)) = role {
            cmd.env(name, value);
        }
        let mut child = cmd.spawn().unwrap();
        let payload = json!({ "tool_name": tool, "tool_input": { "command": line } });
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        child.wait().unwrap().code()
    };
    let as_role = |line: &str, role: Option<(&str, &str)>| by_tool("Bash", line, role);
    let hook = |line: &str, worker: bool| as_role(line, worker.then_some(("FACTORIUM_WORKER", WORKER)));
    assert_eq!(hook("factorium worker gate", true), Some(0));
    // A worker files a follow-up its issue asks for, but not as the owner does (factorium#169).
    assert_eq!(hook("factorium worker file web follow-up --body-file f --blocked-by web#1@cut", true), Some(0));
    assert_eq!(hook("factorium issue add web follow-up --body-file f", true), Some(2));
    assert_eq!(hook("cd x && factorium issue close web 1", true), Some(2));
    assert_eq!(hook("git push origin HEAD", true), Some(2));
    // Only what runs as a command counts: a word that names factorium is no command.
    assert_eq!(hook("cargo test -p factorium && factorium worker exclusive -- make", true), Some(0));
    assert_eq!(hook("which -a factorium; git log --grep push", true), Some(0));
    assert_eq!(hook("FOO=1 /usr/local/bin/factorium promote web", true), Some(2));
    assert_eq!(hook("make | env X=1 factorium cut web", true), Some(2));
    assert_eq!(hook("git -C ../web push", true), Some(2));
    assert_eq!(as_role("factorium stop && factorium run", Some(("FACTORIUM_WATCH", "1"))), Some(2), "it refuses");
    // A word with `=` assigns only with a name before it: `--x=1` is the command run.
    assert_eq!(hook("--x=1 factorium issue close web 1", true), Some(0));
    assert_eq!(hook("factorium issue close web 1", false), Some(0), "the owner's own session");
    // The hook judges a factorium line as the command line does, each by its role.
    let watch = Some(("FACTORIUM_WATCH", "1"));
    assert_eq!(as_role("factorium watch report --text done", watch), Some(0));
    assert_eq!(hook("factorium watch report --text done", true), Some(2));
    assert_eq!(as_role("factorium issue close web 1 && git push", watch), Some(0), "the watch acts as the operator");
    // Only the shell tool's command is a shell line: another tool handed a
    // `command` (Codex's `apply_patch`, its patch) runs none.
    let worker = Some(("FACTORIUM_WORKER", WORKER));
    assert_eq!(by_tool("apply_patch", "git push origin HEAD", worker), Some(0));
}

/// The guard lets a call through only when it has read it: a payload it
/// cannot read is refused, with the harness's refused code (factorium#99).
#[test]
fn the_guard_hook_refuses_a_tool_call_it_cannot_read() {
    let guard = |harness: &str, payload: &str| -> (Option<i32>, String) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_factorium"))
            .args(["guard", harness])
            .env("FACTORIUM_WORKER", WORKER)
            .env_remove("FACTORIUM_WATCH")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let refused = |h: factorium_harness::Harness, payload: &str| {
        let (code, said) = guard(h.key(), payload);
        assert_eq!(code, Some(h.refused().into()), "{}: {payload}: {said}", h.key());
        assert_ne!(code, Some(0));
        assert!(said.contains("cannot read this tool call"), "{}: {said}", h.key());
    };
    for &h in factorium_harness::Harness::all() {
        for malformed in ["", "{", "not json", "{\"tool_name\": 1}", "{} trailing", "[]"] {
            refused(h, malformed);
        }
    }
    // Grok's two spellings of one field disagree: which one the tool runs is
    // not known, so neither is judged.
    let grok = factorium_harness::Harness::Grok;
    let ls = json!({ "command": "ls" });
    let push = json!({ "command": "git push origin HEAD" });
    let names = json!({ "toolName": "read_file", "tool_name": "run_terminal_command", "toolInput": push });
    refused(grok, &names.to_string());
    let inputs = json!({ "toolName": "run_terminal_command", "toolInput": ls, "tool_input": push });
    refused(grok, &inputs.to_string());
    // A well-formed call that runs no shell line, and a shell line a worker may run, still pass.
    let read = json!({ "toolName": "read_file", "toolInput": { "path": "x" } });
    assert_eq!(guard("grok", &read.to_string()).0, Some(0));
    let same = json!({ "toolName": "run_terminal_command", "tool_name": "run_terminal_command", "toolInput": ls });
    assert_eq!(guard("grok", &same.to_string()).0, Some(0));
    let read = json!({ "tool_name": "Read", "tool_input": { "file_path": "x" } });
    assert_eq!(guard("claude-code", &read.to_string()).0, Some(0));
    // A harness the guard does not know is no reason to let a call by.
    let (code, said) = guard("no-such-harness", &read.to_string());
    assert_eq!(code, Some(2), "{said}");
}

#[test]
fn a_body_from_a_file_or_stdin_comes_back_byte_for_byte() {
    let dir = Dir::new();
    sh(
        &dir.0,
        "git init -q -b main seed && cd seed && git commit -q --allow-empty -m start && cd .. \
         && git clone -q --bare seed remote.git && git clone -q remote.git web",
    );
    let repo = dir.0.join("web");
    assert!(run(&dir, &["checkout", "add", "web", repo.to_str().unwrap()]).0);
    let body = "Say \"hi\" and 'bye'.\n\n`code` and $(cat <<'EOF'\nx\nEOF\n) and $HOME\\n\n";
    let file = dir.0.join("body.md");
    std::fs::write(&file, body).unwrap();

    let (ok, added) = run(&dir, &["issue", "add", "web", "From a file", "--body-file", file.to_str().unwrap()]);
    assert!(ok, "{added}");
    let (_, shown) = run(&dir, &["issue", "show", "web", "1"]);
    assert_eq!(shown["body"], body);

    let stdin_body = format!("{body}from stdin\n");
    let mut cmd = bin(&dir);
    cmd.args(["issue", "edit", "web", "1", "--body-file", "-"]);
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin_body.as_bytes()).unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    let (_, shown) = run(&dir, &["issue", "show", "web", "1"]);
    assert_eq!(shown["body"], stdin_body.as_str());

    let both = bin(&dir)
        .args(["issue", "add", "web", "Both", "--body", "x", "--body-file", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!both.status.success());
    let (_, list) = run(&dir, &["issue", "list", "web"]);
    assert_eq!(list.as_array().unwrap().len(), 1, "the refused add made no issue");
}

#[test]
fn the_service_definition_is_a_launchd_agent_that_runs_the_daemon_with_its_path() {
    let dir = Dir::new();
    let bin = dir.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = format!("{}:/usr/bin:{}:/usr/bin", bin.display(), dir.0.join("gone").display());
    let home = dir.0.to_str().unwrap();
    let (ok, shown) = run_with(&dir, &[("PATH", &path), ("HOME", home)], &["service", "show"]);
    assert!(ok, "{shown}");
    assert_eq!(shown["file"], format!("{home}/Library/LaunchAgents/com.github.sunfmin.factorium.plist"));
    let file = dir.0.join("agent.plist");
    std::fs::write(&file, shown["plist"].as_str().unwrap()).unwrap();
    let lint = Command::new("plutil").arg("-lint").arg(&file).output().unwrap();
    assert!(lint.status.success(), "{}", String::from_utf8_lossy(&lint.stdout));
    let get = |key: &str| {
        let out = Command::new("plutil").args(["-extract", key, "raw", "-o", "-"]).arg(&file).output().unwrap();
        assert!(out.status.success(), "{key}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    assert_eq!(get("Label"), "com.github.sunfmin.factorium");
    assert_eq!(get("ProgramArguments.0"), env!("CARGO_BIN_EXE_factorium"));
    assert_eq!(get("ProgramArguments.1"), "run");
    assert_eq!(get("KeepAlive"), "true", "restarted when it exits");
    assert_eq!(get("RunAtLoad"), "true");
    assert_eq!(get("LimitLoadToSessionType"), "Background", "a job of user/<uid>, which no desktop session ends");
    assert_eq!(get("EnvironmentVariables.PATH"), format!("{}:/usr/bin", bin.display()), "declared, as found");
    let db = dir.0.join("data/factorium/factorium.db");
    assert_eq!(get("EnvironmentVariables.FACTORIUM_DB"), db.to_str().unwrap());
    assert_eq!(get("StandardErrorPath"), factorium::Data::of(&db).log().to_str().unwrap());

    // Installed and removed with launchctl stubbed to fail: the definition
    // is written, launchd's refusal is the install's, and remove takes it away.
    let (ok, refused) = run(&dir, &["service", "install"]);
    assert!(!ok && refused["error"]["code"] == "launchctl", "{refused}");
    let agent = dir.0.join("home/Library/LaunchAgents/com.github.sunfmin.factorium.plist");
    assert!(agent.exists());
    let (ok, removed) = run(&dir, &["service", "remove"]);
    assert!(ok && removed["removed"] == true, "{removed}");
    assert!(!agent.exists());
}

#[test]
fn a_program_runs_a_heavy_command_with_its_exit_code_and_reads_the_queues_depth() {
    let dir = Dir::new();
    let out = bin(&dir).args(["heavy", "run", "--", "sh", "-c", "echo $1; exit 3", "-", "a b"]).output().unwrap();
    let ran: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(3), "{ran}");
    assert_eq!(ran["tail"], "a b", "its words as given: {ran}");
    let nice = crate::fixture::nice_is("-gt", crate::fixture::own_nice());
    let (ok, ran) = run(&dir, &["heavy", "run", "--background", "--", "sh", "-c", &nice]);
    assert!(ok && ran["ok"] == true, "at the priority it stated: {ran}");
    let (ok, depth) = run(&dir, &["heavy", "depth"]);
    assert!(ok, "{depth}");
    assert_eq!((&depth["running"], &depth["waiting"], &depth["entries"]), (&json!(0), &json!(0), &json!([])));
    let (ok, answer) = run_with(&dir, &[("FACTORIUM_WORKER", WORKER)], &["heavy", "depth"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "guarded", "a worker runs `worker exclusive`: {answer}");
}

#[test]
fn exclusive_runs_its_words_as_given() {
    let dir = Dir::new();
    let (ok, ran) = run(&dir, &["worker", "exclusive", "--", "printf", "%s|", "", "a b", "it's", "x=1"]);
    assert!(ok, "{ran}");
    assert_eq!(ran["tail"], "|a b|it's|x=1|");
}

#[test]
fn exclusive_runs_at_a_gates_priority_unless_asked_for_background() {
    let dir = Dir::new();
    // The CLI runs at this process's nice value, as a gate would.
    let nice = |op: &str| crate::fixture::nice_is(op, crate::fixture::own_nice());
    let (ok, ran) = run(&dir, &["worker", "exclusive", "--", "sh", "-c", &nice("-eq")]);
    assert!(ok && ran["ok"] == true, "at the gate's priority: {ran}");
    let (ok, ran) = run(&dir, &["worker", "exclusive", "--background", "--", "sh", "-c", &nice("-gt")]);
    assert!(ok && ran["ok"] == true, "below it: {ran}");
}

#[test]
fn a_heavy_command_whose_process_died_holds_nothing_up() {
    let dir = Dir::new();
    // No reading ever has room: only a queue with nothing running starts one.
    assert!(run(&dir, &["settings", "memory_free_min", "100"]).0);
    sh(&dir.0, "mkfifo go");
    let mut held = bin(&dir);
    // `running` once its last process is the one left waiting on the fifo.
    let line = "perl -e 'alarm 20; open my $f, \">\", \"running\"; exec @ARGV' cat go";
    held.args(["worker", "exclusive", "--", "sh", "-c", line]).current_dir(&dir.0);
    let mut held = held.stdout(Stdio::null()).spawn().unwrap();
    let running = dir.0.join("running");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    // Killed once it noted its command's session, so the kill finds it at
    // the same point however long a loaded host took to get it there.
    let db = rusqlite::Connection::open(dir.0.join("data/factorium/factorium.db")).unwrap();
    let noted = || {
        let runs = "SELECT count(*) FROM events WHERE kind = 'HeavyRuns'";
        running.exists() && db.query_row(runs, [], |r| r.get::<_, i64>(0)).unwrap() > 0
    };
    while !noted() {
        assert!(std::time::Instant::now() < deadline, "the first never ran");
        std::thread::sleep(crate::fixture::LOOK);
    }
    held.kill().unwrap();
    held.wait().unwrap();

    let mut next = bin(&dir);
    next.args(["worker", "exclusive", "--", "true"]);
    let mut next = next.stdout(Stdio::null()).spawn().unwrap();
    let ended = loop {
        if let Some(status) = next.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() > deadline {
            next.kill().unwrap();
            break None;
        }
        std::thread::sleep(crate::fixture::LOOK);
    };
    // The dead one's command is still waiting on the fifo.
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK: never wait for a reader that is gone.
        let _ = std::fs::OpenOptions::new().write(true).custom_flags(0x0004).open(dir.0.join("go"));
    }
    assert!(ended.is_some_and(|s| s.success()), "the dead one's entry held it up");
}

/// factorium#193: a waiter that gives up (killed while it waits, as a
/// harness ends a command past its timeout) and asks again within the keep
/// bound takes back the place it had, ahead of one queued after its first ask.
#[test]
fn a_waiter_that_gave_up_and_asks_again_starts_before_one_queued_after_it() {
    let dir = Dir::new();
    // No reading ever has room: one starts only when nothing runs, oldest first.
    assert!(run(&dir, &["settings", "memory_free_min", "100"]).0);
    sh(&dir.0, "mkfifo go");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let db = rusqlite::Connection::open_with_flags(
        dir.0.join("data/factorium/factorium.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let until_queued = |n: i64| {
        let queued = "SELECT count(*) FROM events WHERE kind = 'HeavyQueued'";
        while db.query_row(queued, [], |r| r.get::<_, i64>(0)).unwrap() < n {
            assert!(std::time::Instant::now() < deadline, "{n} never queued");
            std::thread::sleep(crate::fixture::LOOK);
        }
    };
    let ask = |line: &str| {
        let mut cmd = bin(&dir);
        cmd.args(["heavy", "run", "--", "sh", "-c", line]).current_dir(&dir.0).stdout(Stdio::null());
        cmd.spawn().unwrap()
    };
    // It holds the queue until the fifo is written.
    let mut held = ask("perl -e 'alarm 20; open my $f, \">\", \"running\"; exec @ARGV' cat go");
    while !dir.0.join("running").exists() {
        assert!(std::time::Instant::now() < deadline, "the first never ran");
        std::thread::sleep(crate::fixture::LOOK);
    }
    let mut gave_up = ask("echo first >> log");
    until_queued(2);
    let mut after = ask("echo after >> log");
    until_queued(3);
    gave_up.kill().unwrap();
    gave_up.wait().unwrap();
    let mut again = ask("echo first >> log");
    until_queued(4);
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK: fails until `cat` has the fifo open for reading.
        let open = || std::fs::OpenOptions::new().write(true).custom_flags(0x0004).open(dir.0.join("go"));
        while open().is_err() {
            assert!(std::time::Instant::now() < deadline, "the first never read the fifo");
            std::thread::sleep(crate::fixture::LOOK);
        }
    }
    for child in [&mut held, &mut again, &mut after] {
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                child.kill().unwrap();
            }
            std::thread::sleep(crate::fixture::LOOK);
        }
    }
    let log = std::fs::read_to_string(dir.0.join("log")).unwrap_or_default();
    assert_eq!(log, "first\nafter\n", "asked again, it starts before the one queued after its first ask");
}

/// Starts the daemon on `dir`, its `ps` one that counts: each tick reads the
/// host's processes through it.
fn ticking(dir: &Dir) -> std::process::Child {
    let calls = dir.0.join("stubs/ps.calls");
    let ps = format!("#!/bin/sh\necho >> '{}'\nexec /bin/ps \"$@\"\n", calls.display());
    let _ = bin(dir);
    std::fs::write(dir.0.join("stubs/ps"), ps).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.0.join("stubs/ps"), std::fs::Permissions::from_mode(0o755)).unwrap();
    bin(dir).arg("run").stdout(Stdio::piped()).spawn().unwrap()
}

/// Until the daemon [`ticking`] on `dir` starts its tick number `ticks`,
/// however long a loaded host takes it there: no deadline, as a starved
/// daemon at background priority is slow, not stuck (factorium#183). One
/// that ends first fails it at once; a hang is the runner's to name.
fn until_tick(dir: &Dir, daemon: &mut std::process::Child, ticks: usize) {
    let calls = dir.0.join("stubs/ps.calls");
    while std::fs::read_to_string(&calls).map_or(0, |c| c.lines().count()) < ticks {
        let gone = daemon.try_wait().unwrap();
        assert!(gone.is_none(), "the daemon did not keep ticking: {gone:?}");
        std::thread::sleep(crate::fixture::LOOK);
    }
}

/// Runs the daemon on `dir` until it starts its tick number `ticks`, then
/// `stop`s it, as `stop` must; answers what it printed.
fn daemon_for(dir: &Dir, ticks: usize) -> std::process::Output {
    let mut daemon = ticking(dir);
    until_tick(dir, &mut daemon, ticks);
    let (ok, stopped) = run(dir, &["stop"]);
    if stopped["stopped"] != true {
        let _ = daemon.kill();
    }
    assert!(ok && stopped["pid"] == daemon.id(), "{stopped}");
    ended_in_order(dir, daemon)
}

/// Waits, as [`until_tick`] does, for a daemon sent SIGTERM or SIGINT: it
/// exits 0, as a normal return from main, and its pid file is gone.
fn ended_in_order(dir: &Dir, daemon: std::process::Child) -> std::process::Output {
    let out = daemon.wait_with_output().unwrap();
    assert!(out.status.success(), "{:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    let pid = dir.0.join("data/factorium/daemon.pid");
    assert!(!pid.exists(), "{} stays", pid.display());
    out
}

#[test]
fn a_daemon_sent_sigint_between_ticks_ends_in_order() {
    let dir = Dir::new();
    // A long wait between ticks: the signal ends it, not its next tick.
    assert!(run(&dir, &["settings", "tick_every", "600"]).0);
    // Signalled once its first tick began: in it or in the wait after it,
    // whichever a loaded host makes it, it ends after that one tick.
    let mut daemon = ticking(&dir);
    until_tick(&dir, &mut daemon, 1);
    assert!(Command::new("kill").args(["-INT", &daemon.id().to_string()]).status().unwrap().success());
    let out = ended_in_order(&dir, daemon);
    let said: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(said["stopped"], true, "{said}");
    assert_eq!(said["ticks"], 1, "{said}");
}

#[test]
fn run_ticks_quietly_until_stop_ends_it() {
    let dir = Dir::new();
    assert!(run(&dir, &["settings", "tick_every", "1"]).0);
    // Ticks with nothing to do, past the first (which looks), each done
    // once the next one starts.
    let out = daemon_for(&dir, 3);
    // Its only words are its answer at the end: a tick that did nothing prints nothing.
    let said: Value = serde_json::from_slice(&out.stdout).expect("one answer, no tick lines");
    assert_eq!(said["stopped"], true, "{said}");
}

#[test]
fn the_daemon_looks_for_the_kit_s_programs_on_its_path_at_its_start() {
    let dir = Dir::new();
    // One not on its PATH, one there but not to be run, one a directory.
    let stubs = dir.0.join("stubs");
    let _ = bin(&dir);
    std::fs::write(stubs.join("notrun"), "#!/bin/sh\n").unwrap();
    std::fs::create_dir_all(stubs.join("adir")).unwrap();
    let kit = run(&dir, &["kit", "set", "claude-code=nosuch-program", "codex=notrun", "pi=adir"]);
    assert!(kit.0, "{}", kit.1);
    // It looks before its first tick.
    daemon_for(&dir, 1);
    let (_, doctor) = run(&dir, &["doctor"]);
    let mut missing = crate::fixture::keyed(&doctor, "missing:");
    missing.sort();
    assert_eq!(missing, ["adir", "nosuch-program", "notrun"], "git and the stubs are found: {doctor}");
}

#[test]
fn a_program_s_answer_is_read_as_it_printed_it() {
    let dir = Dir::new();
    let _ = bin(&dir);
    let token = r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"t"}}"#;
    let script =
        format!("#!/bin/sh\ncase \"$1\" in list) echo 'agent-account-team/x' ;; get) echo '{token}' ;; esac\n");
    let stub = dir.0.join("stubs").join("mytokens");
    std::fs::write(&stub, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (ok, accounts) = run(&dir, &["accounts"]);
    assert!(ok, "{accounts}");
    assert_eq!(accounts[0]["name"], "team", "{accounts}");
}

/// Doctor that cannot read the store answers that as its one finding,
/// naming the store's error and the data directory, and exits 1.
#[test]
fn doctor_on_a_store_it_cannot_read_answers_that_finding_and_fails() {
    let dir = Dir::new();
    assert!(run(&dir, &["settings", "silent_bound", "1900"]).0);
    let db = dir.0.join("data/factorium/factorium.db");
    rusqlite::Connection::open(&db).unwrap().execute_batch("ALTER TABLE events RENAME COLUMN data TO gone;").unwrap();
    let (ok, doctor) = run(&dir, &["doctor"]);
    assert!(!ok, "{doctor}");
    let what = doctor["findings"][0]["what"].as_str().unwrap_or_default();
    assert!(what.contains("no such column") && what.contains(&*db.parent().unwrap().to_string_lossy()), "{doctor}");
}

/// The built binary, installed in `dir`'s Homebrew as `version` (a copy, as
/// Homebrew installs it), its opt link following it; answers its path.
fn installed(dir: &Dir, version: &str) -> PathBuf {
    let keg = dir.0.join("brew/Cellar/factorium").join(version);
    std::fs::create_dir_all(keg.join("bin")).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_factorium"), keg.join("bin/factorium")).unwrap();
    let opt = dir.0.join("brew/opt/factorium");
    std::fs::create_dir_all(opt.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&opt);
    std::os::unix::fs::symlink(&keg, &opt).unwrap();
    let _ = bin(dir);
    let brew = format!("#!/bin/sh\n[ \"$1\" = --prefix ] && echo '{}'\n", opt.display());
    std::fs::write(dir.0.join("stubs/brew"), brew).unwrap();
    keg.join("bin/factorium").canonicalize().unwrap()
}

/// The command a process runs, as `ps` says it.
fn command_of(pid: u32) -> String {
    let out = Command::new("ps").args(["-o", "command=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Until `done`, or panics past `secs` (so a daemon that never gets there
/// fails, not hangs), as long as `daemon` lives.
fn within(secs: u64, daemon: &mut std::process::Child, what: &str, mut done: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while !done() {
        assert!(daemon.try_wait().unwrap().is_none(), "the daemon ended before {what}");
        if std::time::Instant::now() > until {
            let _ = daemon.kill();
            panic!("not {what} in {secs}s");
        }
        std::thread::sleep(crate::fixture::LOOK);
    }
}

#[test]
fn a_daemon_that_runs_a_newly_installed_binary_keeps_its_lock_and_a_second_is_refused() {
    let dir = Dir::new();
    assert!(run(&dir, &["settings", "tick_every", "1"]).0);
    let old = installed(&dir, "0.0.9");
    let mut daemon = Command::new(&old);
    for (k, v) in bin(&dir).get_envs() {
        match v {
            Some(v) => daemon.env(k, v),
            None => daemon.env_remove(k),
        };
    }
    let said = std::fs::File::create(dir.0.join("daemon.err")).unwrap();
    let mut daemon = daemon.arg("run").stdout(Stdio::null()).stderr(said).spawn().unwrap();
    let pid = dir.0.join("data/factorium/daemon.pid");
    let me = daemon.id();
    within(60, &mut daemon, "it ran", || std::fs::read_to_string(&pid).is_ok_and(|p| p.trim() == me.to_string()));
    installed(&dir, "0.1.0");
    // It runs the new binary (through the opt link) in its place, as its own pid.
    let link = dir.0.join("brew/opt/factorium/bin/factorium");
    within(120, &mut daemon, "its exec", || command_of(me).starts_with(&*link.to_string_lossy()));
    // The lock came through the exec: it goes on, and holds it.
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(daemon.try_wait().unwrap().is_none(), "the new binary was refused its own lock");
    let mut second = bin(&dir).arg("run").stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let ended = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while second.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > ended {
            let _ = (second.kill(), daemon.kill());
            panic!("a second daemon ran beside the first");
        }
        std::thread::sleep(crate::fixture::LOOK);
    }
    let out = second.wait_with_output().unwrap();
    let refused: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!out.status.success(), "{refused}");
    assert_eq!(refused["error"]["message"], format!("a daemon runs already, as pid {me}"), "{refused}");
    let (ok, stopped) = run(&dir, &["stop"]);
    assert!(ok && stopped["pid"] == me, "{stopped}");
    ended_in_order(&dir, daemon);
}

/// What `issue add` says on stderr, its issue added.
fn issue_add_says(dir: &Dir, title: &str) -> String {
    let out = bin(dir).args(["issue", "add", "web", title]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// No daemon running is doctor's first finding and a line on stderr of the
/// other commands; a running one is neither; one that holds its lock and
/// stopped ticking (hung) is the finding again (factorium#214).
#[test]
fn no_daemon_running_is_doctor_s_first_finding_and_commands_say_so() {
    let dir = Dir::new();
    sh(
        &dir.0,
        "git init -q -b main seed && cd seed && git commit -q --allow-empty -m start && cd .. \
         && git clone -q --bare seed remote.git && git clone -q remote.git web",
    );
    assert!(run(&dir, &["checkout", "add", "web", dir.0.join("web").to_str().unwrap()]).0);
    assert!(run(&dir, &["settings", "tick_every", "1"]).0);
    let first = |doctor: &Value| doctor["findings"][0].clone();

    let (_, doctor) = run(&dir, &["doctor"]);
    assert_eq!(first(&doctor)["key"], "daemon:none", "{doctor}");
    assert!(first(&doctor)["fix"].as_str().unwrap().contains("factorium service install"), "{doctor}");
    assert!(issue_add_says(&dir, "a").contains("no daemon runs"), "issue add warns");

    let mut daemon = ticking(&dir);
    until_tick(&dir, &mut daemon, 2);
    let tick = dir.0.join("data/factorium/daemon.tick");
    let ticked: u128 = std::fs::read_to_string(&tick).unwrap().trim().parse().unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    assert!(now - ticked < 60_000, "the daemon writes when it ticked: {ticked}, now {now}");
    let (_, doctor) = run(&dir, &["doctor"]);
    assert!(!crate::fixture::finding_keys(&doctor).contains(&"daemon:none".to_string()), "{doctor}");
    assert_eq!(issue_add_says(&dir, "b"), "", "a running daemon: no warning");

    // Hung: alive, its lock held, its last tick long ago.
    let pid = daemon.id().to_string();
    assert!(Command::new("kill").args(["-STOP", &pid]).status().unwrap().success());
    std::fs::write(dir.0.join("data/factorium/daemon.tick"), "0").unwrap();
    let (_, doctor) = run(&dir, &["doctor"]);
    let said = issue_add_says(&dir, "c");
    assert!(Command::new("kill").args(["-CONT", &pid]).status().unwrap().success());
    assert_eq!(first(&doctor)["key"], "daemon:none", "{doctor}");
    assert!(first(&doctor)["what"].as_str().unwrap().contains("it hangs"), "{doctor}");
    assert!(said.contains("it hangs"), "{said}");
    let (ok, stopped) = run(&dir, &["stop"]);
    assert!(ok && stopped["pid"] == daemon.id(), "{stopped}");
    ended_in_order(&dir, daemon);

    // A live pid that holds no lock (a pid reused since): no daemon.
    std::fs::write(dir.0.join("data/factorium/daemon.pid"), std::process::id().to_string()).unwrap();
    std::fs::write(&tick, now.to_string()).unwrap();
    let (_, doctor) = run(&dir, &["doctor"]);
    assert!(first(&doctor)["what"].as_str().unwrap().contains("does not hold its lock"), "{doctor}");
}
