//! A worker's second agent: a fresh headless session factorium starts on
//! another harness of the kit, given slimfit's goal and the change's diff
//! and nothing of the worker's. `factorium worker pick-mutants` asks it
//! to pick the change's mutants from slimfit's candidates; `factorium
//! worker review-growth` to judge whether the change could be smaller. One
//! that fails hands the question to the next, the worker's own harness
//! last; what it took goes on the issue.

use factorium_domain::issue::Author;
use factorium_domain::pick::{self, Taken};
use factorium_domain::review;
use factorium_domain::settings::Time;
use factorium_domain::worker::Started;
use factorium_harness::{Ask, Asked, Harness};
use factorium_host::Priority;
use factum::Uuid;
use std::path::Path;
use std::time::Duration;

use crate::session::{Pick, SessionEnv};
use crate::{Answer, App, Refusal, Want, fail};

/// What a second agent answered, taken, and who it was: the pick it ran
/// on, its session, the ms it took, and those that failed before it.
struct Second<T> {
    got: T,
    on: Pick,
    asked: Asked,
    took_ms: u64,
    failed: Vec<(String, String)>,
    author: String,
}

impl<T> Second<T> {
    fn taken(&self) -> Taken<'_> {
        let (on, asked) = (&self.on, &self.asked);
        Taken {
            harness: on.harness.key(),
            session: &asked.session,
            model: &asked.model,
            account: on.account.as_deref(),
            author: self.author.clone(),
            took_ms: self.took_ms,
            tokens_in: asked.tokens_in,
            tokens_out: asked.tokens_out,
            fell_back: &self.failed,
        }
    }
}

impl App {
    /// Has the worker's change's mutants picked, and answers the picks, or
    /// why none, by whom, with the commands that run and record them; when
    /// every picker fails, the last one's refusal, and nothing is kept.
    pub async fn worker_pick_mutants(&self, worker: Uuid) -> Answer {
        let (st, diff, author) = self.change(worker).await?;
        let goal = self.slimfit(&["help", "picker"])?;
        let listing = self.slimfit(&["check", "--repo", &st.worktree, "--mutants"])?;
        let listed = serde_json::from_str(&listing).map_err(|e| Refusal::new("slimfit", format!("{e}: {listing}")))?;
        let question = pick::question(&goal, &diff, &listing);
        let second = self.second(&st, author, "picker", &question, |text| pick::read(text, &listed)).await?;
        let (answer, cost) = pick::answer(&second.got, &second.taken(), factorium_harness::quoted);
        self.note(&st.checkout, st.number, Author::Factorium, &cost).await?;
        Ok(answer)
    }

    /// Has the worker's change's growth judged, and answers the verdict, by
    /// whom, with the command that writes its growth record; when every
    /// reviewer fails, the last one's refusal, and nothing is kept.
    pub async fn worker_review_growth(&self, worker: Uuid) -> Answer {
        let (st, diff, author) = self.change(worker).await?;
        let question = review::question(&self.slimfit(&["help", "reviewer"])?, &diff);
        let second = self.second(&st, author, "reviewer", &question, review::read).await?;
        let serves = format!("{}#{}", st.checkout, st.number);
        let (answer, cost) = review::answer(&second.got, &serves, &second.taken(), factorium_harness::quoted);
        self.note(&st.checkout, st.number, Author::Factorium, &cost).await?;
        Ok(answer)
    }

    /// The live worker, its change's diff from where it leaves origin's
    /// proving, and its id, as `--author` takes it.
    async fn change(&self, worker: Uuid) -> Answer<(Started, String, String)> {
        let st = self.live(worker, Want::Any).await?.started;
        let proving = self.checkout(&st.checkout).await?.fetched_proving();
        let git = self.host.git(&st.worktree);
        let from = git.run(&["merge-base", "HEAD", &proving]).map_err(fail("git"))?;
        let diff = git.run(&["diff", from.trim()]).map_err(fail("git"))?;
        let author = format!("{}:{worker}:{}", st.harness, st.model.as_deref().unwrap_or("default"));
        Ok((st, diff, author))
    }

    fn slimfit(&self, args: &[&str]) -> Answer<String> {
        self.host.programs().run("slimfit", args).result("slimfit", args).map_err(fail("slimfit"))
    }

    /// `question` asked of a fresh session as `role`, its answer taken by
    /// `read`: on the kit's other harness that can be asked, each that
    /// fails handing it to the next, the worker's own last.
    async fn second<T>(
        &self,
        st: &Started,
        author: String,
        role: &str,
        question: &str,
        read: impl Fn(&str) -> Result<T, String>,
    ) -> Answer<Second<T>> {
        let (mut failed, mut last): (Vec<(String, String)>, Option<Refusal>) = (Vec::new(), None);
        loop {
            let asks =
                |key: &str| !failed.iter().any(|(h, _)| h == key) && Harness::read(key).is_some_and(Harness::asks);
            let on = match self.pick_by(|kit, now, here| kit.choose_picker(&st.harness, asks, now, here)).await {
                Ok(on) => on,
                Err(none) => return Err(last.unwrap_or(none)),
            };
            let dir = self.data().pick(uuid::Uuid::now_v7());
            let answered = self.ask_in(&dir, &on, role, question).await;
            let _ = std::fs::remove_dir_all(&dir);
            let why = match answered.and_then(|(asked, took_ms)| {
                let got = read(&asked.text).map_err(fail(&format!("{role}_answer")))?;
                Ok((got, asked, took_ms))
            }) {
                Ok((got, asked, took_ms)) => {
                    return Ok(Second { got, on, asked, took_ms, failed, author });
                }
                Err(why) => why,
            };
            failed.push((on.harness.key().into(), why.message.clone()));
            last = Some(why);
        }
    }

    /// What a fresh session on `on`, alone in `dir`, answered `question`
    /// as `role`, and the ms it took; refused when it fails or passes
    /// `pick_bound`.
    async fn ask_in(&self, dir: &Path, on: &Pick, role: &str, question: &str) -> Answer<(Asked, u64)> {
        let bound = Duration::from_millis(self.settings().await?.ms(Time::PickBound) as u64);
        let failed = format!("{role}_failed");
        let (q, a, user_dir) = (dir.join("question.md"), dir.join("answer.json"), dir.join("home"));
        let made = std::fs::create_dir_all(&user_dir).and_then(|()| std::fs::write(&q, question));
        made.map_err(|e| Refusal::new("io", e.to_string()))?;
        let home = self.host.home();
        on.harness.ask_home(&home, on.account.as_deref(), &user_dir).map_err(fail(&failed))?;
        // Dropped once the ask is done, as it ends: no secrets file outlives it.
        let session = SessionEnv::new(self, on)?;
        let (q, a) = (q.to_string_lossy(), a.to_string_lossy());
        let (command, model, path, secrets) =
            (&on.command, on.model.as_deref(), session.path(), session.secrets_file());
        let ask = Ask { command, model, home: &home, path, user_dir: &user_dir, question: &q, answer: &a, secrets };
        let (at, line) = (dir.to_path_buf(), on.harness.ask_line(&ask).expect("one that asks"));
        let run = move || factorium_host::run(&at, &line, &[], bound, Priority::Normal, &|_| {}, &|| false);
        let ran = tokio::task::spawn_blocking(run).await.map_err(|e| Refusal::new(&failed, e.to_string()));
        drop(session);
        let (ran, said) = (ran?, std::fs::read_to_string(dir.join("answer.json")).unwrap_or_default());
        if !ran.ok {
            let (code, how) = match ran.timed_out {
                true => {
                    (format!("{role}_timed_out"), format!("ran past pick_bound ({}s) and was ended", bound.as_secs()))
                }
                false => (failed, format!("exited {:?}", ran.code)),
            };
            return Err(Refusal::new(&code, format!("the {} {role} {how}: {}\n{said}", on.harness.key(), ran.tail)));
        }
        let asked = on.harness.asked(&said, on.model.as_deref()).map_err(fail(&failed))?;
        Ok((asked, ran.took_ms))
    }
}
