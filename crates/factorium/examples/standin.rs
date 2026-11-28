//! The scenario world's stand-ins as one command, so truefit runs each
//! contract case (`truefit.toml`) against the stand-in and the real program
//! the same way. `standin fake` answers from the scenarios' own fakes
//! (`tests/scenarios/fixture/fakes.rs`, the very code, in a world rooted at
//! the case's directory); `standin real` runs the real programs.
//!
//! It reads its steps from stdin, one JSON object a line (`#` lines and
//! blank ones skipped), and prints, a line each, what factorium reads of
//! each answer:
//!
//! - `{"run": [program, args…], "read": …}`: runs the program as factorium
//!   does. `read` is what is printed of the answer: `"ok"` (the default,
//!   whether it succeeded), `"out"` (that and its stdout), or a list of
//!   JSON pointers into its stdout, `*` standing for each item of an array
//!   or object. Programs the scenarios stand in for (orca, brew, stablehand,
//!   slimfit, rolesmith, launchctl, sleep, ssh) go to the fake; any other runs for real on
//!   both sides (git, to make a case's repo).
//! - `{"orca": method, "args": […]}`: calls factorium's own reader of Orca
//!   (`factorium::host::Orca`) and prints what it made of the answers.
//! - `{"world": {…}}`: sets what a scenario sets of the fake's world
//!   (`slimfit`, `ssh`, `installed`, `tap`, `busy`, `unobserved`
//!   and `background`); `real` skips it.
//! - `"only": "fake" | "real"` on any step runs it on that side alone,
//!   unprinted: how each side is brought to the same state.
//! - `"keep": {"name": pointer}` keeps a value of the answer (`""`: its
//!   whole trimmed stdout) as `{name}` in later steps, beside `{dir}` (the
//!   working directory), `{uid}`, `{nonce}` (unique to the run), `{machine}` (this one, as
//!   `hostname -s` names it) and
//!   `{scratch}`: the Orca cases' one repo per host (truefit.toml).
//! - `{"scratch": branch}`: makes `{scratch}` if need be, keeps its head as
//!   `{sha}`, and removes its worktrees on `branch` and the branch.
//!
//! `TRUEFIT_FAULT`, if set, names a failure the fake answers throughout
//! (`fakes::ORCA_FAULTS`, `fakes::BREW_FAULTS`,
//! `fakes::STABLEHAND_FAULTS`, `fakes::SLIMFIT_FAULTS`,
//! `fakes::ROLESMITH_FAULTS`, `fakes::SSH_FAULTS`); `real` never
//! reads it.

#[allow(dead_code)]
#[path = "../tests/scenarios/fixture/fakes.rs"]
mod fakes;

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;

use factorium::host::{ManualClock, Orca, Programs, Reading, Said, System, Term};
use fakes::FakeHost;
use serde_json::{Value, json};

/// What the scenarios stand in for: the rest is real on both sides.
const FAKED: [&str; 8] = ["orca", "brew", "stablehand", "slimfit", "rolesmith", "launchctl", "sleep", "ssh"];

/// The programs a side runs: the fake's world, or the machine, where
/// stablehand runs on books of the case's own (`{dir}/.stablehand`), never
/// the host's, and finds the engines a case makes in `{dir}/bin` first.
struct Side(Option<FakeHost>, PathBuf);

impl Programs for Side {
    fn run(&self, program: &str, args: &[&str]) -> Said {
        match &self.0 {
            Some(fake) if FAKED.contains(&program) => fake.run(program, args),
            None if program == "stablehand" => {
                let path = format!("{}:{}", self.1.join("bin").display(), std::env::var("PATH").unwrap_or_default());
                let mut command = std::process::Command::new(program);
                command.args(args).env("STABLEHAND_CONFIG_DIR", self.1.join(".stablehand")).env("PATH", path);
                let out = command.stdin(std::process::Stdio::null()).output().unwrap();
                let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
                Said { ok: out.status.success(), out: text(&out.stdout), err: text(&out.stderr) }
            }
            _ => System.run(program, args),
        }
    }
}

/// Sets what a scenario sets of the fake's world.
fn set(fake: &FakeHost, to: &Value) {
    let strings = |v: &Value| -> BTreeMap<String, String> {
        let pairs = v.as_object().into_iter().flatten();
        pairs.map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect()
    };
    fake.slimfit.lock().unwrap().extend(strings(&to["slimfit"]));
    let hosts = to["ssh"].as_object().into_iter().flatten();
    fake.hosts.lock().unwrap().extend(hosts.map(|(host, env)| (host.clone(), strings(env))));
    let mut brew = fake.brew.lock().unwrap();
    brew.installed.extend(strings(&to["installed"]));
    brew.tap.extend(strings(&to["tap"]));
    if let Some(handle) = to["busy"].as_str() {
        let mut terminals = fake.orca.terminals.lock().unwrap();
        let term = terminals.get_mut(handle).expect("a terminal to be busy");
        (term.busy, term.unobserved) = (true, to["unobserved"] == true);
    }
    if let Some(handle) = to["background"].as_str() {
        fake.orca.terminals.lock().unwrap().get_mut(handle).expect("a terminal with a shell").background = true;
    }
}

/// The values at `pointer` in `v`, `*` standing for each item.
fn at<'a>(v: &'a Value, pointer: &str) -> Vec<&'a Value> {
    let mut here = vec![v];
    for part in pointer.split('/').skip(1) {
        here = here
            .into_iter()
            .flat_map(|v| match (part, v) {
                ("*", Value::Array(a)) => a.iter().collect(),
                ("*", Value::Object(o)) => o.values().collect(),
                (p, v) => v.pointer(&format!("/{p}")).into_iter().collect::<Vec<_>>(),
            })
            .collect();
    }
    here
}

/// What factorium reads of an answer, as `read` names it.
fn read(said: &Said, how: &Value) -> Value {
    match how {
        Value::Array(pointers) => {
            let out: Value = serde_json::from_str(&said.out).unwrap_or(Value::Null);
            let read = pointers.iter().filter_map(Value::as_str).map(|p| {
                let found: Vec<&Value> = at(&out, p);
                (p.to_string(), json!(found))
            });
            json!({ "ok": said.ok, "read": read.collect::<serde_json::Map<_, _>>() })
        }
        Value::String(s) if s == "out" => json!({ "ok": said.ok, "out": said.out }),
        _ => json!({ "ok": said.ok }),
    }
}

/// An Orca reader's result as printed: its value, or why it failed.
fn made<T: Into<Value>>(r: Result<T, String>) -> Value {
    r.map_or_else(|why| json!({ "err": why }), |v| json!({ "ok": v.into() }))
}

/// What factorium's Orca reader makes of the answers to `method`.
fn orca(orca: &Orca, method: &str, a: &[String]) -> Value {
    let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or_default();
    let unit = |r: Result<(), String>| made(r.map(|()| true));
    match method {
        "worktree" => {
            made(orca.worktree(arg(0).as_ref(), arg(1), arg(2), arg(3), arg(4)).map(|p| p.display().to_string()))
        }
        "remove" => unit(orca.remove(arg(0).as_ref())),
        "open" => made(orca.open(arg(0).as_ref(), arg(1), arg(2))),
        "ready" => unit(orca.ready(arg(0))),
        "send" => unit(orca.send(arg(0), arg(1))),
        "press" => unit(orca.press(arg(0), arg(1))),
        "close" => unit(orca.close(arg(0))),
        "deliver" => made(orca.deliver(arg(0), arg(1), a.get(2).map(String::as_str)).map(|d| format!("{d:?}"))),
        // Which of the three it is: the rows are the harness's, not Orca's.
        "screen" => json!(match orca.screen(arg(0)) {
            Term::Running(_) => "running".to_string(),
            Term::Exited => "exited".to_string(),
            Term::Unreadable(why) => format!("unreadable: {why}"),
        }),
        // The title Orca lists the terminal under, if it lists it: Orca
        // lists the host's other terminals too, which no case reads.
        "listed" => made(orca.terminals().map(|t| {
            t.into_iter().find(|(h, _)| h == arg(0)).map(|(_, title)| title).map_or(Value::Null, Value::from)
        })),
        "comment" => unit(orca.comment(arg(0).as_ref(), arg(1))),
        "show_as" => unit(orca.show_as(arg(0).as_ref(), arg(1))),
        // The name and comment Orca lists the worktree at a path with, if it lists it.
        "listed_worktree" => made(orca.worktrees().map(|rows| {
            let row = rows.into_iter().find(|w| w.path == std::path::Path::new(arg(0)));
            row.map_or(Value::Null, |w| json!({ "name": w.name, "comment": w.comment }))
        })),
        _ => panic!("no Orca reader {method}"),
    }
}

/// The scratch repo at `repo`, made if need be, without `branch` or its
/// worktrees (removed through Orca): its head.
fn scratch(side: &Side, repo: &str, branch: &str) -> String {
    side.run("git", &["init", "-q", "-b", "main", repo]);
    let git = |args: &[&str]| side.run("git", &[&["-C", repo], args].concat());
    if !git(&["rev-parse", "HEAD"]).ok {
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "base"]);
    }
    let on = format!("branch refs/heads/{branch}");
    let listed = git(&["worktree", "list", "--porcelain"]).out;
    for entry in listed.split("\n\n").filter(|e| e.lines().any(|l| l == on)) {
        if let Some(path) = entry.lines().find_map(|l| l.strip_prefix("worktree ")) {
            let _ = Orca(side).remove(path.as_ref());
        }
    }
    git(&["worktree", "prune"]);
    git(&["branch", "-q", "-D", branch]);
    git(&["rev-parse", "HEAD"]).out.trim().to_string()
}

/// `s` with each `{name}` it holds as kept.
fn fill(s: &str, kept: &BTreeMap<String, String>) -> String {
    kept.iter().fold(s.to_string(), |s, (k, v)| s.replace(&format!("{{{k}}}"), v))
}

fn main() {
    let fake = match std::env::args().nth(1).as_deref() {
        Some("fake") => true,
        Some("real") => false,
        _ => panic!("usage: standin fake|real < steps"),
    };
    let dir: PathBuf = std::env::current_dir().unwrap();
    let side = Side(
        fake.then(|| {
            let reading = Reading { cpu_busy: 0, memory_free: 100, disk_free: 1, disk_total: 1 };
            // Its clock reads the time a real program reads, so a time past for one is past for both.
            let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as i64;
            let host = FakeHost::at(&dir, Arc::new(ManualClock::new(now)), reading);
            match std::env::var("TRUEFIT_FAULT") {
                Ok(f) if fakes::BREW_FAULTS.contains(&f.as_str()) => host.brew.lock().unwrap().fault(&f, true),
                Ok(f) if fakes::STABLEHAND_FAULTS.contains(&f.as_str()) => {
                    host.stablehand.lock().unwrap().fault(&f, true)
                }
                Ok(f) if fakes::ROLESMITH_FAULTS.contains(&f.as_str()) => {
                    host.rolesmith.lock().unwrap().fault(&f, true)
                }
                Ok(f) if fakes::SLIMFIT_FAULTS.contains(&f.as_str()) => host.slimfit_fault(&f, true),
                Ok(f) if fakes::SSH_FAULTS.contains(&f.as_str()) => host.ssh_fault(&f, true),
                Ok(f) => host.orca.fault(&f, true),
                Err(_) => {}
            }
            host
        }),
        dir.clone(),
    );
    let uid = std::fs::metadata(&dir).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap();
    let machine = factorium_harness::account::this_machine().unwrap();
    let nonce = format!("{}-{}", std::process::id(), std::time::UNIX_EPOCH.elapsed().unwrap().as_millis());
    let mut kept: BTreeMap<String, String> =
        [("dir", dir.display().to_string()), ("uid", uid.to_string()), ("nonce", nonce), ("machine", machine)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
    let state =
        if fake { dir.clone() } else { factorium::default_db(|k| std::env::var_os(k)).parent().unwrap().into() };
    kept.insert("scratch".into(), state.join("factorium-truefit").display().to_string());
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let step: Value = serde_json::from_str(&fill(&line, &kept)).unwrap_or_else(|e| panic!("{e}: {line}"));
        let only = step["only"].as_str();
        if only.is_some_and(|o| o != if fake { "fake" } else { "real" }) {
            continue;
        }
        let strings = |v: &Value| -> Vec<String> {
            v.as_array().into_iter().flatten().map(|a| a.as_str().unwrap_or_default().to_string()).collect()
        };
        let (printed, answer, text) = if let Some(branch) = step["scratch"].as_str() {
            let sha = scratch(&side, &kept["scratch"], branch);
            kept.insert("sha".into(), sha);
            continue;
        } else if let Some(to) = step.get("world") {
            if let Some(fake) = &side.0 {
                set(fake, to);
            }
            continue;
        } else if let Some(method) = step["orca"].as_str() {
            let answer = orca(&Orca(&side), method, &strings(&step["args"]));
            (json!({ "orca": method, "made": answer }), answer, String::new())
        } else {
            let run = strings(&step["run"]);
            let args: Vec<&str> = run[1..].iter().map(String::as_str).collect();
            let said = side.run(&run[0], &args);
            let out = serde_json::from_str(&said.out).unwrap_or(Value::Null);
            (json!({ "run": run, "said": read(&said, &step["read"]) }), out, said.out.trim().to_string())
        };
        // A pointer `""` keeps a program's whole trimmed stdout.
        for (name, pointer) in step["keep"].as_object().into_iter().flatten() {
            let pointer = pointer.as_str().unwrap_or_default();
            let v =
                if pointer.is_empty() { Some(text.as_str()) } else { answer.pointer(pointer).and_then(Value::as_str) };
            kept.insert(name.clone(), v.unwrap_or_default().to_string());
        }
        if only.is_none() {
            println!("{printed}");
        }
    }
}
