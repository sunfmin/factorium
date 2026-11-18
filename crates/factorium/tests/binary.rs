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

/// A worker's id, as its session's environment holds it.
const WORKER: &str = "00000000-0000-0000-0000-000000000001";

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
    let worker = [("FACTORIUM_WORKER", WORKER)];
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
    assert_eq!(get("EnvironmentVariables.PATH"), format!("{}:/usr/bin", bin.display()), "declared, as found");
    let db = dir.0.join("data/factorium/factorium.db");
    assert_eq!(get("EnvironmentVariables.FACTORIUM_DB"), db.to_str().unwrap());
    assert_eq!(get("StandardErrorPath"), factorium::Data::of(&db).log().to_str().unwrap());
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
    let mut held = bin(&dir);
    held.args(["worker", "exclusive", "--", "sh", "-c", "touch running; perl -e 'alarm 20; exec @ARGV' cat go"])
        .current_dir(&dir.0);
    let mut held = held.stdout(Stdio::null()).spawn().unwrap();
    let running = dir.0.join("running");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !running.exists() {
        assert!(std::time::Instant::now() < deadline, "the first never ran");
        std::thread::yield_now();
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

/// Runs the daemon on `dir` for `ms` past its start, then `stop`s it, as
/// `stop` must; answers what it printed.
fn daemon_for(dir: &Dir, ms: u64) -> std::process::Output {
    let mut daemon = bin(dir).arg("run").stdout(Stdio::piped()).spawn().unwrap();
    let pid = factorium::Data::of(&dir.0.join("data/factorium/factorium.db")).pid();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !pid.exists() {
        let gone = daemon.try_wait().unwrap();
        if gone.is_some() || std::time::Instant::now() > deadline {
            let _ = daemon.kill();
            panic!("the daemon did not keep running: {gone:?}");
        }
        std::thread::yield_now();
    }
    std::thread::sleep(std::time::Duration::from_millis(ms));
    let (ok, stopped) = run(dir, &["stop"]);
    if stopped["stopped"] != true {
        let _ = daemon.kill();
    }
    assert!(ok && stopped["pid"] == daemon.id(), "{stopped}");
    daemon.wait_with_output().unwrap()
}

#[test]
fn run_ticks_quietly_until_stop_ends_it() {
    let dir = Dir::new();
    assert!(run(&dir, &["settings", "tick_every", "1"]).0);
    // A few ticks with nothing to do.
    let out = daemon_for(&dir, 2500);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "a tick that did nothing prints nothing");
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
    // Its pid file is written before it looks.
    daemon_for(&dir, 500);
    let (_, doctor) = run(&dir, &["doctor"]);
    let keys: Vec<&str> = doctor["findings"].as_array().unwrap().iter().filter_map(|f| f["key"].as_str()).collect();
    let mut missing: Vec<&str> = keys.iter().filter_map(|k| k.strip_prefix("missing:")).collect();
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
