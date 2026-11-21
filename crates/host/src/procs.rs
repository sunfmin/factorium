//! The host's processes, as `ps` lists them, each with the worker it runs
//! for: the `FACTORIUM_WORKER` its environment holds.

use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// Percent of one core.
    pub cpu: f64,
    /// Resident memory, in KiB.
    pub rss_kib: u64,
    /// The command, as `ps` shows it.
    pub command: String,
    /// The worker it runs for, when its environment names one.
    pub worker: Option<String>,
}

const TAG: &str = "FACTORIUM_WORKER=";

/// Every process of this user.
pub fn list() -> Vec<Proc> {
    let user = std::env::var("USER").unwrap_or_default();
    let mut ps = Command::new("ps");
    ps.args(["-ww", "-E", "-o", "pid=,ppid=,pcpu=,rss=,command="]);
    if user.is_empty() {
        ps.arg("-ax")
    } else {
        ps.args(["-U", &user])
    };
    let Ok(out) = ps.output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(parse).collect()
}

fn parse(line: &str) -> Option<Proc> {
    let mut words = line.split_whitespace();
    let pid = words.next()?.parse().ok()?;
    let ppid = words.next()?.parse().ok()?;
    let cpu = words.next()?.parse().ok()?;
    let rss_kib = words.next()?.parse().ok()?;
    let rest: Vec<&str> = words.collect();
    let worker = rest.iter().find_map(|w| w.strip_prefix(TAG)).map(str::to_string);
    let command = rest.iter().take_while(|w| !w.contains('=') || !w.chars().next().is_some_and(char::is_uppercase));
    let command = command.copied().collect::<Vec<_>>().join(" ");
    Some(Proc { pid, ppid, cpu, rss_kib, command, worker })
}

/// The live processes of session `session`: what a run started in a
/// session of its own, however its processes regrouped (`setpgid`) or were
/// reparented since; only `setsid` leaves it. Zombies are left out: they
/// are ended, only not yet reaped. Read with `getsid`, as neither `ps`'s
/// `sess` nor a process's environment is readable on macOS 27.
pub fn in_session(session: u32) -> Vec<u32> {
    let me = std::process::id();
    let Ok(out) = Command::new("ps").args(["-ax", "-o", "pid=,stat="]).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid: u32 = words.next()?.parse().ok()?;
            let live = !words.next()?.starts_with('Z');
            (live && pid != me && session_of(pid) == Some(session)).then_some(pid)
        })
        .collect()
}

/// Sends `signal` to each live process of session `session`; answers how
/// many it sent to. Each is checked again to be in that session just
/// before, and this process's own session is never signalled: a wrong
/// listing cannot reach past the session (a mutant of `in_session` that
/// matched every other session once signalled all the owner's processes).
#[allow(unsafe_code)]
pub fn signal_session(session: u32, signal: libc::c_int) -> usize {
    if session <= 1 || session_of(std::process::id()) == Some(session) {
        return 0;
    }
    let mut sent = 0;
    for pid in in_session(session).into_iter().filter(|p| session_of(*p) == Some(session)) {
        let Ok(pid) = libc::pid_t::try_from(pid) else { continue };
        // SAFETY: kill only signals a process; no memory is shared.
        sent += usize::from(unsafe { libc::kill(pid, signal) } == 0);
    }
    sent
}

/// The session `pid` is in, if it lives.
#[allow(unsafe_code)]
fn session_of(pid: u32) -> Option<u32> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    // SAFETY: getsid reads the kernel's process table; no memory is shared.
    let sid = unsafe { libc::getsid(pid) };
    u32::try_from(sid).ok()
}

pub fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Whether `file` stays open in a program this process runs (`exec`s, or
/// starts), as a lock it holds must through an exec; files open closing in
/// them by default. Answers its descriptor.
#[allow(unsafe_code)]
pub fn kept_across_exec(file: &std::fs::File, kept: bool) -> std::io::Result<i32> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    let flags = if kept { 0 } else { libc::FD_CLOEXEC };
    // SAFETY: fcntl sets the flags of a descriptor `file` owns.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

/// The file at `path` this process was handed open as descriptor `fd` by
/// the image it replaced (`kept_across_exec`), if `fd` is open on that very
/// file: then it is this process's to own, and closes in programs it runs.
#[allow(unsafe_code)]
pub fn inherited(fd: i32, path: &std::path::Path) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    let at = std::fs::metadata(path).ok()?;
    // SAFETY: fstat writes only into `st`, which it is handed. A descriptor
    // not open leaves it zeroed, which no file's device and inode are.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    unsafe { libc::fstat(fd, &raw mut st) };
    if u64::try_from(st.st_dev).ok()? != at.dev() || st.st_ino != at.ino() {
        return None;
    }
    // SAFETY: `fd` is open on `path`'s file, which nothing else in this
    // process opened as it: it came through the exec, and is owned here.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    kept_across_exec(&file, false).ok()?;
    Some(file)
}
