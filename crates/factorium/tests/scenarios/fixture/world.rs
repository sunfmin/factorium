//! A world: a scenario's own directory, database, machine and clock, and the
//! `App` it drives as the CLI does. What it starts never outlives it.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use factorium::host::{Clock, ManualClock, Reading};
use factorium::{Answer, App, Data, Tick};
use factum::Uuid;
use serde_json::Value;

use super::fakes::{FakeHost, git_of, make_home};
use super::{Terminal, worker};

pub const GIB: u64 = 1 << 30;

/// A kit entry for Claude Code.
pub const CLAUDE: &str = "claude-code=claude --dangerously-skip-permissions";

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
        let clock = Arc::new(ManualClock::new(1_800_000_000_000));
        let reading = Reading { cpu_busy: 10, memory_free: 60, disk_free: 500 * GIB, disk_total: 1000 * GIB };
        let host = Arc::new(FakeHost::at(&dir, clock.clone(), reading));
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
        self.keep_heavy_trace();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
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

/// A factorium binary as a scenario has one: it answers `doctor` on the
/// SQLite file `--db` names, and notes each database it was run on in
/// `proved` beside it.
pub const OPENS: &str = r#"#!/bin/sh
echo "$2" >> "$(dirname "$0")/proved"
[ "$(head -c 15 "$2")" = 'SQLite format 3' ] || { echo "not a database: $2"; exit 1; }
echo '{"findings": []}'
"#;

/// One that needs a newer database than this host's: a column its store
/// lacks, as 0.1.42 needed `taken_over` (factorium#141).
pub const NEEDS_NEWER: &str = r#"#!/bin/sh
echo "$2" >> "$(dirname "$0")/proved"
grep -q 'handed_back INTEGER' "$2" || { echo 'Error: no such column: handed_back'; exit 1; }
"#;

/// One that opens the database, and may not be run after (no longer
/// executable): what an exec of it meets.
pub const OPENS_ONCE: &str = r#"#!/bin/sh
chmod -x "$0"
echo '{"findings": []}'
"#;

/// The databases the binary of `formula`'s `version` was run on, oldest first.
pub fn proved(w: &World, formula: &str, version: &str) -> Vec<PathBuf> {
    let at = w.dir.join("brew/Cellar").join(formula).join(version).join("bin/proved");
    std::fs::read_to_string(at).unwrap_or_default().lines().map(PathBuf::from).collect()
}

/// Writes `script` at `path` as a program.
pub fn program(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl World {
    /// Makes the daemon `app` run as version 0.0.9 of `formula`, as
    /// Homebrew installs it, a binary that [`OPENS`] the database; answers
    /// the link Homebrew keeps to it.
    pub fn running_as(&self, app: &App, formula: &str) -> PathBuf {
        let bin = self.dir.join("brew/Cellar").join(formula).join("0.0.9/bin/factorium");
        program(&bin, OPENS);
        let opt = self.dir.join("brew/opt").join(formula);
        std::fs::create_dir_all(opt.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(bin.parent().unwrap().parent().unwrap(), &opt).unwrap();
        self.host.brew.lock().unwrap().installed.insert(formula.into(), "0.0.9".into());
        app.runs_as(&bin.canonicalize().unwrap());
        opt.join("bin/factorium")
    }

    /// The binary that `formula`'s next cut installs is `script`: as
    /// Homebrew copies it from the version installed.
    pub fn next_binary(&self, formula: &str, script: &str) {
        program(&self.dir.join("brew/Cellar").join(formula).join("0.0.9/bin/factorium"), script);
    }

    /// Homebrew links `formula` 0.1.0, by hand, its binary `script`.
    pub fn links_newer(&self, formula: &str, script: &str) {
        let keg = self.dir.join("brew/Cellar").join(formula).join("0.1.0");
        program(&keg.join("bin/factorium"), script);
        let opt = self.dir.join("brew/opt").join(formula);
        std::fs::remove_file(&opt).unwrap();
        std::os::unix::fs::symlink(&keg, &opt).unwrap();
    }
}
