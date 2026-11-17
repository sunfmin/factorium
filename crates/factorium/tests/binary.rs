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

/// Runs the binary with `XDG_DATA_HOME` in `dir` and `env` added; answers its
/// exit success and its printed JSON.
fn run_with(dir: &Dir, env: &[(&str, &str)], args: &[&str]) -> (bool, Value) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_factorium"));
    cmd.args(args).env("XDG_DATA_HOME", dir.0.join("data")).env_remove("FACTORIUM_DB");
    cmd.env_remove("FACTORIUM_WORKER").env_remove("FACTORIUM_WATCH");
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
    let (ok, settings) = run(&dir, &["settings", "silent_bound"]);
    assert!(ok);
    assert_eq!(settings["value"], 1800);
    assert_eq!(settings["unit"], "seconds");
}

#[test]
fn a_refusal_exits_one_with_its_code() {
    let dir = Dir::new();
    let (ok, answer) = run(&dir, &["issue", "show", "web", "1"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "no_issue");
}

#[test]
fn a_worker_runs_only_worker_commands_and_only_the_watch_runs_watch_commands() {
    let dir = Dir::new();
    let worker = [("FACTORIUM_WORKER", "00000000-0000-0000-0000-000000000001")];
    let (ok, answer) = run_with(&dir, &worker, &["issue", "list"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "guarded");
    let (ok, answer) = run_with(&dir, &worker, &["worker", "done"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "no_worker", "a worker command runs for a worker");
    let (ok, answer) = run(&dir, &["watch", "next"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "guarded");
    let (ok, _) = run_with(&dir, &[("FACTORIUM_WATCH", "1")], &["watch", "next"]);
    assert!(ok);
}

#[test]
fn the_guard_hook_refuses_a_worker_an_owner_command_and_a_push() {
    let hook = |line: &str, worker: bool| -> Option<i32> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_factorium"));
        cmd.args(["guard", "claude-code"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd.env_remove("FACTORIUM_WORKER");
        if worker {
            cmd.env("FACTORIUM_WORKER", "00000000-0000-0000-0000-000000000001");
        }
        let mut child = cmd.spawn().unwrap();
        let payload = json!({ "tool_name": "Bash", "tool_input": { "command": line } });
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        child.wait().unwrap().code()
    };
    assert_eq!(hook("factorium worker gate", true), Some(0));
    assert_ne!(hook("cd x && factorium issue close web 1", true), Some(0));
    assert_ne!(hook("git push origin HEAD", true), Some(0));
    // Only what runs as a command counts: a word that names factorium is no command.
    assert_eq!(hook("cargo test -p factorium && factorium worker exclusive -- make", true), Some(0));
    assert_eq!(hook("which -a factorium; git log --grep push", true), Some(0));
    assert_ne!(hook("FOO=1 /usr/local/bin/factorium promote web", true), Some(0));
    assert_ne!(hook("make | env X=1 factorium cut web", true), Some(0));
    assert_ne!(hook("git -C ../web push", true), Some(0));
    assert_eq!(hook("factorium issue close web 1", false), Some(0), "the owner's own session");
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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_factorium"));
    cmd.args(["issue", "edit", "web", "1", "--body-file", "-"]);
    cmd.env("XDG_DATA_HOME", dir.0.join("data")).env_remove("FACTORIUM_DB");
    cmd.env_remove("FACTORIUM_WORKER").env_remove("FACTORIUM_WATCH");
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin_body.as_bytes()).unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    let (_, shown) = run(&dir, &["issue", "show", "web", "1"]);
    assert_eq!(shown["body"], stdin_body.as_str());

    let both = Command::new(env!("CARGO_BIN_EXE_factorium"))
        .args(["issue", "add", "web", "Both", "--body", "x", "--body-file", file.to_str().unwrap()])
        .env("XDG_DATA_HOME", dir.0.join("data"))
        .env_remove("FACTORIUM_DB")
        .env_remove("FACTORIUM_WORKER")
        .env_remove("FACTORIUM_WATCH")
        .output()
        .unwrap();
    assert!(!both.status.success());
    let (_, list) = run(&dir, &["issue", "list", "web"]);
    assert_eq!(list.as_array().unwrap().len(), 1, "the refused add made no issue");
}
