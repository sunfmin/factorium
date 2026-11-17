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

pub fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
