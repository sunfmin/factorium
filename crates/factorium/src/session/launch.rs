//! A session for a role: who runs a command, what each may run, and how a
//! session of a role is opened. A worker's session starts in its worktree
//! with its brief; the watch's in its home, its rules for its brief. Both
//! get the kit's first eligible harness on its account with the most left,
//! the account's secrets by a file, this factorium on PATH and its database,
//! the guard, and their rules. The environment names that tell a session's
//! commands who runs them live here and nowhere else.

use std::path::{Path, PathBuf};

use factorium_domain::kit::{Kits, kit_id};
use factorium_harness::account::{self, Here};
use factorium_harness::{Harness, Kept, RULES, Start};
use factum::Uuid;

use crate::{Answer, App, Refusal, fail};

/// The database a command opens, when `--db` does not say.
pub const DB_ENV: &str = "FACTORIUM_DB";
/// A worker's session: the worker's id.
const WORKER_ENV: &str = "FACTORIUM_WORKER";
/// The watch's session.
const WATCH_ENV: &str = "FACTORIUM_WATCH";
/// The title of the watch's terminal; a worker's is `<checkout>#<n>`.
pub(crate) const WATCH_TITLE: &str = "factorium watch";
/// The rules every worker is given, beside the repo's own.
const WORKER_RULES: &str = include_str!("worker_rules.md");
/// The rules the watch is given; its brief too.
const WATCH_RULES: &str = include_str!("watch_rules.md");

/// Who runs a command, by the environment its session was started with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    Owner,
    /// A worker's session; its id, if the environment holds a good one.
    Worker(Option<Uuid>),
    Watch,
}

impl Caller {
    /// The caller of this process.
    pub fn here() -> Caller {
        if let Some(id) = std::env::var_os(WORKER_ENV) {
            Caller::Worker(id.to_str().and_then(|w| w.parse().ok()))
        } else if std::env::var_os(WATCH_ENV).is_some() {
            Caller::Watch
        } else {
            Caller::Owner
        }
    }

    /// Its name, as the events record who ran a command.
    pub fn by(&self) -> &'static str {
        match self {
            Caller::Owner => "owner",
            Caller::Worker(_) => "worker",
            Caller::Watch => "watch",
        }
    }

    /// The worker it is.
    pub fn worker(&self) -> Answer<Uuid> {
        match self {
            Caller::Worker(Some(id)) => Ok(*id),
            _ => {
                Err(Refusal::new("not_a_worker", format!("only a worker's session runs this: {WORKER_ENV} is not set")))
            }
        }
    }

    /// Refuses the top command `command` (`worker`, `issue`, …) if it may
    /// not run it: a worker runs only `worker`, and only the watch `watch`.
    pub fn may_run(&self, command: &str) -> Answer<()> {
        match (command, self) {
            ("worker" | "guard", _) => Ok(()),
            // The watch lives under the daemon: stopping it ends the watch too.
            ("stop", Caller::Worker(_) | Caller::Watch) => Err(Refusal::new(
                "guarded",
                "the daemon is not stopped from a session: it swaps to a newly installed binary on its own, \
                 between ticks, after any cut (`factorium cut` included)",
            )),
            (_, Caller::Worker(_)) => Err(Refusal::new("guarded", "a worker runs only `factorium worker …`")),
            ("watch", Caller::Owner) => Err(Refusal::new("guarded", "only the watch runs `factorium watch …`")),
            _ => Ok(()),
        }
    }

    /// Refuses the shell `line` its session would run, as the guard hook:
    /// each factorium command in it as [`Caller::may_run`] says, and a
    /// worker never pushes.
    pub fn may_run_line(&self, line: &str) -> Answer<()> {
        // Each command the line runs: what follows a separator, past variable
        // assignments and the words that run the next one (`env`, `exec`, …).
        for segment in line.split(|c: char| ";&|()\n`".contains(c) || c == '$') {
            let mut words = segment.split_whitespace().peekable();
            while let Some(w) = words.peek() {
                let assigns = w.split_once('=').is_some_and(|(n, _)| !n.is_empty() && !n.starts_with('-'));
                let runs_next = ["env", "exec", "command", "nohup", "time", "nice", "xargs"].contains(w);
                if !(assigns || runs_next || (w.starts_with('-') && segment.trim_start().starts_with("env"))) {
                    break;
                }
                words.next();
            }
            let Some(command) = words.next() else { continue };
            match command.rsplit('/').next() {
                Some("factorium") => self.may_run(words.next().unwrap_or(""))?,
                Some("git") if matches!(self, Caller::Worker(_)) => {
                    // The subcommand: past git's own options, `-C dir` and `-c x=y` taking a word.
                    while let Some(w) = words.next() {
                        if w == "-C" || w == "-c" {
                            words.next();
                        } else if !w.starts_with('-') {
                            if w == "push" {
                                let why = "a worker does not push: `factorium worker done` lands your branch";
                                return Err(Refusal::new("guarded", why));
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The role a session is opened for, and where.
pub(crate) enum Role<'a> {
    /// A worker in its worktree, handed `brief`, which `name` names.
    Worker { id: Uuid, worktree: &'a Path, name: &'a str, title: &'a str, brief: &'a str },
    /// The watch, in its home.
    Watch,
}

/// What a session starts on: the kit's choice and its account.
pub(crate) struct Pick {
    pub(crate) harness: Harness,
    /// The account it starts on, if its harness takes one.
    pub(crate) account: Option<String>,
    pub(crate) model: Option<String>,
    command: String,
    /// The account's environment, kept off the terminal's command line.
    secrets: Vec<(&'static str, String)>,
}

/// A session opened: its terminal, the brief it was handed, and the line to
/// type at it to hand it that brief once its harness is up, if its start
/// line does not.
pub(crate) struct Opened {
    pub(crate) terminal: String,
    pub(crate) brief: PathBuf,
    pub(crate) typed: Option<String>,
}

fn io(e: std::io::Error) -> Refusal {
    Refusal::new("io", e.to_string())
}

impl App {
    /// The accounts mytokens holds, as this machine can use them now.
    pub(crate) fn accounts_here(&self) -> Here {
        account::here(&|args| self.host.mytokens(args), &self.host.home())
    }

    /// The watch's home and the brief its session is handed there.
    pub(crate) fn watch_home(&self) -> (PathBuf, PathBuf) {
        let home = self.dir(&["watch"]);
        let brief = home.join("BRIEF.md");
        (home, brief)
    }

    /// What a session for `checkout` (the watch: `""`) starts on now; refused
    /// `no_harness` when nothing of the kit can.
    pub(crate) async fn pick(&self, checkout: &str) -> Answer<Pick> {
        let kit = self.state::<Kits>(kit_id()).await?;
        let accounts = self.accounts_here().usable;
        let here: Vec<(String, String)> = accounts.iter().map(|a| (a.name.clone(), a.harness.key().into())).collect();
        let choice = kit.choose(checkout, self.now(), &here).map_err(fail("no_harness"))?;
        let harness = Harness::read(&choice.entry.harness)
            .ok_or_else(|| Refusal::new("no_harness", choice.entry.harness.clone()))?;
        let secrets = choice
            .account
            .as_ref()
            .and_then(|a| accounts.into_iter().find(|u| &u.name == a))
            .map(|u| u.env)
            .unwrap_or_default();
        Ok(Pick { harness, account: choice.account, model: choice.entry.model, command: choice.entry.command, secrets })
    }

    /// Opens a terminal running a session of `role` on `pick`, its brief
    /// written for it.
    pub(crate) fn open_session(&self, role: Role, pick: &Pick) -> Answer<Opened> {
        let (dir, worktree, brief, title, mut env) = match role {
            Role::Worker { id, worktree, name, title, brief } => {
                let path = self.dir(&["briefs", &format!("{name}.md")]);
                std::fs::create_dir_all(path.parent().expect("a parent")).map_err(io)?;
                std::fs::write(&path, brief).map_err(io)?;
                let dir = self.dir(&["role", "worker"]);
                std::fs::create_dir_all(dir.join(factorium_harness::SKILLS)).map_err(io)?;
                std::fs::write(dir.join(RULES), WORKER_RULES).map_err(io)?;
                (dir, worktree.to_path_buf(), path, title.to_string(), vec![(WORKER_ENV, id.to_string())])
            }
            Role::Watch => {
                let (home, brief) = self.watch_home();
                self.host.git(&home).init("The watch's home").map_err(fail("git"))?;
                let rules = WATCH_RULES.replace("{db}", &self.db.to_string_lossy());
                std::fs::write(home.join(RULES), &rules).map_err(io)?;
                std::fs::write(&brief, &rules).map_err(io)?;
                (home.clone(), home, brief, WATCH_TITLE.to_string(), vec![(WATCH_ENV, "1".to_string())])
            }
        };
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or("factorium".into());
        // The session runs factorium by name: this one, on this database;
        // and finds what the kit's program runs by name beside it.
        env.push((DB_ENV, self.db.to_string_lossy().into_owned()));
        let mut path: Vec<String> = Path::new(&exe).parent().map(|bin| bin.display().to_string()).into_iter().collect();
        path.extend(program_dir(&pick.command, &self.host.home()));
        path.push(std::env::var("PATH").unwrap_or_default());
        env.push(("PATH", path.join(":")));
        let (dir_text, worktree_text, brief_text) =
            (dir.to_string_lossy(), worktree.to_string_lossy(), brief.to_string_lossy());
        let (home, user_home) = (self.session_home(pick.harness, pick.account.as_deref()), self.host.home());
        let secret = self.write_secrets(&pick.secrets)?;
        let secret_text = secret.as_ref().map(|file| file.to_string_lossy().into_owned());
        let start = Start {
            command: &pick.command,
            dir: &dir_text,
            worktree: &worktree_text,
            model: pick.model.as_deref(),
            guard: &exe,
            home: &home,
            user_home: &user_home,
            env,
            brief: &brief_text,
            secrets: secret_text.as_deref(),
        };
        let opened = self.open_terminal(pick, &start, &worktree, &title);
        if let (Err(_), Some(file)) = (&opened, &secret) {
            let _ = std::fs::remove_file(file);
        }
        let (terminal, typed) = opened?;
        Ok(Opened { terminal, brief, typed })
    }

    /// Opens the terminal of the session `start` says: its terminal, and
    /// the line to type at it once its harness is up.
    fn open_terminal(&self, pick: &Pick, start: &Start, at: &Path, title: &str) -> Answer<(String, Option<String>)> {
        let launched = pick.harness.launch(start).map_err(|e| Refusal::new("harness", e))?;
        for kept in &launched.kept {
            keep(kept).map_err(io)?;
        }
        let terminal = self.host.orca().open(at, title, &launched.line).map_err(fail("orca"))?;
        Ok((terminal, launched.typed))
    }

    /// Writes `secrets`, if any, to a file only this user may read, which
    /// the start line reads first and removes: its path.
    fn write_secrets(&self, secrets: &[(&'static str, String)]) -> Answer<Option<PathBuf>> {
        if secrets.is_empty() {
            return Ok(None);
        }
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let dir = self.dir(&["secrets"]);
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(io)?;
        let file = dir.join(format!("{}.sh", uuid::Uuid::now_v7()));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(io)?;
        std::io::Write::write_all(&mut f, factorium_harness::secrets_file(secrets).as_bytes()).map_err(io)?;
        Ok(Some(file))
    }
}

/// Puts one thing a session's home holds in place. A file is written whole
/// and then moved in, for its user alone: a session starting beside it reads
/// the last one written, never half of one.
fn keep(kept: &Kept) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let (Kept::Dir { path } | Kept::File { path, .. } | Kept::Link { path, .. }) = kept;
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))?;
    match kept {
        Kept::Dir { path } => std::fs::create_dir_all(path)?,
        Kept::Link { path, to } => {
            if path.symlink_metadata().is_err() {
                std::os::unix::fs::symlink(to, path)?;
            }
        }
        Kept::File { path, text } => {
            let fresh = path.with_extension(format!("{}.new", uuid::Uuid::now_v7()));
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&fresh)?;
            std::io::Write::write_all(&mut f, text.as_bytes())?;
            std::fs::rename(&fresh, path)?;
        }
    }
    Ok(())
}

/// The directory of the program a kit's `command` names by path, as its
/// first word: absolute, or under the host user's `home` as `~/`. A program
/// named by name is found on the PATH already.
fn program_dir(command: &str, home: &Path) -> Option<String> {
    let program = command.split_whitespace().next()?;
    let path = match program.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if program.starts_with('/') => PathBuf::from(program),
        None => return None,
    };
    Some(path.parent()?.display().to_string())
}
