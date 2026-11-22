//! The machine a scenario runs on: Orca, Homebrew, mytokens, slimfit and launchd as
//! programs whose answers the scenario gives, the host's readings and
//! processes, and its one clock. Git is real.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use factorium::host::{Clock, Git, Host, ManualClock, Proc, Programs, Reading, Said};
use factum::Uuid;
use serde_json::{Value, json};

/// A terminal the stand-in Orca keeps: what started it, what was typed at
/// it, and what it shows.
#[derive(Clone, Debug, Default)]
pub struct Terminal {
    pub worktree: PathBuf,
    pub title: String,
    pub line: String,
    pub sent: Vec<String>,
    pub screen: Vec<String>,
    pub alive: bool,
    /// Its agent is in a turn that does not end (a hung call): a line typed
    /// with Enter queues behind it, and only Escape stops it.
    pub busy: bool,
    /// Orca does not see its agent's turns start (one that ends within a
    /// second, or starts right after an interrupt).
    pub unobserved: bool,
    /// Its agent holds a shell it moved to the background: Orca sees it
    /// working, in a turn or not (Claude Code 2.1.330), so a wait for it
    /// to sit idle times out.
    pub background: bool,
    /// Closed through Orca: its tab is gone, not only its program.
    pub closed: bool,
    /// Its harness opened a dialog of its own: a line typed with Enter
    /// answers it and starts no turn; Escape dismisses it, back to its prompt.
    pub dialog: bool,
}

/// A terminal's screen at its shell's prompt, as zsh draws it.
pub const SHELL: &str = "felix@macnoMac-Studio web-1-1 % ";
/// The Escape key, as Orca is given it to type.
pub const ESCAPE: &str = "\x1b";
/// A terminal's screen with its harness up: its prompt, and a footer with
/// the words a harness that names some shows once up (Kimi Code's).
pub const UP: &str = "› Ask anything · ? for shortcuts · context: 0% (0/1M)";

/// What runs while a terminal closes, given its handle.
pub type Closing = Box<dyn FnMut(&str) + Send>;

/// Orca as `orca … --json` answers: its envelopes are shaped as Orca 1.4
/// answered them (`{"ok":true,"result":…}`, `{"ok":false,"error":{"code",
/// "message"}}`, `terminal read` giving `terminal.status` and `terminal.tail`).
#[derive(Default)]
pub struct FakeOrca {
    pub terminals: Mutex<BTreeMap<String, Terminal>>,
    pub removed: Mutex<Vec<PathBuf>>,
    /// Orca refuses to open terminals while set.
    pub refuse_open: Mutex<bool>,
    /// Terminals open at their shell's prompt while set, their harness not
    /// up; else at the harness's own screen.
    pub opens_at_shell: Mutex<bool>,
    /// Orca refuses to type at terminals while set ([`FakeOrca::fault`]).
    refuse_send: Mutex<bool>,
    /// While set, `orca` prints no answer, only this on stderr (its runtime is down).
    pub down: Mutex<Option<String>>,
    /// The repos and worktrees Orca holds, by id.
    held: Mutex<BTreeMap<String, PathBuf>>,
    /// Repos Orca was just given: their own worktree is not found once yet.
    finding: Mutex<Vec<PathBuf>>,
    /// While set, a repo Orca was just given is never found.
    pub never_found: Mutex<bool>,
    /// Orca refuses to remove worktrees while set ([`FakeOrca::fault`]).
    refuse_rm: Mutex<bool>,
    /// Orca refuses to close a terminal while set.
    pub refuse_close: Mutex<bool>,
    /// While set, Orca answers a terminal's close and leaves it as it was.
    pub ignore_close: Mutex<bool>,
    /// Real Orca (1.4.254) takes seconds to close an agent's terminal (2.2s
    /// for dsh's) and lists it, connected, until the close returns. While
    /// set, this runs with the handle when one terminal's close is asked,
    /// before it takes: what another process sees in that time.
    pub while_closing: Mutex<Option<Closing>>,
    /// While set, this runs with the worktree when a terminal is asked to
    /// open there, before Orca answers.
    pub while_opening: Mutex<Option<Closing>>,
    /// While set, this runs with the worktree when one is asked to be
    /// removed, before Orca removes it.
    pub while_removing: Mutex<Option<Closing>>,
}

fn said(ok: bool, out: Value) -> Said {
    Said { ok, out: out.to_string(), err: String::new() }
}

fn answer(result: Value) -> Said {
    said(true, json!({ "id": Uuid::now_v7().to_string(), "ok": true, "result": result }))
}

fn error(code: &str) -> Said {
    said(false, json!({ "id": Uuid::now_v7().to_string(), "ok": false, "error": { "code": code, "message": code } }))
}

/// The value after `--name` in `args`.
fn flag<'a>(args: &[&'a str], name: &str) -> &'a str {
    args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1)).copied().unwrap_or_default()
}

/// The failures the stand-in Orca answers when asked (truefit.toml's orca
/// `faults`, named by `TRUEFIT_FAULT` to `standin fake`), each as real Orca
/// 1.4.267 answers it: `send-refused`, a terminal it will not type at (as
/// one closed); `remove-refused`, a worktree it will not remove (as a
/// directory it does not hold).
pub const ORCA_FAULTS: [&str; 2] = ["send-refused", "remove-refused"];

impl FakeOrca {
    /// Sets, or clears, the failure `name` (one of [`ORCA_FAULTS`]).
    pub fn fault(&self, name: &str, on: bool) {
        let flag = match name {
            "send-refused" => &self.refuse_send,
            "remove-refused" => &self.refuse_rm,
            _ => panic!("no Orca fault {name}: {ORCA_FAULTS:?}"),
        };
        *flag.lock().unwrap() = on;
    }

    fn answer(&self, root: &Path, args: &[&str]) -> Said {
        if let Some(why) = &*self.down.lock().unwrap() {
            return Said { ok: false, out: String::new(), err: why.clone() };
        }
        if args[..2] == ["terminal", "close"] && !args.contains(&"--all") {
            let during = self.while_closing.lock().unwrap().take();
            if let Some(mut during) = during {
                during(flag(args, "--terminal"));
                *self.while_closing.lock().unwrap() = Some(during);
            }
        }
        if args[..2] == ["terminal", "create"] {
            let during = self.while_opening.lock().unwrap().take();
            if let Some(mut during) = during {
                during(flag(args, "--worktree"));
                *self.while_opening.lock().unwrap() = Some(during);
            }
        }
        if args[..2] == ["worktree", "rm"] {
            let during = self.while_removing.lock().unwrap().take();
            if let Some(mut during) = during {
                during(flag(args, "--worktree"));
                *self.while_removing.lock().unwrap() = Some(during);
            }
        }
        let mut held = self.held.lock().unwrap();
        let path_of = |sel: &str| sel.strip_prefix("path:").map(PathBuf::from);
        match (args[0], args[1]) {
            ("repo", "show") => match held.iter().find(|(_, p)| Some(*p) == path_of(flag(args, "--repo")).as_ref()) {
                Some((id, p)) => answer(json!({ "repo": { "id": id, "path": p } })),
                None => error("repo_not_found"),
            },
            ("repo", "add") => {
                let (id, path) = (format!("repo-{}", held.len() + 1), PathBuf::from(flag(args, "--path")));
                held.insert(id.clone(), path.clone());
                self.finding.lock().unwrap().push(path.clone());
                answer(json!({ "repo": { "id": id, "path": path } }))
            }
            ("worktree", "create") => {
                let repo = path_of(flag(args, "--repo")).unwrap();
                let path = root.join("worktrees").join(flag(args, "--name"));
                let (branch, sha) = (flag(args, "--branch"), flag(args, "--base-branch"));
                if let Err(e) =
                    git_of(root, &repo, &["worktree", "add", "--quiet", "-b", branch, &path.to_string_lossy(), sha])
                {
                    return Said { ok: false, out: String::new(), err: e };
                }
                let id = format!("wt-{}", held.len() + 1);
                held.insert(id, path.clone());
                // Orca opens a shell in a new workspace.
                let shell =
                    Terminal { worktree: path.clone(), title: "shell".into(), alive: true, ..Default::default() };
                self.terminals.lock().unwrap().insert(format!("shell-{}", flag(args, "--name")), shell);
                answer(json!({ "worktree": { "path": path, "branch": branch } }))
            }
            ("worktree", "rm") => {
                let path = path_of(flag(args, "--worktree")).unwrap();
                self.removed.lock().unwrap().push(path.clone());
                // A worktree gone from disk is gone from Orca too.
                if !path.exists() {
                    return error("selector_not_found");
                }
                if *self.refuse_rm.lock().unwrap() {
                    return error("selector_not_found");
                }
                // Orca refuses a worktree that holds another git registers.
                let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
                let listed = git_of(root, &path, &["worktree", "list", "--porcelain"]).unwrap_or_default();
                let inner = listed.lines().filter_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from);
                if let Some(inner) =
                    inner.into_iter().find(|p| real(p) != real(&path) && real(p).starts_with(real(&path)))
                {
                    let message = format!(
                        "Refusing to delete worktree because it contains another registered worktree: {}",
                        inner.display()
                    );
                    let error = json!({ "id": Uuid::now_v7().to_string(), "ok": false, "error": { "code": "refused", "message": message } });
                    return said(false, error);
                }
                if path.exists()
                    && let Err(e) = git_of(root, &path, &["worktree", "remove", "--force", &path.to_string_lossy()])
                {
                    return Said { ok: false, out: String::new(), err: e };
                }
                answer(json!({ "removed": true }))
            }
            ("terminal", "create") => {
                if *self.refuse_open.lock().unwrap() {
                    return error("terminal_create_failed");
                }
                let sel = flag(args, "--worktree");
                let worktree = match sel.split_once("::") {
                    Some((id, path)) if held.get(id.strip_prefix("id:").unwrap_or(id)).is_some() => {
                        let mut finding = self.finding.lock().unwrap();
                        match finding.iter().position(|p| p == Path::new(path)) {
                            Some(i) => {
                                if !*self.never_found.lock().unwrap() {
                                    finding.remove(i);
                                }
                                return error("selector_not_found");
                            }
                            None => PathBuf::from(path),
                        }
                    }
                    Some(_) => return error("selector_not_found"),
                    None => match path_of(sel).filter(|p| held.values().any(|h| h == p)) {
                        Some(p) => p,
                        None => return error("selector_not_found"),
                    },
                };
                let mut t = self.terminals.lock().unwrap();
                let handle = format!("term-{}", t.keys().filter(|k| k.starts_with("term-")).count() + 1);
                let screen = if *self.opens_at_shell.lock().unwrap() { SHELL } else { UP };
                let term = Terminal {
                    worktree,
                    title: flag(args, "--title").into(),
                    line: flag(args, "--command").into(),
                    alive: true,
                    screen: vec![screen.into()],
                    ..Default::default()
                };
                t.insert(handle.clone(), term);
                answer(json!({ "terminal": { "handle": handle } }))
            }
            ("terminal", verb) => {
                let mut t = self.terminals.lock().unwrap();
                if verb == "close" && args.contains(&"--all") {
                    let worktree = path_of(flag(args, "--worktree"));
                    for t in t.values_mut().filter(|t| Some(&t.worktree) == worktree.as_ref()) {
                        (t.alive, t.closed) = (false, true);
                    }
                    return answer(json!({ "closed": true }));
                }
                if verb == "list" {
                    let open = t.iter().filter(|(_, t)| !t.closed);
                    let rows: Vec<Value> = open.map(|(h, t)| json!({ "handle": h, "title": t.title })).collect();
                    return answer(json!({ "terminals": rows }));
                }
                let handle = flag(args, "--terminal");
                let Some(term) = t.get_mut(handle) else { return error("terminal_handle_stale") };
                let status = if term.alive { "running" } else { "exited" };
                match verb {
                    "read" => {
                        answer(json!({ "terminal": { "handle": handle, "status": status, "tail": term.screen } }))
                    }
                    "wait" if term.background && flag(args, "--for") == "tui-idle" => error("timeout"),
                    "wait" => answer(json!({ "terminal": { "handle": handle, "status": status }, "satisfied": true })),
                    "send" if *self.refuse_send.lock().unwrap() => error("terminal_not_writable"),
                    "send" if term.alive => {
                        let (text, enter) = (flag(args, "--text"), args.contains(&"--enter"));
                        term.sent.push(text.into());
                        if !enter && text == ESCAPE {
                            term.busy = false;
                            if term.dialog {
                                (term.dialog, term.screen) = (false, vec![UP.into()]);
                            }
                        }
                        // A watched prompt that reaches an idle agent starts a turn.
                        let mut stages = vec!["input_accepted"];
                        if enter && !term.busy && !term.unobserved && !term.dialog && args.contains(&"--wait-submit") {
                            stages.push("turn_started");
                        }
                        let prompt = enter.then(|| json!({ "stages": stages, "provider": "claude" }));
                        answer(json!({ "send": { "handle": handle, "accepted": true, "prompt": prompt } }))
                    }
                    "close" if *self.refuse_close.lock().unwrap() => error("terminal_close_failed"),
                    "close" if *self.ignore_close.lock().unwrap() => answer(json!({ "closed": true })),
                    "close" => {
                        (term.alive, term.closed) = (false, true);
                        answer(json!({ "closed": true }))
                    }
                    _ => error("terminal_exited"),
                }
            }
            _ => error("unknown_command"),
        }
    }
}

/// Homebrew as `brew` answers. Its tap holds, once `brew update` sees it,
/// the newest tag of the remote of the formula's name.
#[derive(Default)]
pub struct FakeBrew {
    /// Formula → the version installed.
    pub installed: BTreeMap<String, String>,
    /// Formula → the version the tap has.
    pub tap: BTreeMap<String, String>,
    /// How many more `brew update`s see the tap as it was.
    pub behind: u32,
    /// Install and upgrade succeed and change nothing (Homebrew does not link).
    pub stuck: bool,
    /// How many more installs and upgrades find the formula locked by another brew.
    pub locked: u32,
    /// The other brew installs the tap's version while it holds the lock.
    pub holder_installs: bool,
    /// What `ps -Ao pid=,command=` prints.
    pub ps: String,
    /// What installs and upgrades fail with, if they fail as no lock does.
    pub refuses: Option<String>,
    /// Install and upgrade leave PATH's `bin/<name>` on the keg before, and
    /// remove that keg (factorium#244).
    pub stale_link: bool,
    /// `brew link --overwrite` leaves the links as they are.
    pub relink_fails: bool,
}

impl FakeBrew {
    fn answer(&mut self, root: &Path, args: &[&str]) -> Said {
        let text =
            |ok: bool, out: String| Said { ok, out, err: if ok { String::new() } else { "Error: No such keg".into() } };
        let formula = args.last().copied().unwrap_or_default().to_string();
        match args {
            ["update", ..] if self.behind > 0 => {
                self.behind -= 1;
                text(true, String::new())
            }
            ["update", ..] => {
                for remote in std::fs::read_dir(root.join("remotes")).into_iter().flatten().flatten() {
                    let name = remote.file_name().to_string_lossy().trim_end_matches(".git").to_string();
                    let tags = git_of(root, &remote.path(), &["tag", "--sort=-v:refname"]).unwrap_or_default();
                    if let Some(tag) = tags.lines().next() {
                        self.tap.insert(name, tag.trim_start_matches('v').to_string());
                    }
                }
                text(true, String::new())
            }
            ["list", "--formula", _] => text(self.installed.contains_key(&formula), formula.clone()),
            ["list", "--versions", _] => match self.installed.get(&formula) {
                Some(v) => text(true, format!("{formula} {v}\n")),
                None => text(false, String::new()),
            },
            ["upgrade", _] if !self.installed.contains_key(&formula) => text(false, String::new()),
            ["install" | "upgrade", _] if self.refuses.is_some() => {
                Said { ok: false, out: String::new(), err: self.refuses.clone().unwrap() }
            }
            ["install" | "upgrade", _] if self.locked > 0 => {
                self.locked -= 1;
                if let Some(v) = self.tap.get(&formula).filter(|_| self.holder_installs).cloned() {
                    self.installed.insert(formula.clone(), v.clone());
                    link(root, &formula, &v, false);
                }
                let cellar = root.join("brew/Cellar").join(&formula);
                let err = format!(
                    "Error: A `brew upgrade {formula}` process has already locked {}.\nPlease wait for it to finish or terminate it to continue.",
                    cellar.display()
                );
                Said { ok: false, out: String::new(), err }
            }
            // No tap has it, as `brew update` last saw them.
            ["install", _] if !self.tap.contains_key(&formula) && !self.installed.contains_key(&formula) => {
                text(false, String::new())
            }
            ["install" | "upgrade", _] => {
                if let Some(v) = self.tap.get(&formula).filter(|_| !self.stuck) {
                    self.installed.insert(formula.clone(), v.clone());
                    link(root, &formula, v, self.stale_link);
                }
                text(true, format!("==> {} {formula}\n", args[0]))
            }
            ["--prefix", _] => text(true, format!("{}\n", root.join("brew/opt").join(&formula).display())),
            ["--prefix"] => text(true, format!("{}\n", root.join("brew").display())),
            ["--cellar", _] => text(true, format!("{}\n", root.join("brew/Cellar").join(&formula).display())),
            ["link", "--overwrite", _] => {
                if !self.relink_fails {
                    link_bin(root, &formula);
                }
                text(true, String::new())
            }
            _ => text(false, String::new()),
        }
    }
}

/// Homebrew's layout, where a scenario made one for `formula`: version `v`
/// in the cellar, its binaries copied from the version before, the opt link
/// moved to it, and PATH's links to its binaries; or, `stale`, those left
/// on the version before, which is removed.
fn link(root: &Path, formula: &str, v: &str, stale: bool) {
    let cellar = root.join("brew/Cellar").join(formula);
    let opt = root.join("brew/opt").join(formula);
    let Ok(was) = opt.canonicalize() else { return };
    std::fs::create_dir_all(cellar.join(v)).unwrap();
    Command::new("cp")
        .args(["-R", &was.join("bin").to_string_lossy(), &cellar.join(v).to_string_lossy()])
        .status()
        .unwrap();
    std::fs::remove_file(&opt).unwrap();
    std::os::unix::fs::symlink(cellar.join(v), &opt).unwrap();
    if stale {
        std::fs::remove_dir_all(was).unwrap();
    } else {
        link_bin(root, formula);
    }
}

/// Links PATH's `bin/<name>` to each binary of the keg `formula`'s opt link
/// follows, relative as Homebrew links them, over what was there.
pub fn link_bin(root: &Path, formula: &str) {
    let Ok(keg) = root.join("brew/opt").join(formula).canonicalize() else { return };
    let bin = root.join("brew/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cellar = root.join("brew/Cellar").canonicalize().unwrap();
    let kept = keg.strip_prefix(&cellar).unwrap();
    for program in std::fs::read_dir(keg.join("bin")).into_iter().flatten().flatten() {
        let link = bin.join(program.file_name());
        let _ = std::fs::remove_file(&link);
        let to = Path::new("../Cellar").join(kept).join("bin").join(program.file_name());
        std::os::unix::fs::symlink(to, link).unwrap();
    }
}

/// The machine as a scenario has it.
pub struct FakeHost {
    pub orca: FakeOrca,
    pub brew: Mutex<FakeBrew>,
    pub reading: Mutex<Reading>,
    pub procs: Mutex<Vec<Proc>>,
    /// Runs each time the processes are read, as every tick does.
    pub reading_procs: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// mytokens: service → what `get --json` prints.
    pub tokens: Mutex<BTreeMap<String, String>>,
    /// slimfit: its whole command line, else its subcommand → what it prints
    /// (`help picker`, `help reviewer`, `check`).
    pub slimfit: Mutex<BTreeMap<String, String>>,
    /// Programs not found on PATH.
    pub missing: Mutex<Vec<String>>,
    /// The agents launchd has loaded, as `<domain>/<uid>/<label>`.
    pub service: Mutex<Vec<String>>,
    /// How many prints after its bootout still find an agent, as launchd
    /// still holds a running one a moment after bootout returns.
    pub lingers: Mutex<usize>,
    /// Agents booted out that launchd still holds: prints left that find them.
    pub dropping: Mutex<Vec<(String, usize)>>,
    /// Every program run, as `program args…`, oldest first.
    pub ran: Mutex<Vec<String>>,
    /// Runs, once, on the next `sleep`: what happens while the app waits.
    pub while_sleeping: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// While set, this process reads as gone too: what a later process sees
    /// of one of its runs that died.
    pub gone: Mutex<bool>,
    /// The app's one clock: only the scenario moves it.
    pub clock: Arc<ManualClock>,
    pub(super) root: PathBuf,
}

impl FakeHost {
    /// The machine of the world at `root`, its home made, its clock and its
    /// reading as given, nothing else on it yet.
    pub fn at(root: &Path, clock: Arc<ManualClock>, reading: Reading) -> FakeHost {
        make_home(root);
        let (orca, brew, procs, reading_procs) = Default::default();
        let (tokens, slimfit, missing, service, ran, while_sleeping, gone) = Default::default();
        let (lingers, dropping) = Default::default();
        let reading = Mutex::new(reading);
        FakeHost {
            orca,
            brew,
            reading,
            procs,
            reading_procs,
            tokens,
            slimfit,
            missing,
            service,
            lingers,
            dropping,
            ran,
            while_sleeping,
            gone,
            clock,
            root: root.to_path_buf(),
        }
    }

    /// Whether launchd has factorium's agent loaded in this user's domain.
    pub fn loaded(&self) -> bool {
        let target = self.service_target("user");
        self.service.lock().unwrap().contains(&target)
    }

    /// factorium's agent's name in this user's `domain` (`user`, `gui`).
    pub fn service_target(&self, domain: &str) -> String {
        use std::os::unix::fs::MetadataExt;
        format!("{domain}/{}/com.github.sunfmin.factorium", std::fs::metadata(&self.root).unwrap().uid())
    }

    /// The `brew` commands run, oldest first.
    pub fn brewed(&self) -> Vec<String> {
        self.ran.lock().unwrap().iter().filter_map(|r| r.strip_prefix("brew ")).map(str::to_string).collect()
    }
}

impl Programs for FakeHost {
    fn run(&self, program: &str, args: &[&str]) -> Said {
        self.ran.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        if !self.found(program) {
            return Said { ok: false, out: String::new(), err: format!("{program}: not found") };
        }
        match program {
            "orca" => self.orca.answer(&self.root, args),
            "brew" => self.brew.lock().unwrap().answer(&self.root, args),
            "mytokens" => {
                let t = self.tokens.lock().unwrap();
                let out = match args {
                    ["list"] => Some(t.keys().cloned().collect::<Vec<_>>().join("\n")),
                    ["get", service, "--json"] => t.get(*service).cloned(),
                    _ => None,
                };
                Said { ok: out.is_some(), out: out.unwrap_or_default(), err: "no such secret".into() }
            }
            "slimfit" => {
                let said = self.slimfit.lock().unwrap();
                // A refusal is printed as slimfit prints one: `{"error": …}`, exit 2.
                let out = said.get(&args.join(" ")).or_else(|| said.get(args[0])).cloned();
                let ok = out.as_ref().is_some_and(|o| !o.starts_with(r#"{"error""#));
                Said { ok, out: out.unwrap_or_default(), err: "slimfit: no such answer".into() }
            }
            "ps" => Said { ok: true, out: self.brew.lock().unwrap().ps.clone(), err: String::new() },
            "sleep" => {
                if let Some(then) = self.while_sleeping.lock().unwrap().take() {
                    then();
                }
                Said { ok: true, ..Default::default() }
            }
            "launchctl" => {
                // As launchd: this user's domains, an agent by the label its
                // file names; `user/<uid>` takes only a Background job.
                use std::os::unix::fs::MetadataExt;
                let uid = std::fs::metadata(&self.root).unwrap().uid();
                let mut loaded = self.service.lock().unwrap();
                let mut dropping = self.dropping.lock().unwrap();
                let held = |t: &str, dropping: &[(String, usize)]| dropping.iter().any(|(x, _)| x == t);
                let ok = match args {
                    ["print", t] => {
                        let found = loaded.iter().any(|x| x == t) || held(t, &dropping);
                        if let Some(i) = dropping.iter().position(|(x, _)| x == t) {
                            dropping[i].1 -= 1;
                            if dropping[i].1 == 0 {
                                dropping.remove(i);
                            }
                        }
                        found
                    }
                    ["bootstrap", d, file] if *d == format!("user/{uid}") && background(file) => {
                        match label_of(file).map(|l| format!("{d}/{l}")) {
                            Some(t) if !loaded.contains(&t) && !held(&t, &dropping) => {
                                loaded.push(t);
                                true
                            }
                            _ => false,
                        }
                    }
                    ["bootout", t] => match loaded.iter().position(|x| x == t) {
                        Some(i) => {
                            let t = loaded.remove(i);
                            let lingers = *self.lingers.lock().unwrap();
                            if lingers > 0 {
                                dropping.push((t, lingers));
                            }
                            true
                        }
                        None => false,
                    },
                    _ => false,
                };
                Said { ok, out: String::new(), err: if ok { String::new() } else { "Input/output error".into() } }
            }
            _ => Said { ok: false, out: String::new(), err: format!("{program}: not found") },
        }
    }

    fn found(&self, program: &str) -> bool {
        !self.missing.lock().unwrap().iter().any(|m| m == program)
    }
}

/// Whether the agent's file at `file` is a job of the Background session.
fn background(file: &str) -> bool {
    let plist = std::fs::read_to_string(file).unwrap_or_default();
    plist.contains("<key>LimitLoadToSessionType</key>\n\t<string>Background</string>")
}

/// The label a launchd agent's file at `file` names.
fn label_of(file: &str) -> Option<String> {
    let plist = std::fs::read_to_string(file).ok()?;
    let (_, after) = plist.split_once("<key>Label</key>")?;
    let (_, rest) = after.split_once("<string>")?;
    rest.split_once("</string>").map(|(l, _)| l.to_string())
}

impl Host for FakeHost {
    fn programs(&self) -> &dyn Programs {
        self
    }
    fn reading(&self) -> Reading {
        *self.reading.lock().unwrap()
    }
    fn processes(&self) -> Vec<Proc> {
        if let Some(during) = &*self.reading_procs.lock().unwrap() {
            during();
        }
        self.procs.lock().unwrap().clone()
    }
    fn alive(&self, pid: u32) -> bool {
        pid == std::process::id() && !*self.gone.lock().unwrap()
    }
    fn home(&self) -> PathBuf {
        home(&self.root)
    }
    fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }
}

/// The home of the world at `root`: as a fresh host's, with no identity.
pub(super) fn home(root: &Path) -> PathBuf {
    root.join("home")
}

/// Makes the home of the world at `root`. Its git config turns off git's
/// automatic maintenance, which each fetch, commit and merge would otherwise
/// start as one more process, left running detached (factorium#98).
pub(super) fn make_home(root: &Path) {
    std::fs::create_dir_all(home(root)).unwrap();
    std::fs::write(home(root).join(".gitconfig"), "[maintenance]\n\tauto = false\n").unwrap();
}

/// git in `dir` as the host of the world at `root` runs it: with its home,
/// which holds no config — a fresh host, with no identity of its own.
pub(super) fn git_of(root: &Path, dir: &Path, args: &[&str]) -> Result<String, String> {
    Git::new(home(root), dir).run(args)
}
