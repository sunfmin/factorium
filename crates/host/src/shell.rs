//! Shell commands under a limit. A command runs in a session of its own,
//! so ending it ends everything it started, even what left its process
//! group or its parent.

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

/// The CPU a command's whole process tree gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// As any other process: a gate, and any heavy command whose time a
    /// gate compares, so both measure at the same priority.
    Normal,
    /// Only what others leave: heavy work whose time no gate compares (a
    /// cut, idle work, a mutation run), so a gate running beside it is not
    /// slowed. Inherited by every process it starts.
    Background,
}

impl Priority {
    /// The program and arguments that start `sh` at this priority.
    fn launcher(self) -> &'static [&'static str] {
        match self {
            Priority::Normal => &[],
            // Darwin's background band (lowest CPU, throttled disk), and
            // a nice of 10 more, which leaves room below: a background
            // command's own heavy commands go lower still.
            Priority::Background if cfg!(target_os = "macos") => &["taskpolicy", "-b", "nice", "-n", "10"],
            Priority::Background => &["nice", "-n", "10"],
        }
    }
}

/// How many of its last lines a run keeps.
const TAIL_LINES: usize = 60;

/// The heavy entry a waiter that execs the installed binary keeps: its own,
/// never a step's.
pub const PLACE: &str = "FACTORIUM_HEAVY_PLACE";

/// How long an ended run's processes get to exit after TERM before KILL.
const GRACE: Duration = Duration::from_secs(2);

/// Runs `line` with `sh -c` in `dir` at `priority`, with `env` added, in a
/// session of its own, ending it after `limit`. `started` is told the
/// session as it starts; `stop` is asked every 20ms whether to end it
/// early.
pub fn run(
    dir: &Path,
    line: &str,
    env: &[(String, String)],
    limit: Duration,
    priority: Priority,
    started: &dyn Fn(u32),
    stop: &dyn Fn() -> bool,
) -> Ran {
    let at = Instant::now();
    let log = std::env::temp_dir().join(format!("factorium-run-{}-{:?}", std::process::id(), at));
    let file = match std::fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => return failed(format!("cannot make {}: {e}", log.display())),
    };
    let mut cmd = match priority.launcher() {
        [] => Command::new("sh"),
        [program, args @ ..] => {
            let mut c = Command::new(program);
            c.args(args).arg("sh");
            c
        }
    };
    cmd.arg("-c").arg(format!("exec 2>&1; {line}")).current_dir(dir).stdin(Stdio::null());
    cmd.stdout(file);
    in_own_session(&mut cmd);
    cmd.env_remove(PLACE);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return failed(format!("cannot start sh: {e}")),
    };
    started(child.id());
    let (mut timed_out, mut ended) = (false, false);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if !ended && (at.elapsed() >= limit || stop()) {
            timed_out = at.elapsed() >= limit;
            ended = true;
            end(child.id());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut said = String::new();
    if let Ok(mut f) = std::fs::File::open(&log) {
        let _ = f.read_to_string(&mut said);
    }
    let _ = std::fs::remove_file(&log);
    let code = status.and_then(|s| s.code());
    Ran { ok: !ended && code == Some(0), code, tail: tail(&said), took_ms: at.elapsed().as_millis() as u64, timed_out }
}

/// Starts `cmd` as the leader of a new session (and so of a new process
/// group): every process it starts stays in that session, whatever group
/// or parent it moves to, unless it starts a session of its own.
#[allow(unsafe_code)]
fn in_own_session(cmd: &mut Command) {
    // SAFETY: setsid is async-signal-safe and touches no memory of the
    // parent: it may run between fork and exec. One line: the child's
    // coverage is lost at exec, so a line of its own would read uncovered.
    let start = || if unsafe { libc::setsid() } == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) };
    unsafe { cmd.pre_exec(start) };
}

fn failed(why: String) -> Ran {
    Ran { ok: false, code: None, tail: why, took_ms: 0, timed_out: false }
}

/// The last lines of `said`.
pub fn tail(said: &str) -> String {
    let lines: Vec<&str> = said.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// Ends every process of session `session`, a run's: TERM, then KILL what
/// is left once they had [`GRACE`] to exit. The one way a run is ended,
/// whoever ends it: its limit, its stop (the hold bound, the host no longer
/// idle), or an unqueue from another process.
pub fn end(session: u32) {
    crate::procs::signal_session(session, libc::SIGTERM);
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline && !crate::procs::in_session(session).is_empty() {
        std::thread::sleep(Duration::from_millis(50));
    }
    crate::procs::signal_session(session, libc::SIGKILL);
}

/// What a program answered: whether it succeeded, and what it printed on
/// each stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Said {
    pub ok: bool,
    pub out: String,
    pub err: String,
}

impl Said {
    /// What it printed, or why it failed (`program args: stderr`).
    pub fn result(self, program: &str, args: &[&str]) -> Result<String, String> {
        match self.ok {
            true => Ok(self.out),
            false => Err(format!("{program} {}: {}", args.join(" "), self.err.trim())),
        }
    }
}

/// Running a program to its end and reading its answer: the one way the app
/// reaches orca, brew and mytokens. A scenario stands in here with recorded
/// answers, so what the app makes of them runs as it does for real.
pub trait Programs: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Said;

    /// Whether `program` is found on `PATH`: a file there that may be run.
    fn found(&self, program: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let runnable = |p: &Path| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
        if program.contains('/') {
            return runnable(Path::new(program));
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path).any(|d| runnable(&d.join(program)))
    }
}

/// The real programs, found on `PATH`.
pub struct System;

impl Programs for System {
    fn run(&self, program: &str, args: &[&str]) -> Said {
        match Command::new(program).args(args).stdin(Stdio::null()).output() {
            Ok(out) => Said {
                ok: out.status.success(),
                out: String::from_utf8_lossy(&out.stdout).into_owned(),
                err: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(e) => Said { ok: false, out: String::new(), err: format!("{program}: {e}") },
        }
    }
}

/// Runs a program to its end; its output, or why it failed.
pub fn output(program: &str, args: &[&str]) -> Result<String, String> {
    System.run(program, args).result(program, args)
}
