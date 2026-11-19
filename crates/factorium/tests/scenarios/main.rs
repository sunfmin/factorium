//! Scenarios: each drives `App` as the CLI does and reads only what the
//! CLI would print. Git is real — a bare remote and a clone per repo, copied
//! from one made per run. The readings and the processes are stand-ins the
//! scenario moves; orca, brew and mytokens are programs whose answers it
//! gives, so what the app makes of them runs as for real. Time moves when
//! the scenario moves it.

/// Runs each `async` block at once, each on a thread and runtime of its own,
/// as [`each`] does.
macro_rules! at_once {
    ($($case:expr),+ $(,)?) => {
        std::thread::scope(|s| {
            let running = [$(s.spawn(|| $crate::block_on($case))),+];
            for r in running {
                if let Err(panic) = r.join() {
                    std::panic::resume_unwind(panic);
                }
            }
        })
    };
}

mod accounts;
mod across;
mod admission;
mod base;
mod binary;
mod bounds;
mod bump;
mod comment;
mod daemon;
mod fixtures;
mod harnesses;
mod idle;
mod kept;
mod line;
mod one_harness;
mod outbox;
mod parked;
mod path;
mod priority;
mod queue;
mod stall;
mod terminals;
mod tracker;
mod watch;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use factorium::host::{Clock, Git, Host, ManualClock, Proc, Programs, Reading, Said};
use factorium::{Answer, App, Data, Tick};
use factorium_harness::Harness;
use factum::Uuid;
use serde_json::{Value, json};

pub const GIB: u64 = 1 << 30;

/// A kit entry for Claude Code.
pub const CLAUDE: &str = "claude-code=claude --dangerously-skip-permissions";

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
    /// Runs each time the processes are read, as every tick does.
    pub reading_procs: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// mytokens: service → what `get --json` prints.
    pub tokens: Mutex<BTreeMap<String, String>>,
    /// Programs not found on PATH.
    pub missing: Mutex<Vec<String>>,
    /// Whether launchd has the service loaded.
    pub service: Mutex<bool>,
    /// Every program run, as `program args…`, oldest first.
    pub ran: Mutex<Vec<String>>,
    /// Runs, once, on the next `sleep`: what happens while the app waits.
    pub while_sleeping: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// The app's one clock: only the scenario moves it.
    pub clock: Arc<ManualClock>,
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
            "sleep" => {
                if let Some(then) = self.while_sleeping.lock().unwrap().take() {
                    then();
                }
                Said { ok: true, ..Default::default() }
            }
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
        if let Some(during) = &*self.reading_procs.lock().unwrap() {
            during();
        }
        self.procs.lock().unwrap().clone()
    }
    fn alive(&self, pid: u32) -> bool {
        pid == std::process::id()
    }
    fn home(&self) -> PathBuf {
        home(&self.root)
    }
    fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }
}

/// The home of the world at `root`: as a fresh host's, with no identity.
fn home(root: &Path) -> PathBuf {
    root.join("home")
}

/// Makes the home of the world at `root`. Its git config turns off git's
/// automatic maintenance, which each fetch, commit and merge would otherwise
/// start as one more process, left running detached (factorium#98).
fn make_home(root: &Path) {
    std::fs::create_dir_all(home(root)).unwrap();
    std::fs::write(home(root).join(".gitconfig"), "[maintenance]\n\tauto = false\n").unwrap();
}

/// git in `dir` as the host of the world at `root` runs it: with its home,
/// which holds no config — a fresh host, with no identity of its own.
fn git_of(root: &Path, dir: &Path, args: &[&str]) -> Result<String, String> {
    Git::new(home(root), dir).run(args)
}

/// Raises this process's soft limit of open files, once, as far as 4096
/// (within its hard limit). Each world holds its database's files and its
/// git's pipes, and worlds run at once (`each`, `at_once!`): under launchd's
/// default of 256, which the daemon's gate runs with, the suite ran out
/// (factorium#98). The limit is the suite's own, not its caller's shell's.
#[allow(unsafe_code)]
fn open_files_enough() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: getrlimit and setrlimit only read and write `limit`.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
            return;
        }
        let want = limit.rlim_max.min(4096);
        if limit.rlim_cur < want {
            limit.rlim_cur = want;
            // SAFETY: as above.
            let raised = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) };
            assert_eq!(raised, 0, "raising the open-files limit to {want}");
        }
    });
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
        make_home(&t);
        let git = |dir: &Path, args: &[&str]| git_of(&t, dir, args);
        git(&work, &["init", "--quiet", "--template=", "-b", "main"]).unwrap();
        std::fs::write(work.join("README"), "a repo\n").unwrap();
        std::fs::write(
            work.join("factorium.toml"),
            "[gate]\nfast = \"test ! -e RED\"\nslow = \"test ! -e SLOWRED\"\n\n[cut]\nsteps = [\"echo $FACTORIUM_VERSION > VERSION\"]\nformula = \"web\"\n",
        )
        .unwrap();
        git(&work, &["add", "."]).unwrap();
        git(&work, &["commit", "--quiet", "-m", "start"]).unwrap();
        git(&t, &["clone", "--quiet", "--template=", "--bare", "work", "remote.git"]).unwrap();
        git(&t, &["clone", "--quiet", "--template=", &t.join("remote.git").to_string_lossy(), "clone"]).unwrap();
        t
    })
}

/// Copies the tree at `from` to `to`, in-process: a scenario's repos are
/// copies of the template's, made without a program per repo (factorium#98).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_tree(&src, &dst);
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src).unwrap(), &dst).unwrap();
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
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
        open_files_enough();
        let dir = std::env::temp_dir().join("factorium-scenarios").join(Uuid::now_v7().to_string());
        std::fs::create_dir_all(dir.join("repos")).unwrap();
        make_home(&dir);
        let clock = Arc::new(ManualClock::new(1_800_000_000_000));
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
            reading_procs: Mutex::default(),
            tokens: Mutex::default(),
            ran: Mutex::default(),
            missing: Mutex::default(),
            service: Mutex::default(),
            while_sleeping: Mutex::default(),
            clock: clock.clone(),
            root: dir.clone(),
        });
        let db = dir.join("data").join(name);
        let app = App::open(&db, "owner", host.clone()).unwrap();
        World { dir, db, clock, host, app }
    }

    /// Where the App keeps what it keeps beside the database.
    pub fn data(&self) -> Data {
        Data::of(&self.db)
    }

    /// Another CLI run on the same database.
    pub fn run(&self) -> App {
        App::open(&self.db, "owner", self.host.clone()).unwrap()
    }

    /// A clone of its own remote; answers its path.
    pub fn repo(&self, name: &str) -> PathBuf {
        let remote = self.dir.join("remotes").join(format!("{name}.git"));
        copy_tree(&template().join("remote.git"), &remote);
        // The template's clone, its origin this remote: as `git clone` of it makes.
        let path = self.dir.join("repos").join(name);
        copy_tree(&template().join("clone"), &path);
        let config = path.join(".git/config");
        let was = template().join("remote.git").to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains(&was), "the clone's origin is the template's remote: {text}");
        std::fs::write(&config, text.replace(&was, &remote.to_string_lossy())).unwrap();
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
        self.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    }

    /// A checkout `web`, a kit of the one entry `kit`, and a worker at work
    /// on issue 1, titled `title`; answers the worker as the CLI lists it.
    pub async fn working(&self, kit: &str, title: &str) -> Value {
        self.checkout("web").await;
        self.app.kit_set(&[kit.into()]).await.unwrap();
        self.app.issue_add("web", title, "", false, &[]).await.unwrap();
        self.tick().await;
        worker(&self.app).await
    }

    /// A kimi worker whose terminal opens at its shell's prompt.
    pub async fn at_shell(&self) -> Value {
        *self.host.orca.opens_at_shell.lock().unwrap() = true;
        self.working("kimi=kimi", "t").await
    }

    /// The finding doctor answers under `key`, if any.
    pub async fn finding(&self, key: &str) -> Option<Value> {
        let d = self.run().doctor().await.unwrap();
        d["findings"].as_array().unwrap().iter().find(|f| f["key"] == key).cloned()
    }

    /// The watch's session writes its turn: open, or closed.
    pub fn watch_session(&self, open: bool) {
        let home = self.data().watch();
        let at = (self.app_now() / 1000) as u64;
        harnesses::write_session(self, Harness::ClaudeCode, None, &home.to_string_lossy(), open, at);
    }

    /// The time on the world's clock, in milliseconds.
    pub fn app_now(&self) -> i64 {
        self.clock.now()
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
        let (db, host, into) = (self.db.clone(), Arc::downgrade(&self.host), found.clone());
        *self.host.orca.while_closing.lock().unwrap() = Some(Box::new(move |_: &str| {
            let (db, host) = (db.clone(), host.upgrade().unwrap());
            let doctor = std::thread::spawn(move || {
                let app = App::open(&db, "owner", host).unwrap();
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                rt.block_on(app.doctor()).unwrap()
            });
            into.lock().unwrap().push(doctor.join().unwrap());
        }));
        found
    }

    /// How many slow-gate runs the database recorded, of any tree.
    pub fn slow_gates_ran(&self) -> usize {
        let db = rusqlite::Connection::open(&self.db).unwrap();
        let sql = "SELECT count(*) FROM events WHERE kind = 'GateRan' AND data LIKE '%\"kind\":\"slow\"%'";
        db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap() as usize
    }

    /// The database refuses to record events of `kind` while `refuses`,
    /// as a full disk or a failing one would: the write is aborted.
    pub fn db_refuses(&self, kind: &str, refuses: bool) {
        let db = rusqlite::Connection::open(&self.db).unwrap();
        db.execute_batch("DROP TRIGGER IF EXISTS refuses").unwrap();
        if refuses {
            let sql = format!(
                "CREATE TRIGGER refuses BEFORE INSERT ON events WHEN NEW.kind = '{kind}' \
                 BEGIN SELECT RAISE(ABORT, 'disk I/O error'); END"
            );
            db.execute_batch(&sql).unwrap();
        }
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
        harnesses::write_session(self, Harness::ClaudeCode, None, &cwd, busy, (self.app_now() / 1000) as u64);
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

/// How long a fixture [`World::holds`] starts lives at most, in 20ms
/// steps: two minutes, well above any scenario's run.
const HOLD_STEPS: u32 = 6000;

impl World {
    /// A shell line that touches `dir/started`, then holds until
    /// `dir/release` is there (exit 0). It also ends, with exit 1, once `dir`
    /// is gone (the world dropped, on a panic too) or after two minutes (the
    /// test binary killed): what a scenario starts never outlives it.
    /// Makes `dir`.
    pub fn holds(&self, dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let d = dir.display();
        format!(
            "touch {d}/started; i=0; while [ -d {d} ] && [ $i -lt {HOLD_STEPS} ]; do \
             [ -e {d}/release ] && exit 0; sleep 0.02; i=$((i+1)); done; exit 1"
        )
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

/// A worker's worktree, as the CLI lists it.
pub fn worktree(w: &Value) -> PathBuf {
    PathBuf::from(w["worktree"].as_str().unwrap())
}

pub fn id(w: &Value) -> Uuid {
    w["worker"].as_str().unwrap().parse().unwrap()
}

/// Runs `case` for each of `inputs` at once, each on a thread and runtime of
/// its own: a scenario that goes through several worlds takes as long as its
/// slowest one, not their sum (factorium#98). Answers what each answered,
/// in order; a case's panic is the scenario's.
pub fn each<I: Send, T: Send, F: Future<Output = T>>(
    inputs: impl IntoIterator<Item = I>,
    case: impl Fn(I) -> F + Sync,
) -> Vec<T> {
    std::thread::scope(|s| {
        let case = &case;
        let running: Vec<_> = inputs.into_iter().map(|i| s.spawn(move || block_on(case(i)))).collect();
        running.into_iter().map(|r| r.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
    })
}

/// Runs `f` to its end on a runtime of its own, as a `#[tokio::test]` does.
pub fn block_on<T>(f: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
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
