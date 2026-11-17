//! Scenarios: each drives `App` as the CLI does and reads only what the
//! CLI would print. Git is real — a bare remote and a clone per repo, copied
//! from one made per run. The readings and the processes are stand-ins the
//! scenario moves; orca, brew and mytokens are programs whose answers it
//! gives, so what the app makes of them runs as for real. Time moves when
//! the scenario moves it.

mod accounts;
mod admission;
mod bounds;
mod comment;
mod daemon;
mod harnesses;
mod kept;
mod line;
mod one_harness;
mod outbox;
mod path;
mod queue;
mod stall;
mod terminals;
mod tracker;
mod watch;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use factorium::host::{Git, Host, Proc, Programs, Reading, Said};
use factorium::{Answer, App, Tick};
use factum::{Instant, ManualClock, Uuid};
use serde_json::{Value, json};

pub const GIB: u64 = 1 << 30;

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
    /// Orca refuses to type at terminals while set.
    pub refuse_send: Mutex<bool>,
    /// While set, `orca` prints no answer, only this on stderr (its runtime is down).
    pub down: Mutex<Option<String>>,
    /// The repos and worktrees Orca holds, by id.
    held: Mutex<BTreeMap<String, PathBuf>>,
    /// Repos Orca was just given: their own worktree is not found once yet.
    finding: Mutex<Vec<PathBuf>>,
    /// While set, a repo Orca was just given is never found.
    pub never_found: Mutex<bool>,
    /// Orca refuses to remove worktrees while set.
    pub refuse_rm: Mutex<bool>,
    /// Orca refuses to close a terminal while set.
    pub refuse_close: Mutex<bool>,
    /// While set, Orca answers a terminal's close and leaves it as it was.
    pub ignore_close: Mutex<bool>,
    /// Real Orca (1.4.254) takes seconds to close an agent's terminal (2.2s
    /// for dsh's) and lists it, connected, until the close returns. While
    /// set, this runs with the handle when one terminal's close is asked,
    /// before it takes: what another process sees in that time.
    pub while_closing: Mutex<Option<Closing>>,
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

impl FakeOrca {
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
                    return error("worktree_remove_failed");
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
                    "wait" => answer(json!({ "terminal": { "handle": handle, "status": status }, "satisfied": true })),
                    "send" if *self.refuse_send.lock().unwrap() => error("terminal_send_failed"),
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
            ["install" | "upgrade", _] => {
                if let Some(v) = self.tap.get(&formula).filter(|_| !self.stuck) {
                    self.installed.insert(formula.clone(), v.clone());
                    link(root, &formula, v);
                }
                text(true, format!("==> {} {formula}\n", args[0]))
            }
            ["--prefix", _] => text(true, format!("{}\n", root.join("brew/opt").join(&formula).display())),
            _ => text(false, String::new()),
        }
    }
}

/// Homebrew's layout, where a scenario made one for `formula`: version `v`
/// in the cellar, its binaries copied from the version before, and the opt
/// link moved to it.
fn link(root: &Path, formula: &str, v: &str) {
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
}

/// The machine as a scenario has it.
pub struct FakeHost {
    pub orca: FakeOrca,
    pub brew: Mutex<FakeBrew>,
    pub reading: Mutex<Reading>,
    pub procs: Mutex<Vec<Proc>>,
    /// mytokens: service → what `get --json` prints.
    pub tokens: Mutex<BTreeMap<String, String>>,
    /// Programs not found on PATH.
    pub missing: Mutex<Vec<String>>,
    /// Whether launchd has the service loaded.
    pub service: Mutex<bool>,
    /// Every program run, as `program args…`, oldest first.
    pub ran: Mutex<Vec<String>>,
    root: PathBuf,
}

impl FakeHost {
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
            "sleep" => Said { ok: true, ..Default::default() },
            "launchctl" => {
                // As launchd: this user's GUI domain, the agent by its label.
                use std::os::unix::fs::MetadataExt;
                let domain = format!("gui/{}", std::fs::metadata(&self.root).unwrap().uid());
                let target = format!("{domain}/com.github.sunfmin.factorium");
                let mut loaded = self.service.lock().unwrap();
                let ok = match args {
                    ["print", t] if *t == target => *loaded,
                    ["bootstrap", d, _] if *d == domain => !std::mem::replace(&mut *loaded, true),
                    ["bootout", t] if *t == target => std::mem::replace(&mut *loaded, false),
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

impl Host for FakeHost {
    fn programs(&self) -> &dyn Programs {
        self
    }
    fn reading(&self) -> Reading {
        *self.reading.lock().unwrap()
    }
    fn processes(&self) -> Vec<Proc> {
        self.procs.lock().unwrap().clone()
    }
    fn alive(&self, pid: u32) -> bool {
        pid == std::process::id()
    }
    fn home(&self) -> PathBuf {
        home(&self.root)
    }
}

/// The home of the world at `root`: empty, as a fresh host's.
fn home(root: &Path) -> PathBuf {
    root.join("home")
}

/// git in `dir` as the host of the world at `root` runs it: with its home,
/// which holds no config — a fresh host, with no identity of its own.
fn git_of(root: &Path, dir: &Path, args: &[&str]) -> Result<String, String> {
    Git::new(home(root), dir).run(args)
}

/// The repo every scenario starts from, made once per run: a bare remote
/// whose main holds a factorium.toml whose fast gate is red while `RED`
/// exists and whose slow gate is red while `SLOWRED` exists.
fn template() -> &'static Path {
    static T: OnceLock<PathBuf> = OnceLock::new();
    T.get_or_init(|| {
        let t = std::env::temp_dir().join("factorium-template").join(Uuid::now_v7().to_string());
        let work = t.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(home(&t)).unwrap();
        let git = |dir: &Path, args: &[&str]| git_of(&t, dir, args);
        git(&work, &["init", "--quiet", "-b", "main"]).unwrap();
        std::fs::write(work.join("README"), "a repo\n").unwrap();
        std::fs::write(
            work.join("factorium.toml"),
            "[gate]\nfast = \"test ! -e RED\"\nslow = \"test ! -e SLOWRED\"\n\n[cut]\nsteps = [\"echo $FACTORIUM_VERSION > VERSION\"]\nformula = \"web\"\n",
        )
        .unwrap();
        git(&work, &["add", "."]).unwrap();
        git(&work, &["commit", "--quiet", "-m", "start"]).unwrap();
        git(&t, &["clone", "--quiet", "--bare", "work", "remote.git"]).unwrap();
        t
    })
}

pub struct World {
    pub dir: PathBuf,
    /// The database every run of it opens.
    pub db: PathBuf,
    pub clock: Arc<ManualClock>,
    pub host: Arc<FakeHost>,
    pub app: App,
}

#[allow(clippy::new_without_default)]
impl World {
    pub fn new() -> World {
        World::on("factorium.db")
    }

    /// A world whose database is the file `name` in its data directory.
    pub fn on(name: &str) -> World {
        let dir = std::env::temp_dir().join("factorium-scenarios").join(Uuid::now_v7().to_string());
        std::fs::create_dir_all(dir.join("repos")).unwrap();
        std::fs::create_dir_all(home(&dir)).unwrap();
        let clock = Arc::new(ManualClock::new(Instant(1_800_000_000_000)));
        let host = Arc::new(FakeHost {
            orca: FakeOrca::default(),
            brew: Mutex::default(),
            reading: Mutex::new(Reading {
                cpu_busy: 10,
                memory_free: 60,
                disk_free: 500 * GIB,
                disk_total: 1000 * GIB,
            }),
            procs: Mutex::default(),
            tokens: Mutex::default(),
            ran: Mutex::default(),
            missing: Mutex::default(),
            service: Mutex::default(),
            root: dir.clone(),
        });
        let db = dir.join("data").join(name);
        let app = App::open(&db, clock.clone(), "owner", host.clone()).unwrap();
        World { dir, db, clock, host, app }
    }

    /// Another CLI run on the same database.
    pub fn run(&self) -> App {
        App::open(&self.db, self.clock.clone(), "owner", self.host.clone()).unwrap()
    }

    /// A clone of its own remote; answers its path.
    pub fn repo(&self, name: &str) -> PathBuf {
        let remote = self.dir.join("remotes").join(format!("{name}.git"));
        std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
        let t = template().join("remote.git");
        Command::new("cp").args(["-R", &t.to_string_lossy(), &remote.to_string_lossy()]).status().unwrap();
        let path = self.dir.join("repos").join(name);
        self.git(&self.dir, &["clone", "--quiet", &remote.to_string_lossy(), &path.to_string_lossy()]).unwrap();
        path
    }

    /// A repo on the checkout list, with the usual branches.
    pub async fn checkout(&self, name: &str) -> PathBuf {
        let path = self.repo(name);
        self.app.checkout_add(name, &path, "main", "proving").await.unwrap();
        path
    }

    /// A kit of one Claude Code entry.
    pub async fn kit(&self) {
        self.app.kit_set(&["claude-code=claude --dangerously-skip-permissions".into()]).await.unwrap();
    }

    pub async fn tick(&self) -> Tick {
        self.app.tick().await.unwrap()
    }

    pub fn advance(&self, minutes: i64) {
        self.clock.advance(minutes * 60_000);
    }

    pub fn terminal(&self, handle: &str) -> Terminal {
        self.host.orca.terminals.lock().unwrap()[handle].clone()
    }

    /// Runs doctor, as another process on its database, each time one
    /// terminal's close is asked and has not yet taken; answers what each
    /// run found, oldest first.
    pub fn doctor_while_closing(&self) -> Arc<Mutex<Vec<Value>>> {
        let found = Arc::new(Mutex::new(Vec::new()));
        let (db, clock, host, into) = (self.db.clone(), self.clock.clone(), Arc::downgrade(&self.host), found.clone());
        *self.host.orca.while_closing.lock().unwrap() = Some(Box::new(move |_: &str| {
            let (db, clock, host) = (db.clone(), clock.clone(), host.upgrade().unwrap());
            let doctor = std::thread::spawn(move || {
                let app = App::open(&db, clock, "owner", host).unwrap();
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                rt.block_on(app.doctor()).unwrap()
            });
            into.lock().unwrap().push(doctor.join().unwrap());
        }));
        found
    }

    pub fn set_screen(&self, handle: &str, lines: &[&str]) {
        let mut t = self.host.orca.terminals.lock().unwrap();
        t.get_mut(handle).unwrap().screen = lines.iter().map(|l| l.to_string()).collect();
    }

    /// Its harness opens a dialog of its own, showing `lines`.
    pub fn open_dialog(&self, handle: &str, lines: &[&str]) {
        let mut t = self.host.orca.terminals.lock().unwrap();
        let term = t.get_mut(handle).unwrap();
        (term.dialog, term.screen) = (true, lines.iter().map(|l| l.to_string()).collect());
    }

    /// Its Claude Code session is in a turn (`busy`) or not, which its
    /// session file says; `unobserved` as for [`Terminal`].
    pub fn set_busy(&self, handle: &str, busy: bool, unobserved: bool) {
        let cwd = {
            let mut t = self.host.orca.terminals.lock().unwrap();
            let term = t.get_mut(handle).unwrap();
            (term.busy, term.unobserved) = (busy, unobserved);
            term.worktree.to_string_lossy().into_owned()
        };
        let at = (factum::Clock::now(self.clock.as_ref()).0 / 1000) as u64;
        harnesses::write_session(self, factorium_harness::Harness::ClaudeCode, None, &cwd, busy, at);
    }

    pub fn kill_terminal(&self, handle: &str) {
        self.host.orca.terminals.lock().unwrap().get_mut(handle).unwrap().alive = false;
    }

    /// Runs git in `dir` as the world's host runs it.
    pub fn git(&self, dir: &Path, args: &[&str]) -> Result<String, String> {
        git_of(&self.dir, dir, args)
    }

    /// Commits `file` with `text` in a worktree.
    pub fn commit(&self, dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        self.git(dir, &["add", "-A"]).unwrap();
        self.git(dir, &["commit", "--quiet", "-m", &format!("write {file}")]).unwrap();
    }

    /// The remote's tip of `branch` of repo `name`.
    pub fn remote_tip(&self, name: &str, branch: &str) -> String {
        self.git(&self.dir.join("remotes").join(format!("{name}.git")), &["rev-parse", branch]).unwrap()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A live worker, as the CLI lists it.
pub async fn worker(app: &App) -> Value {
    app.workers(false).await.unwrap().as_array().unwrap().first().cloned().expect("a worker")
}

/// Whether factorium closed terminal `handle`, as the CLI prints it: the
/// worker it was opened for, if one was, shows it `closed` in `workers
/// --all`, and doctor names it neither an orphan nor unclosed.
pub async fn closed(app: &App, handle: &str) -> bool {
    let rows = app.workers(true).await.unwrap();
    let row = rows.as_array().unwrap().iter().find(|r| r["terminal"] == handle).cloned();
    let doctor = app.doctor().await.unwrap();
    let named = doctor["findings"].as_array().unwrap().iter().any(|f| {
        let key = f["key"].as_str().unwrap();
        key == format!("orphan:{handle}") || key == format!("unclosed:{handle}")
    });
    row.is_none_or(|r| r["closed"] == true) && !named
}

pub fn id(w: &Value) -> Uuid {
    w["worker"].as_str().unwrap().parse().unwrap()
}

pub fn refused(answer: Answer, code: &str) {
    match answer {
        Err(r) => assert_eq!(r.code, code, "{r}"),
        Ok(v) => panic!("expected {code}, got {v}"),
    }
}

impl World {
    /// Makes the daemon `app` run as version 0.0.9 of `formula`, as
    /// Homebrew installs it; answers the link Homebrew keeps to it.
    pub fn running_as(&self, app: &App, formula: &str) -> PathBuf {
        let bin = self.dir.join("brew/Cellar").join(formula).join("0.0.9/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("factorium"), "a binary").unwrap();
        let opt = self.dir.join("brew/opt").join(formula);
        std::fs::create_dir_all(opt.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(bin.parent().unwrap(), &opt).unwrap();
        self.host.brew.lock().unwrap().installed.insert(formula.into(), "0.0.9".into());
        app.runs_as(&bin.join("factorium").canonicalize().unwrap());
        opt.join("bin/factorium")
    }
}
