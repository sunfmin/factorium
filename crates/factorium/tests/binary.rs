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
    // A word with `=` assigns only with a name before it: `--x=1` is the command run.
    assert_eq!(hook("--x=1 factorium issue close web 1", true), Some(0));
    assert_eq!(hook("factorium issue close web 1", false), Some(0), "the owner's own session");
}

#[test]
fn exclusive_runs_its_words_as_given() {
    let dir = Dir::new();
    let (ok, ran) = run(&dir, &["worker", "exclusive", "--", "printf", "%s|", "", "a b", "it's", "x=1"]);
    assert!(ok, "{ran}");
    assert_eq!(ran["tail"], "|a b|it's|x=1|");
}

#[test]
fn a_heavy_command_whose_process_died_holds_nothing_up() {
    let dir = Dir::new();
    // No reading ever has room: only a queue with nothing running starts one.
    assert!(run(&dir, &["settings", "memory_free_min", "100"]).0);
    sh(&dir.0, "mkfifo go");
    let mut held = Command::new(env!("CARGO_BIN_EXE_factorium"));
    held.args(["worker", "exclusive", "--", "sh", "-c", "touch running; perl -e 'alarm 20; exec @ARGV' cat go"])
        .current_dir(&dir.0);
    held.env("XDG_DATA_HOME", dir.0.join("data")).env_remove("FACTORIUM_DB").env_remove("FACTORIUM_WORKER");
    let mut held = held.stdout(Stdio::null()).spawn().unwrap();
    let running = dir.0.join("running");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !running.exists() {
        assert!(std::time::Instant::now() < deadline, "the first never ran");
        std::thread::yield_now();
    }
    held.kill().unwrap();
    held.wait().unwrap();

    let mut next = Command::new(env!("CARGO_BIN_EXE_factorium"));
    next.args(["worker", "exclusive", "--", "true"]).env("XDG_DATA_HOME", dir.0.join("data"));
    next.env_remove("FACTORIUM_DB").env_remove("FACTORIUM_WORKER");
    let mut next = next.stdout(Stdio::null()).spawn().unwrap();
    let ended = loop {
        if let Some(status) = next.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() > deadline {
            next.kill().unwrap();
            break None;
        }
        std::thread::yield_now();
    };
    // The dead one's command is still waiting on the fifo.
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK: never wait for a reader that is gone.
        let _ = std::fs::OpenOptions::new().write(true).custom_flags(0x0004).open(dir.0.join("go"));
    }
    assert!(ended.is_some_and(|s| s.success()), "the dead one's entry held it up");
}

#[test]
fn run_ticks_until_stop_ends_it() {
    let dir = Dir::new();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_factorium"));
    daemon.arg("run").env("XDG_DATA_HOME", dir.0.join("data")).env_remove("FACTORIUM_DB");
    let mut daemon = daemon.env_remove("FACTORIUM_WORKER").stdout(Stdio::null()).spawn().unwrap();
    let pid = dir.0.join("data/factorium/daemon.pid");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !pid.exists() {
        let gone = daemon.try_wait().unwrap();
        if gone.is_some() || std::time::Instant::now() > deadline {
            let _ = daemon.kill();
            panic!("the daemon did not keep running: {gone:?}");
        }
        std::thread::yield_now();
    }
    let (ok, stopped) = run(&dir, &["stop"]);
    if stopped["stopped"] != true {
        let _ = daemon.kill();
    }
    assert!(ok && stopped["pid"] == daemon.id(), "{stopped}");
    daemon.wait().unwrap();
}
