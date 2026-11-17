//! Shell commands under a limit. A command runs in its own process group,
//! so ending it ends everything it started.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How a command ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    pub ok: bool,
    /// The exit code; `None` when it was ended or killed by a signal.
    pub code: Option<i32>,
    /// The last lines it printed, both streams interleaved.
    pub tail: String,
    pub took_ms: u64,
    /// It ran past its limit and was ended.
    pub timed_out: bool,
}

/// How many of its last lines a run keeps.
const TAIL_LINES: usize = 60;

/// Runs `line` with `sh -c` in `dir`, with `env` added, ending it after
/// `limit`. `stop` is asked every 100ms whether to end it early.
pub fn run(dir: &Path, line: &str, env: &[(String, String)], limit: Duration, stop: &dyn Fn() -> bool) -> Ran {
    let started = Instant::now();
    let log = std::env::temp_dir().join(format!("factorium-run-{}-{:?}", std::process::id(), started));
    let file = match std::fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => return failed(format!("cannot make {}: {e}", log.display())),
    };
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(format!("exec 2>&1; {line}")).current_dir(dir).stdin(Stdio::null());
    cmd.stdout(file).process_group(0);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return failed(format!("cannot start sh: {e}")),
    };
    let (mut timed_out, mut ended) = (false, false);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if !ended && (started.elapsed() >= limit || stop()) {
            timed_out = started.elapsed() >= limit;
            ended = true;
            kill_group(child.id());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut said = String::new();
    if let Ok(mut f) = std::fs::File::open(&log) {
        let _ = f.read_to_string(&mut said);
    }
    let _ = std::fs::remove_file(&log);
    let code = status.and_then(|s| s.code());
    Ran {
        ok: !ended && code == Some(0),
        code,
        tail: tail(&said),
        took_ms: started.elapsed().as_millis() as u64,
        timed_out,
    }
}

fn failed(why: String) -> Ran {
    Ran { ok: false, code: None, tail: why, took_ms: 0, timed_out: false }
}

/// The last lines of `said`.
pub fn tail(said: &str) -> String {
    let lines: Vec<&str> = said.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// Ends a process group: TERM, then KILL a second later.
pub fn kill_group(pid: u32) {
    let group = format!("-{pid}");
    let _ = Command::new("kill").args(["-TERM", "--", &group]).status();
    std::thread::sleep(Duration::from_millis(200));
    let _ = Command::new("kill").args(["-KILL", "--", &group]).stderr(Stdio::null()).status();
}

/// Runs a program to its end; its output, or why it failed.
pub fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{program}: {e}"))?;
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(said)
    } else {
        Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}
