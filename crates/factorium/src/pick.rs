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
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::session::{Pick, SessionEnv};
use crate::{Answer, App, Refusal, Want, fail};

impl App {
    /// Has the worker's change's mutants picked, and answers the picks, or
    /// why none, by whom, with the commands that run and record them; when
    /// every picker fails, the last one's refusal, and nothing is kept.
    /// When slimfit lists no candidates no picker is asked: none, and why.
    pub async fn worker_pick_mutants(&self, worker: Uuid) -> Answer {
        let (st, diff) = self.change(worker).await?;
        let args = ["check", "--repo", &st.worktree, "--mutants"];
        let said = self.host.programs().run("slimfit", &args);
        let listed = pick::listing(said.ok, &said.out)
            .map_err(|e| Refusal::new("slimfit", format!("slimfit {}: {e} {}", args.join(" "), said.err.trim())))?;
        let Some(listed) = listed else {
            let (answer, line) = pick::unlisted(&author(&st, worker));
            self.note(&st.checkout, st.number, Author::Factorium, &line).await?;
            return Ok(answer);
        };
        let (goal, listing) = (self.slimfit(&["help", "picker"])?, said.out);
        let question = pick::question(&goal, &diff, &listing);
        let quote = factorium_harness::quoted;
        self.second(&st, worker, "picker", &question, |text, t| Ok(pick::answer(&pick::read(text, &listed)?, t, quote)))
            .await
    }

    /// Has the worker's change's growth judged, and answers the verdict, by
    /// whom, with the command that writes its growth record; when every
    /// reviewer fails, the last one's refusal, and nothing is kept.
    pub async fn worker_review_growth(&self, worker: Uuid) -> Answer {
        let (st, diff) = self.change(worker).await?;
        let question = review::question(&self.slimfit(&["help", "reviewer"])?, &diff);
        let (serves, quote) = (format!("{}#{}", st.checkout, st.number), factorium_harness::quoted);
        self.second(&st, worker, "reviewer", &question, |text, t| {
            Ok(review::answer(&review::read(text)?, &serves, t, quote))
        })
        .await
    }

    /// The live worker and its change's diff, from where it leaves origin's proving.
    async fn change(&self, worker: Uuid) -> Answer<(Started, String)> {
        let st = self.live(worker, Want::Any).await?.started;
        let proving = self.checkout(&st.checkout).await?.fetched_proving();
        let git = self.host.git(&st.worktree);
        let from = git.run(&["merge-base", "HEAD", &proving]).map_err(fail("git"))?;
        let diff = git.run(&["diff", from.trim()]).map_err(fail("git"))?;
        Ok((st, diff))
    }

    fn slimfit(&self, args: &[&str]) -> Answer<String> {
        self.host.programs().run("slimfit", args).result("slimfit", args).map_err(fail("slimfit"))
    }

    /// `question` asked of a fresh session as `role`, on the kit's other
    /// harness that can be asked, each that fails handing it to the next,
    /// the worker's own last; its answer and the issue's line of what it
    /// took made by `take` of its text and who it was. Keeps the line and
    /// answers the answer.
    async fn second(
        &self,
        st: &Started,
        worker: Uuid,
        role: &str,
        question: &str,
        take: impl Fn(&str, &Taken) -> Result<(Value, String), String>,
    ) -> Answer {
        let author = author(st, worker);
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
            let taken = answered.and_then(|(asked, took_ms)| {
                let (harness, account, author) = (on.harness.key(), on.account.as_deref(), author.clone());
                let (session, model, tokens_in, tokens_out) =
                    (&asked.session, &asked.model, asked.tokens_in, asked.tokens_out);
                let t = Taken {
                    harness,
                    session,
                    model,
                    account,
                    author,
                    took_ms,
                    tokens_in,
                    tokens_out,
                    fell_back: &failed,
                };
                take(&asked.text, &t).map_err(fail(&format!("{role}_answer")))
            });
            let why = match taken {
                Ok((answer, cost)) => {
                    self.note(&st.checkout, st.number, Author::Factorium, &cost).await?;
                    return Ok(answer);
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

/// The worker's id, as `--author` takes it: `harness:worker:model`.
fn author(st: &Started, worker: Uuid) -> String {
    format!("{}:{worker}:{}", st.harness, st.model.as_deref().unwrap_or("default"))
}
