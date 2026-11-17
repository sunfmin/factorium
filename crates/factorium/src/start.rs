//! Starting sessions: a worker on an issue, and the watch. A worker gets a
//! worktree branched from the proving tip, the first eligible harness of the
//! kit on its account with the most left, the guard, and its brief.

use std::path::{Path, PathBuf};

use factorium_domain::issue::{Take, TakeIssue};
use factorium_domain::kit::{Kits, kit_id};
use factorium_domain::watch::{self, StartWatch, Watches, watch_id};
use factorium_domain::worker::{self as w, Started};
use factorium_domain::worker_id;
use factorium_harness::account::{self, Source};
use factorium_harness::{Harness, Kept, RULES, Start};
use factorium_host::{Git, Host, Term};
use factum::Value;
use serde_json::json;

use crate::{Answer, App, Refusal, fail};

/// The rules every worker is given, beside the repo's own.
const WORKER_RULES: &str = include_str!("worker_rules.md");
/// The title of the watch's terminal; a worker's is `<checkout>#<n>`.
pub(crate) const WATCH_TITLE: &str = "factorium watch";
/// The rules the watch is given.
const WATCH_RULES: &str = include_str!("watch_rules.md");

/// An account usable here: its name, its harness key, and the environment a
/// session on it starts with.
type Usable = (String, String, Vec<(&'static str, String)>);

/// mytokens, through the host.
struct Tokens<'a>(&'a dyn Host);

impl Source for Tokens<'_> {
    fn list(&self) -> Result<String, String> {
        self.0.mytokens(&["list"])
    }

    fn get(&self, service: &str) -> Result<String, String> {
        self.0.mytokens(&["get", service, "--json"])
    }
}

/// What a session is started with beyond the kit's entry.
struct Launch<'a> {
    harness: Harness,
    /// The account it starts on, if its harness takes one.
    account: Option<&'a str>,
    command: &'a str,
    model: Option<&'a str>,
    /// Where it starts, holding its rules.
    dir: &'a Path,
    worktree: &'a Path,
    env: Vec<(&'static str, String)>,
    /// The account's secrets: kept off the terminal's command line, in a
    /// file only this user reads, which the line reads and removes.
    secrets: Vec<(&'static str, String)>,
    brief: &'a Path,
    title: &'a str,
}

impl App {
    /// The accounts usable on this machine now: (name, harness key, the
    /// environment a session on it starts with).
    pub(crate) fn accounts_here(&self) -> Vec<Usable> {
        account::usable(&Tokens(self.host.as_ref()), &self.host.home())
            .into_iter()
            .map(|a| (a.name, a.harness.key().to_string(), a.env))
            .collect()
    }

    /// `line`, reading `secrets` first from a file only this user may read,
    /// which it removes: a secret on a command line shows in `ps`, in the
    /// terminal's scrollback and in any error naming the line.
    fn secrets_first(&self, secrets: &[(&'static str, String)], line: String) -> Answer<(String, Option<PathBuf>)> {
        if secrets.is_empty() {
            return Ok((line, None));
        }
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let io = |e: std::io::Error| Refusal::new("io", e.to_string());
        let dir = self.dir(&["secrets"]);
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(io)?;
        let file = dir.join(format!("{}.sh", uuid::Uuid::now_v7()));
        let text: String = secrets.iter().map(|(n, v)| format!("export {n}={}\n", shell_word(v))).collect();
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(io)?;
        std::io::Write::write_all(&mut f, text.as_bytes()).map_err(io)?;
        let word = shell_word(&file.to_string_lossy());
        Ok((format!(". {word} && rm -f {word} && {line}"), Some(file)))
    }

    /// The directory a role's sessions start in, holding its rules.
    fn role(&self, name: &str, rules: &str) -> Answer<PathBuf> {
        let dir = self.dir(&["role", name]);
        std::fs::create_dir_all(dir.join(factorium_harness::SKILLS)).map_err(|e| Refusal::new("io", e.to_string()))?;
        std::fs::write(dir.join(RULES), rules).map_err(|e| Refusal::new("io", e.to_string()))?;
        Ok(dir)
    }

    /// Opens a terminal running a session as `l` says, and hands it its brief.
    fn launch(&self, l: Launch) -> Answer<String> {
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or("factorium".into());
        let hook = format!("{} guard {}", shell_word(&exe), l.harness.key());
        // The session runs factorium by name: this one, on this database.
        let mut env = l.env;
        env.push(("FACTORIUM_DB", self.db.to_string_lossy().into_owned()));
        let path = std::env::var("PATH").unwrap_or_default();
        if let Some(bin) = Path::new(&exe).parent() {
            env.push(("PATH", format!("{}:{path}", bin.display())));
        }
        let (dir, worktree) = (l.dir.to_string_lossy(), l.worktree.to_string_lossy());
        let brief = l.brief.to_string_lossy();
        let (home, user_home) = (self.session_home(l.harness, l.account), self.host.home());
        let start = Start {
            command: l.command,
            dir: &dir,
            worktree: &worktree,
            model: l.model,
            hook: &hook,
            home: &home,
            user_home: &user_home,
            env,
            brief: &brief,
        };
        let launched = l.harness.launch(&start).map_err(|e| Refusal::new("harness", e))?;
        for kept in &launched.kept {
            keep(kept).map_err(|e| Refusal::new("io", e.to_string()))?;
        }
        let (line, typed) = (launched.line, launched.typed);
        let (line, secret) = self.secrets_first(&l.secrets, line)?;
        let opened = self.host.orca().open(l.worktree, l.title, &line);
        if let (Err(_), Some(file)) = (&opened, &secret) {
            let _ = std::fs::remove_file(file);
        }
        let terminal = opened.map_err(fail("orca"))?;
        if let Some(typed) = typed {
            // Past a minute it is typed anyway, as a person would.
            let _ = self.host.orca().ready(&terminal);
            if let Err(e) = self.host.orca().send(&terminal, &typed) {
                // No session holds it: what factorium opens, factorium closes.
                self.host.orca().close(&terminal);
                return Err(Refusal::new("orca", e));
            }
        }
        Ok(terminal)
    }

    /// Starts a worker on issue `number` of `checkout`.
    pub(crate) async fn start_worker(&self, checkout: &str, number: u64) -> Answer {
        let c = self.checkout(checkout).await?;
        let kit = self.state::<Kits>(kit_id()).await?;
        let accounts = self.accounts_here();
        let here: Vec<(String, String)> = accounts.iter().map(|(n, h, _)| (n.clone(), h.clone())).collect();
        let choice = kit.choose(checkout, self.now(), &here).map_err(fail("no_harness"))?;
        let harness = Harness::read(&choice.entry.harness)
            .ok_or_else(|| Refusal::new("no_harness", choice.entry.harness.clone()))?;
        let issue = self.issue(checkout, number).await?;
        let attempt = issue.attempts + 1;
        let worker = worker_id(checkout, number, attempt);
        self.submit::<TakeIssue>(Take { checkout: checkout.into(), number, worker }).await?;
        let started = self.open_worker(&c, &issue, attempt, worker, harness, &choice, &accounts).await;
        match started {
            Ok(st) => {
                let shown = json!({ "worker": worker, "issue": format!("{checkout}#{number}"), "harness": st.harness, "account": st.account, "worktree": st.worktree });
                self.submit::<w::StartWorker>(w::Start { worker, started: st }).await?;
                Ok(shown)
            }
            Err(r) => {
                let release = Take { checkout: checkout.into(), number, worker };
                self.submit::<factorium_domain::issue::ReleaseIssue>(release).await?;
                Err(r)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_worker(
        &self,
        c: &factorium_domain::checkout::Checkout,
        issue: &factorium_domain::issue::Issue,
        attempt: u32,
        worker: factum::Uuid,
        harness: Harness,
        choice: &factorium_domain::kit::Choice,
        accounts: &[Usable],
    ) -> Answer<Started> {
        let git = Git::at(&c.path);
        let tip = git.fetch(&c.proving).map_err(fail("git"))?;
        let name = format!("{}-{}-{attempt}", c.name, issue.number);
        let branch = format!("factorium/{name}");
        let made = self.host.orca().worktree(Path::new(&c.path), &name, &branch, &tip).map_err(fail("orca"))?;
        let brief = self.dir(&["briefs", &format!("{name}.md")]);
        std::fs::create_dir_all(brief.parent().expect("a parent")).map_err(|e| Refusal::new("io", e.to_string()))?;
        std::fs::write(&brief, worker_brief(c, issue, &branch)).map_err(|e| Refusal::new("io", e.to_string()))?;
        let mut secrets = Vec::new();
        let env = vec![("FACTORIUM_WORKER", worker.to_string())];
        if let Some(a) = &choice.account {
            secrets.extend(accounts.iter().find(|(n, ..)| n == a).map(|(.., v)| v.clone()).unwrap_or_default());
        }
        let role = self.role("worker", WORKER_RULES)?;
        let title = format!("{}#{}", c.name, issue.number);
        let launch = Launch {
            harness,
            account: choice.account.as_deref(),
            command: &choice.entry.command,
            model: choice.entry.model.as_deref(),
            dir: &role,
            worktree: &made.path,
            env,
            secrets,
            brief: &brief,
            title: &title,
        };
        let terminal = self.launch(launch)?;
        Ok(Started {
            checkout: c.name.clone(),
            number: issue.number,
            attempt,
            harness: harness.key().into(),
            account: choice.account.clone(),
            model: choice.entry.model.clone(),
            worktree: made.path.to_string_lossy().into_owned(),
            branch: made.branch,
            terminal,
            brief: brief.to_string_lossy().into_owned(),
            base: tip,
            at: 0,
        })
    }

    /// Starts the watch's session if the kit has a harness and none runs:
    /// answers its terminal, or `None` when it cannot run.
    pub(crate) async fn ensure_watch(&self, before: &watch::Watch) -> Answer<Option<String>> {
        if let Some(t) = &before.terminal {
            match self.host.orca().screen(t) {
                Term::Running(_) => return Ok(Some(t.clone())),
                // Not known to be gone: no second session; none to type at now.
                Term::Unreadable(_) => return Ok(None),
                Term::Exited => {}
            }
        }
        let kit = self.state::<Kits>(kit_id()).await?;
        let accounts = self.accounts_here();
        let here: Vec<(String, String)> = accounts.iter().map(|(n, h, _)| (n.clone(), h.clone())).collect();
        let Ok(choice) = kit.choose("", self.now(), &here) else { return Ok(None) };
        let Some(harness) = Harness::read(&choice.entry.harness) else { return Ok(None) };
        let (home, brief) = self.watch_home();
        if !home.join(".git").exists() {
            std::fs::create_dir_all(&home).map_err(|e| Refusal::new("io", e.to_string()))?;
            Git::at(&home).run(&["init", "--quiet"]).map_err(fail("git"))?;
            Git::at(&home)
                .run(&["commit", "--quiet", "--allow-empty", "-m", "The watch's home"])
                .map_err(fail("git"))?;
        }
        let rules = WATCH_RULES.replace("{db}", &self.db.to_string_lossy());
        std::fs::write(home.join(RULES), &rules).map_err(|e| Refusal::new("io", e.to_string()))?;
        std::fs::write(&brief, &rules).map_err(|e| Refusal::new("io", e.to_string()))?;
        let mut secrets = Vec::new();
        let env = vec![("FACTORIUM_WATCH", "1".to_string())];
        if let Some(a) = &choice.account {
            secrets.extend(accounts.iter().find(|(n, ..)| n == a).map(|(.., v)| v.clone()).unwrap_or_default());
        }
        let launch = Launch {
            harness,
            account: choice.account.as_deref(),
            command: &choice.entry.command,
            model: choice.entry.model.as_deref(),
            dir: &home,
            worktree: &home,
            env,
            secrets,
            brief: &brief,
            title: WATCH_TITLE,
        };
        let terminal = self.launch(launch)?;
        self.submit::<StartWatch>(watch::Started {
            terminal: terminal.clone(),
            harness: harness.key().into(),
            account: choice.account.clone(),
        })
        .await?;
        let _ = self.state::<Watches>(watch_id()).await?;
        Ok(Some(terminal))
    }
}

/// The brief a worker is handed: its issue and how it finishes.
fn worker_brief(c: &factorium_domain::checkout::Checkout, i: &factorium_domain::issue::Issue, branch: &str) -> String {
    let mut b = format!("# {}#{}: {}\n\n{}\n", c.name, i.number, i.title, i.body);
    if !i.comments.is_empty() {
        b.push_str("\n## Comments\n\n");
        for comment in &i.comments {
            b.push_str(&format!("- {comment}\n"));
        }
    }
    b.push_str(&format!(
        "\n## How you finish\n\nYou work in this worktree on the branch `{branch}`, made from `{}`'s proving branch \
         `{}`. Commit your work on it. Run `factorium worker gate` to run the repo's fast gate, as often as you \
         like. When the work is committed and the gate is green, run `factorium worker done`: factorium merges your \
         branch onto the proving branch and runs the gate there. If you cannot go on without a decision or \
         something you lack, run `factorium worker escalate \"what you need\"` and stop.\n",
        c.name, c.proving
    ));
    b
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

fn shell_word(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// A started worker's view, for answers.
pub(crate) fn _shown(st: &Started) -> Value {
    json!(st)
}
