//! Starting sessions: a worker on an issue, and the watch. A worker gets a
//! worktree branched from the proving tip, the first eligible harness of the
//! kit on its account with the most left, the guard, and its brief.

use std::path::{Path, PathBuf};

use factorium_domain::issue::{Take, TakeIssue};
use factorium_domain::kit::{Kits, kit_id};
use factorium_domain::watch::{self, StartWatch, Watches, watch_id};
use factorium_domain::worker::{self as w, Started};
use factorium_domain::worker_id;
use factorium_harness::account::{self, Session, Source};
use factorium_harness::{BriefAtStart, Harness, Rules, Start, Told};
use factorium_host::{Git, Host};
use factum::Value;
use serde_json::json;

use crate::{Answer, App, Refusal, fail};

/// The rules every worker is given, beside the repo's own.
const WORKER_RULES: &str = include_str!("worker_rules.md");
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
        let Ok(found) = account::read(&Tokens(self.host.as_ref())) else { return Vec::new() };
        let here = account::this_machine().unwrap_or_default();
        let home = self.host.home();
        found
            .accounts
            .into_iter()
            .filter_map(|a| match a.session(&here, &home) {
                Session::Ready(env) => {
                    let vars = env.vars.iter().map(|(n, v)| (*n, v.expose().to_string())).collect();
                    Some((a.name.clone(), a.harness.key().to_string(), vars))
                }
                _ => None,
            })
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
        std::fs::create_dir_all(dir.join(factorium_harness::start::SKILLS))
            .map_err(|e| Refusal::new("io", e.to_string()))?;
        std::fs::write(dir.join(factorium_harness::start::RULES), rules)
            .map_err(|e| Refusal::new("io", e.to_string()))?;
        Ok(dir)
    }

    /// Opens a terminal running a session as `l` says, and hands it its brief.
    fn launch(&self, l: Launch) -> Answer<String> {
        let kept = self.dir(&["kept", l.harness.key()]);
        std::fs::create_dir_all(&kept).map_err(|e| Refusal::new("io", e.to_string()))?;
        // The host user's files the harness needs (its login), linked in.
        for link in l.harness.kept().links {
            let (from, to) = (self.host.home().join(link.home), kept.join(link.kept));
            if from.exists() && !to.exists() {
                let _ = std::fs::create_dir_all(to.parent().unwrap_or(&kept));
                let _ = std::os::unix::fs::symlink(&from, &to);
            }
        }
        let (dir, worktree, kept_s) = (l.dir.to_string_lossy(), l.worktree.to_string_lossy(), kept.to_string_lossy());
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or("factorium".into());
        let hook = format!("{} guard {}", shell_word(&exe), l.harness.key());
        let read = |p: &Path| std::fs::read_to_string(p).ok();
        let (repo_rules, own_rules) = (read(&l.worktree.join("CLAUDE.md")), read(&l.dir.join("CLAUDE.md")));
        let (repo_file, own_file) = (l.worktree.join("CLAUDE.md"), l.dir.join("CLAUDE.md"));
        let (repo_file, own_file) = (repo_file.to_string_lossy(), own_file.to_string_lossy());
        let told = Told {
            repo: repo_rules.as_deref().map(|text| Rules { file: &repo_file, text }),
            added: None,
            own: own_rules.as_deref().map(|text| Rules { file: &own_file, text }),
        };
        // The session runs factorium by name: this one, on this database.
        let mut env = l.env;
        env.push(("FACTORIUM_DB", self.db.to_string_lossy().into_owned()));
        let path = std::env::var("PATH").unwrap_or_default();
        if let Some(bin) = Path::new(&exe).parent() {
            env.push(("PATH", format!("{}:{path}", bin.display())));
        }
        let start = Start {
            dir: &dir,
            worktree: &worktree,
            added: &dir,
            model: l.model,
            hook: &hook,
            kept: &kept_s,
            host_user: false,
            kept_config: false,
            host_rules: &[],
            dsh_profile: Default::default(),
            shadowed: &[],
            told,
            env,
        };
        let line = l.harness.start_line(l.command, &start);
        let brief = l.brief.to_string_lossy();
        let (line, typed) = match l.harness.brief_at_start(&line, &brief) {
            BriefAtStart::Started(line) => (line, None),
            BriefAtStart::Typed(typed) => (line, Some(typed)),
        };
        let (line, secret) = self.secrets_first(&l.secrets, line)?;
        let opened = self.host.orca().open(l.worktree, l.title, &line);
        if let (Err(_), Some(file)) = (&opened, &secret) {
            let _ = std::fs::remove_file(file);
        }
        let terminal = opened.map_err(fail("orca"))?;
        if let Some(typed) = typed {
            self.host.orca().send(&terminal, &typed).map_err(fail("orca"))?;
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
        if let Some(t) = &before.terminal
            && self.host.orca().screen(t).is_some()
        {
            return Ok(Some(t.clone()));
        }
        let kit = self.state::<Kits>(kit_id()).await?;
        let accounts = self.accounts_here();
        let here: Vec<(String, String)> = accounts.iter().map(|(n, h, _)| (n.clone(), h.clone())).collect();
        let Ok(choice) = kit.choose("", self.now(), &here) else { return Ok(None) };
        let Some(harness) = Harness::read(&choice.entry.harness) else { return Ok(None) };
        let home = self.dir(&["watch"]);
        if !home.join(".git").exists() {
            std::fs::create_dir_all(&home).map_err(|e| Refusal::new("io", e.to_string()))?;
            Git::at(&home).run(&["init", "--quiet"]).map_err(fail("git"))?;
            Git::at(&home)
                .run(&["commit", "--quiet", "--allow-empty", "-m", "The watch's home"])
                .map_err(fail("git"))?;
        }
        let rules = WATCH_RULES.replace("{db}", &self.db.to_string_lossy());
        std::fs::write(home.join("CLAUDE.md"), &rules).map_err(|e| Refusal::new("io", e.to_string()))?;
        let brief = home.join("BRIEF.md");
        std::fs::write(&brief, &rules).map_err(|e| Refusal::new("io", e.to_string()))?;
        let mut secrets = Vec::new();
        let env = vec![("FACTORIUM_WATCH", "1".to_string())];
        if let Some(a) = &choice.account {
            secrets.extend(accounts.iter().find(|(n, ..)| n == a).map(|(.., v)| v.clone()).unwrap_or_default());
        }
        let launch = Launch {
            harness,
            command: &choice.entry.command,
            model: choice.entry.model.as_deref(),
            dir: &home,
            worktree: &home,
            env,
            secrets,
            brief: &brief,
            title: "factorium watch",
        };
        let terminal = self.launch(launch)?;
        self.submit::<StartWatch>(watch::Started { terminal: terminal.clone(), harness: harness.key().into() }).await?;
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
