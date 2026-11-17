//! Scenarios: each drives `App` as the CLI does and reads only what the
//! CLI would print. Git is real — a bare remote and a clone per repo, copied
//! from one made per run. Orca, the readings, the processes, mytokens and
//! Homebrew are stand-ins the scenario moves; time moves when it moves it.

mod admission;
mod harnesses;
mod line;
mod path;
mod tracker;
mod watch;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use factorium::host::orca::Made;
use factorium::host::{Host, Orca, Proc, Reading};
use factorium::{Answer, App, Tick};
use factum::{Instant, ManualClock, Uuid};
use serde_json::Value;

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
}

#[derive(Default)]
pub struct FakeOrca {
    pub terminals: Mutex<BTreeMap<String, Terminal>>,
    pub removed: Mutex<Vec<PathBuf>>,
    /// Orca refuses to open terminals while set.
    pub refuse_open: Mutex<bool>,
    root: PathBuf,
}

impl Orca for FakeOrca {
    fn worktree(&self, repo: &Path, name: &str, branch: &str, sha: &str) -> Result<Made, String> {
        let path = self.root.join("worktrees").join(name);
        git(repo, &["worktree", "add", "--quiet", "-b", branch, &path.to_string_lossy(), sha])?;
        Ok(Made { path, branch: branch.into() })
    }

    fn remove(&self, worktree: &Path) -> Result<(), String> {
        self.removed.lock().unwrap().push(worktree.to_path_buf());
        if worktree.exists() {
            git(worktree, &["worktree", "remove", "--force", &worktree.to_string_lossy()])?;
        }
        Ok(())
    }

    fn open(&self, worktree: &Path, title: &str, line: &str) -> Result<String, String> {
        if *self.refuse_open.lock().unwrap() {
            return Err("orca terminal create: refused".into());
        }
        let mut t = self.terminals.lock().unwrap();
        let handle = format!("term-{}", t.len() + 1);
        let term = Terminal {
            worktree: worktree.into(),
            title: title.into(),
            line: line.into(),
            alive: true,
            ..Default::default()
        };
        t.insert(handle.clone(), term);
        Ok(handle)
    }

    fn send(&self, terminal: &str, text: &str) -> Result<(), String> {
        let mut t = self.terminals.lock().unwrap();
        let term = t.get_mut(terminal).filter(|t| t.alive).ok_or("no such terminal")?;
        term.sent.push(text.into());
        Ok(())
    }

    fn screen(&self, terminal: &str) -> Option<Vec<String>> {
        self.terminals.lock().unwrap().get(terminal).filter(|t| t.alive).map(|t| t.screen.clone())
    }

    fn close(&self, terminal: &str) {
        if let Some(t) = self.terminals.lock().unwrap().get_mut(terminal) {
            t.alive = false;
        }
    }
}

/// The machine as a scenario has it.
pub struct FakeHost {
    pub orca: FakeOrca,
    pub reading: Mutex<Reading>,
    pub procs: Mutex<Vec<Proc>>,
    /// mytokens: service → what `get --json` prints.
    pub tokens: Mutex<BTreeMap<String, String>>,
    pub brewed: Mutex<Vec<String>>,
    pub home: PathBuf,
}

impl Host for FakeHost {
    fn orca(&self) -> &dyn Orca {
        &self.orca
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
    fn mytokens(&self, args: &[&str]) -> Result<String, String> {
        let t = self.tokens.lock().unwrap();
        match args {
            ["list"] => Ok(t.keys().cloned().collect::<Vec<_>>().join("\n")),
            ["get", service, "--json"] => t.get(*service).cloned().ok_or("no such secret".into()),
            _ => Err("mytokens: unknown".into()),
        }
    }
    fn brew_upgrade(&self, formula: &str) -> Result<String, String> {
        self.brewed.lock().unwrap().push(formula.into());
        Ok(String::new())
    }
    fn home(&self) -> PathBuf {
        self.home.clone()
    }
}

pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)))
    }
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
    pub clock: Arc<ManualClock>,
    pub host: Arc<FakeHost>,
    pub app: App,
}

#[allow(clippy::new_without_default)]
impl World {
    pub fn new() -> World {
        let dir = std::env::temp_dir().join("factorium-scenarios").join(Uuid::now_v7().to_string());
        std::fs::create_dir_all(dir.join("repos")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let clock = Arc::new(ManualClock::new(Instant(1_800_000_000_000)));
        let host = Arc::new(FakeHost {
            orca: FakeOrca { root: dir.clone(), ..Default::default() },
            reading: Mutex::new(Reading {
                cpu_busy: 10,
                memory_free: 60,
                disk_free: 500 * GIB,
                disk_total: 1000 * GIB,
            }),
            procs: Mutex::default(),
            tokens: Mutex::default(),
            brewed: Mutex::default(),
            home: dir.join("home"),
        });
        let app = App::open(&dir.join("data").join("factorium.db"), clock.clone(), "owner", host.clone()).unwrap();
        World { dir, clock, host, app }
    }

    /// Another CLI run on the same database.
    pub fn run(&self) -> App {
        App::open(&self.dir.join("data").join("factorium.db"), self.clock.clone(), "owner", self.host.clone()).unwrap()
    }

    /// A clone of its own remote; answers its path.
    pub fn repo(&self, name: &str) -> PathBuf {
        let remote = self.dir.join("remotes").join(format!("{name}.git"));
        std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
        let t = template().join("remote.git");
        Command::new("cp").args(["-R", &t.to_string_lossy(), &remote.to_string_lossy()]).status().unwrap();
        let path = self.dir.join("repos").join(name);
        git(&self.dir, &["clone", "--quiet", &remote.to_string_lossy(), &path.to_string_lossy()]).unwrap();
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

    pub fn set_screen(&self, handle: &str, lines: &[&str]) {
        let mut t = self.host.orca.terminals.lock().unwrap();
        t.get_mut(handle).unwrap().screen = lines.iter().map(|l| l.to_string()).collect();
    }

    pub fn kill_terminal(&self, handle: &str) {
        self.host.orca.close(handle);
    }

    /// The remote's tip of `branch` of repo `name`.
    pub fn remote_tip(&self, name: &str, branch: &str) -> String {
        git(&self.dir.join("remotes").join(format!("{name}.git")), &["rev-parse", branch]).unwrap()
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

pub fn id(w: &Value) -> Uuid {
    w["worker"].as_str().unwrap().parse().unwrap()
}

/// Commits `file` with `text` in a worktree.
pub fn commit(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]).unwrap();
    git(dir, &["commit", "--quiet", "-m", &format!("write {file}")]).unwrap();
}

pub fn refused(answer: Answer, code: &str) {
    match answer {
        Err(r) => assert_eq!(r.code, code, "{r}"),
        Ok(v) => panic!("expected {code}, got {v}"),
    }
}
