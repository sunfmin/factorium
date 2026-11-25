//! A session for a role: who runs a command, what each may run, and how a
//! session of a role is opened. A worker's session starts in its worktree
//! with its brief; the watch's in its home, its rules for its brief. Both
//! get the engine, model and account stablehand picks, started by the command stablehand keeps for that engine,
//! the account's login (`stablehand credential`: a token by a file, or its login files linked), this factorium on PATH and its database,
//! the guard, and their rules; a worker whose role changes under it is
//! told once. The environment names that tell a session's
//! commands who runs them live here and nowhere else.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use factorium_domain::issue::On;
use factorium_domain::kit::{Ask, Choice, Kit, Kits, Picked, kit_id};
use factorium_domain::outbox::Sender;
use factorium_domain::worker::{NoteRoleTold, RoleRead, TellRole, Workers};
use factorium_harness::{Call, Harness, Kept, RULES, Start};
use factum::{Uuid, Value};
use serde_json::json;

use crate::accounts::{Credential, engine_commands};

use crate::kept::WorkerBranch;
use crate::{Answer, App, Refusal, Want, fail};

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

    /// The guard hook's answer to the tool call `payload` harness `harness`
    /// is about to make: nothing when it may run, else why not and the exit
    /// code that harness refuses with. A call it cannot read is refused, and
    /// so is any call of a harness it does not know, with 2, the code every
    /// harness refuses with.
    pub fn guard(&self, harness: &str, payload: &str) -> Result<(), (u8, String)> {
        let Some(h) = Harness::read(harness) else {
            return Err((2, format!("the guard knows no harness `{harness}`")));
        };
        let why = match h.shell_call(payload) {
            Call::NotShell => return Ok(()),
            Call::Unreadable(why) => format!("the guard cannot read this tool call ({why})"),
            Call::Shell(line) => match self.may_run_line(&line) {
                Ok(()) => return Ok(()),
                Err(refusal) => refusal.message,
            },
        };
        Err((h.refused(), why))
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
    /// `role`: the role directory its issue names, whose rules and skills it
    /// starts with beside the worker's rules.
    Worker { id: Uuid, worktree: &'a Path, name: &'a str, title: &'a str, brief: &'a str, role: Option<&'a str> },
    /// The watch, in its home.
    Watch,
}

/// What a session starts on: the kit's choice and its account.
pub(crate) struct Pick {
    pub(crate) harness: Harness,
    /// The account it starts on, if its harness takes one.
    pub(crate) account: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) command: String,
    /// The home this host keeps for sessions on the account's bound login;
    /// `None`: no account, or a portable login.
    pub(crate) home: Option<PathBuf>,
    /// The account's portable login, its variables, kept off the terminal's
    /// command line.
    pub(crate) secrets: Vec<(String, String)>,
    /// The account's bound login's files, linked into its home (`Start::logins`).
    pub(crate) logins: Vec<(String, PathBuf)>,
}

/// A session opened: its terminal, the brief it was handed, and the line to
/// type at it to hand it that brief once its harness is up, if its start
/// line does not.
pub(crate) struct Opened {
    pub(crate) terminal: String,
    pub(crate) brief: PathBuf,
    pub(crate) typed: Option<String>,
    /// The watch's: the hash of the rules it was briefed with.
    pub(crate) rules: Option<String>,
    /// A worker's with a role: what its role gave it ([`role_dir`]).
    pub(crate) role: Option<RoleRead>,
}

fn io(e: std::io::Error) -> Refusal {
    Refusal::new("io", e.to_string())
}

impl App {
    /// The watch's home and the brief its session is handed there.
    pub(crate) fn watch_home(&self) -> (PathBuf, PathBuf) {
        (self.data().watch(), self.data().watch_brief())
    }

    /// The rules this binary briefs the watch with, on this database.
    fn watch_rules(&self) -> String {
        WATCH_RULES.replace("{db}", &self.db.to_string_lossy())
    }

    /// The hash of the rules this binary briefs the watch with: a watch
    /// briefed with others is restarted between rounds.
    pub(crate) fn watch_rules_hash(&self) -> String {
        rules_hash_of(&self.watch_rules())
    }

    /// What a session for `checkout` (the watch: `""`) starts on now, as
    /// `on` names (its issue's account and model, if any), with its
    /// account's environment; refused `no_harness` when it cannot.
    pub(crate) async fn pick(&self, checkout: &str, on: &On) -> Answer<Pick> {
        let choice = self.choice(checkout, on).await?;
        self.pick_of(choice)
    }

    /// What [`App::pick`] would start a session for `checkout` on now, its
    /// account's environment not read: whether one can start. One
    /// `stablehand pick`, narrowed by the checkout's pin and the issue's
    /// model; an issue naming an account narrows it to that account's
    /// engine.
    pub(crate) async fn choice(&self, checkout: &str, on: &On) -> Answer<Choice> {
        let model = on.model.as_deref();
        let Some(account) = on.account.as_deref() else {
            return self
                .choose_by(|kit, commands, now, pick| {
                    kit.choose(checkout, model, now, commands, pick(&kit.ask(checkout, model)))
                })
                .await;
        };
        let kept = self.kept_account(account).map_err(|r| Refusal::new("no_harness", r.message))?;
        if let Some(why) = kept.unusable {
            return Err(Refusal::new("no_harness", format!("account {account} cannot be used here: {why}")));
        }
        let engine = kept.engine;
        self.choose_by(|kit, commands, _, pick| {
            let ask = Ask { engines: vec![engine.clone()], model: model.map(Into::into) };
            kit.choose_on(checkout, account, &engine, model, commands, &pick(&ask)?)
        })
        .await
    }

    /// What a session starts on now, as `choose` chooses from the kit, the
    /// time and stablehand's pick, with its account's environment; refused
    /// `no_harness` when it cannot.
    pub(crate) async fn pick_by(
        &self,
        choose: impl FnOnce(
            &Kit,
            &BTreeMap<String, String>,
            i64,
            &mut dyn FnMut(&Ask) -> Result<Picked, String>,
        ) -> Result<Choice, String>,
    ) -> Answer<Pick> {
        let choice = self.choose_by(choose).await?;
        self.pick_of(choice)
    }

    /// What `choose` chooses from the kit, each engine's command (the one
    /// stablehand keeps, `stablehand command show`, else its bare program),
    /// the time and stablehand's pick (`stablehand pick`), told the
    /// sessions factorium runs on each account; refused `no_harness` when
    /// it cannot. While stablehand's commands cannot be read, every pick
    /// fails as that read did.
    async fn choose_by(
        &self,
        choose: impl FnOnce(
            &Kit,
            &BTreeMap<String, String>,
            i64,
            &mut dyn FnMut(&Ask) -> Result<Picked, String>,
        ) -> Result<Choice, String>,
    ) -> Answer<Choice> {
        let kit = self.state::<Kits>(kit_id()).await?;
        let running = self.running().await?;
        let kept = self.kept_commands().map_err(|r| r.message);
        let commands = engine_commands(kept.as_ref().unwrap_or(&BTreeMap::new()));
        let mut pick = |ask: &Ask| match &kept {
            Ok(_) => self.picked(ask, &running),
            Err(why) => Err(why.clone()),
        };
        choose(&kit, &commands, self.now(), &mut pick).map_err(fail("no_harness"))
    }

    /// A session on `choice`, handed its account's login as stablehand
    /// gives it (`stablehand credential`).
    fn pick_of(&self, choice: Choice) -> Answer<Pick> {
        let harness =
            Harness::read(&choice.harness).ok_or_else(|| Refusal::new("no_harness", choice.harness.clone()))?;
        let (home, credential) = match &choice.account {
            Some(a) => {
                let credential = self.credential(a).map_err(|r| Refusal::new("no_harness", r.message))?;
                let home = (!credential.files.is_empty()).then(|| self.data().kept_on(harness.key(), a));
                (home, credential)
            }
            None => (None, Credential { env: Vec::new(), files: Vec::new() }),
        };
        let (model, command) = (choice.model, choice.command);
        let (secrets, logins) = (credential.env, credential.files);
        Ok(Pick { harness, account: choice.account, home, model, command, secrets, logins })
    }

    /// Opens a terminal running a session of `role` on `pick`, its brief
    /// written for it.
    pub(crate) fn open_session(&self, role: Role, pick: &Pick) -> Answer<Opened> {
        let (mut rules_hash, mut role_read) = (None, None);
        let (dir, worktree, brief, title, mut env) = match role {
            Role::Worker { id, worktree, name, title, brief, role } => {
                let path = self.data().brief(name);
                std::fs::create_dir_all(path.parent().expect("a parent")).map_err(io)?;
                std::fs::write(&path, brief).map_err(io)?;
                let dir = match role {
                    None => self.data().worker_role(),
                    Some(_) => self.data().issue_role(name),
                };
                role_read = role_dir(&dir, role.map(Path::new)).map_err(io)?;
                (dir, worktree.to_path_buf(), path, title.to_string(), vec![(WORKER_ENV, id.to_string())])
            }
            Role::Watch => {
                let (home, brief) = self.watch_home();
                self.host.git(&home).init("The watch's home").map_err(fail("git"))?;
                let rules = self.watch_rules();
                std::fs::write(home.join(RULES), &rules).map_err(io)?;
                std::fs::write(&brief, &rules).map_err(io)?;
                rules_hash = Some(rules_hash_of(&rules));
                (home.clone(), home, brief, WATCH_TITLE.to_string(), vec![(WATCH_ENV, "1".to_string())])
            }
        };
        // The session runs factorium by name: this one, on this database.
        env.push((DB_ENV, self.db.to_string_lossy().into_owned()));
        let session = SessionEnv::new(self, pick)?;
        env.push(("PATH", session.path().to_string()));
        let (dir_text, worktree_text, brief_text) =
            (dir.to_string_lossy(), worktree.to_string_lossy(), brief.to_string_lossy());
        let (home, user_home) = (self.home_at(pick.harness, pick.home.as_deref()), self.host.home());
        let start = Start {
            command: &pick.command,
            dir: &dir_text,
            worktree: &worktree_text,
            model: pick.model.as_deref(),
            guard: &session.exe,
            home: &home,
            user_home: &user_home,
            logins: &pick.logins,
            env,
            brief: &brief_text,
            secrets: session.secrets_file(),
        };
        let (terminal, typed) = self.open_terminal(pick, &start, &worktree, &title)?;
        session.hand_over();
        Ok(Opened { terminal, brief, typed, rules: rules_hash, role: role_read })
    }

    /// A role changed under a live worker (factorium#273): its session read
    /// its role once, at its start, and a role directory is linked live.
    /// Tells each live worker whose role changed since it last knew of it,
    /// once, through its outbox as a comment is; its session's rules are
    /// written again with the role's new ones, and what it was told is
    /// recorded. One waiting for the watch, unless parked on its engine's
    /// limit, is told once it works again. What each is owed is recorded
    /// first, then all of them are typed at at once (factorium#279).
    /// Answers each one told.
    pub(crate) async fn tell_role_changes(&self) -> Answer<Vec<Value>> {
        let mut owed = Vec::new();
        for row in self.live_workers().await? {
            let wk = self.state::<Workers>(row.worker).await?;
            let Some(known) = wk.role_known() else { continue };
            let st = wk.started.as_ref().expect("a worker with a role started");
            let dir = &st.role.as_ref().expect("it has a role").dir;
            let now = read_role(Path::new(dir)).map_err(|e| Refusal::new("io", format!("its role {dir}: {e}")))?;
            if now.hash == known.hash {
                continue;
            }
            let Some(live) = self.live_or_none(row.worker, Want::Any).await? else { continue };
            if live.waiting && !live.on_limit {
                continue;
            }
            let changed = now.changed_from(&known.files);
            let name = WorkerBranch { checkout: st.checkout.clone(), number: st.number, attempt: st.attempt }.worker();
            let session_dir = self.data().issue_role(&name);
            let mut also = String::new();
            if changed.iter().any(|f| f == RULES) {
                write_role_rules(&session_dir, Path::new(dir)).map_err(|e| Refusal::new("io", e.to_string()))?;
                also = format!(
                    ", and your own rules, written again with its new ones: `{}`",
                    session_dir.join(RULES).display()
                );
            }
            let text = format!(
                "your role `{dir}` changed since you last read it: {}. Before your next step, re-read its core skill and each changed file{also}.",
                changed.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
            );
            let n = self.give(&live.session, Sender::Role, &text).await?;
            self.submit::<NoteRoleTold>(TellRole { worker: row.worker, read: now }).await?;
            owed.push((row.worker, changed, live.session, n));
        }
        let given: Vec<_> = owed.iter().map(|(_, _, s, n)| (s, Some((*n, false)))).collect();
        let deliveries = self.flush(&given).await?;
        let told = owed.iter().zip(deliveries).map(|((worker, changed, _, _), delivery)| {
            json!({ "worker": worker, "changed": changed, "delivery": delivery.expect("the message given is tried").word() })
        });
        Ok(told.collect())
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
}

/// The environment a session on a pick runs in, a terminal's or a headless
/// ask's: its PATH, and its account's secrets in a file only this user may
/// read, which its line reads first and removes. Dropped before a shell took
/// the file over ([`SessionEnv::hand_over`]), it removes the file itself.
pub(crate) struct SessionEnv {
    /// This factorium's executable.
    pub(crate) exe: String,
    path: String,
    secrets: Option<String>,
}

impl SessionEnv {
    pub(crate) fn new(app: &App, pick: &Pick) -> Answer<SessionEnv> {
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or("factorium".into());
        // This factorium, then the kit program's directory: what it runs by name is beside it.
        let mut path: Vec<String> = Path::new(&exe).parent().map(|bin| bin.display().to_string()).into_iter().collect();
        path.extend(program_dir(&pick.command, &app.host.home()));
        path.push(std::env::var("PATH").unwrap_or_default());
        let secrets = (!pick.secrets.is_empty()).then(|| write_secrets(app, &pick.secrets)).transpose()?;
        Ok(SessionEnv { exe, path: path.join(":"), secrets })
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn secrets_file(&self) -> Option<&str> {
        self.secrets.as_deref()
    }

    /// Hands the secrets file to the shell that started: its line removes it.
    pub(crate) fn hand_over(mut self) {
        self.secrets = None;
    }
}

impl Drop for SessionEnv {
    fn drop(&mut self) {
        let _ = self.secrets.take().map(std::fs::remove_file);
    }
}

/// Writes `secrets` to a new file only this user may read: its path.
fn write_secrets(app: &App, secrets: &[(String, String)]) -> Answer<String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = app.data().secrets();
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(io)?;
    let file = dir.join(format!("{}.sh", uuid::Uuid::now_v7()));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(io)?;
    std::io::Write::write_all(&mut f, factorium_harness::secrets_file(secrets).as_bytes()).map_err(io)?;
    Ok(file.to_string_lossy().into_owned())
}

/// Makes `dir` the directory a worker's session starts in: its rules
/// ([`RULES`]) the worker's, then, where its issue names a role, that role's
/// own rules; its skills ([`factorium_harness::SKILLS`]) the role's, by a
/// link, else none. The role's directory is read, never written. Answers,
/// where there is a role, what it gave: its rules and every file under its
/// skills, read now ([`read_role`]).
fn role_dir(dir: &Path, role: Option<&Path>) -> std::io::Result<Option<RoleRead>> {
    let skills = dir.join(factorium_harness::SKILLS);
    let Some(role) = role else {
        std::fs::create_dir_all(&skills)?;
        return std::fs::write(dir.join(RULES), WORKER_RULES).map(|()| None);
    };
    std::fs::create_dir_all(skills.parent().expect("a parent"))?;
    let _ = std::fs::remove_file(&skills).or_else(|_| std::fs::remove_dir_all(&skills));
    match role.join(factorium_harness::SKILLS) {
        theirs if theirs.is_dir() => std::os::unix::fs::symlink(theirs, &skills)?,
        _ => std::fs::create_dir_all(&skills)?,
    }
    write_role_rules(dir, role)?;
    read_role(role).map(Some)
}

/// Writes the rules of a worker's session in `dir`: the worker's, then
/// `role`'s own, as they are now.
fn write_role_rules(dir: &Path, role: &Path) -> std::io::Result<()> {
    let own = std::fs::read_to_string(role.join(RULES)).unwrap_or_default();
    std::fs::write(dir.join(RULES), format!("{WORKER_RULES}\n# Your role ({})\n\n{own}", role.display()))
}

/// What role directory `role` holds now: a hash of its rules and of every
/// file under its skills, followed through links, each file's path under
/// the skills and its bytes, in path order, each part led by its length so
/// no two contents read alike; and each of those files by its path under
/// `role`, with a hash of its bytes.
fn read_role(role: &Path) -> std::io::Result<RoleRead> {
    let rules = std::fs::read(role.join(RULES)).unwrap_or_default();
    let skills = role.join(factorium_harness::SKILLS);
    let mut found = Vec::new();
    if skills.is_dir() {
        files_under(&skills, Path::new(""), &mut found)?;
    }
    found.sort();
    let mut all = Vec::new();
    let mut part = |bytes: &[u8]| {
        all.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        all.extend_from_slice(bytes);
    };
    part(&rules);
    let mut files = BTreeMap::from([(RULES.to_string(), hash_of(&rules))]);
    for rel in found {
        let bytes = std::fs::read(skills.join(&rel))?;
        part(rel.to_string_lossy().as_bytes());
        part(&bytes);
        files.insert(Path::new(factorium_harness::SKILLS).join(&rel).to_string_lossy().into_owned(), hash_of(&bytes));
    }
    Ok(RoleRead { hash: hash_of(&all), files })
}

/// Each file under `dir/rel`, by its path under `dir`, links followed.
fn files_under(dir: &Path, rel: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir.join(rel))? {
        let rel = rel.join(entry?.file_name());
        if std::fs::metadata(dir.join(&rel))?.is_dir() {
            files_under(dir, &rel, files)?;
        } else {
            files.push(rel);
        }
    }
    Ok(())
}

/// A hash of `rules` that is the same from one build to the next.
fn rules_hash_of(rules: &str) -> String {
    hash_of(rules.as_bytes())
}

/// A hash of `bytes` that is the same from one build to the next (64-bit
/// FNV-1a), as hex.
fn hash_of(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
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

/// The directory of the program a session's `command` names by path, as its
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
