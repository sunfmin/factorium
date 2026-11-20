//! `factorium worker pick-mutants`: a worker's mutants picked by a second
//! agent, a fresh headless session factorium starts on another harness of
//! the kit, given slimfit's goal, the change's diff and slimfit's candidates
//! and nothing of the worker's. One that fails hands the pick to the next,
//! the worker's own harness last; the record and its cost go on the issue.

use factorium_domain::issue::Author;
use factorium_domain::pick::{self, Picked};
use factorium_domain::settings::Time;
use factorium_harness::{Ask, Asked, Harness};
use factorium_host::Priority;
use factum::Uuid;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::session::Pick;
use crate::{Answer, App, Refusal, Want, fail};

impl App {
    /// Has the worker's change's mutants picked, and answers the picks, or
    /// why none, by whom, with the commands that run and record them; when
    /// every picker fails, the last one's refusal, and nothing is kept.
    pub async fn worker_pick_mutants(&self, worker: Uuid) -> Answer {
        let st = self.live(worker, Want::Any).await?.started;
        let proving = self.checkout(&st.checkout).await?.fetched_proving();
        let git = self.host.git(&st.worktree);
        let from = git.run(&["merge-base", "HEAD", &proving]).map_err(fail("git"))?;
        let diff = git.run(&["diff", from.trim()]).map_err(fail("git"))?;
        let slimfit = |args: &[&str]| self.host.programs().run("slimfit", args).result("slimfit", args);
        let goal = slimfit(&["help", "picker"]).map_err(fail("slimfit"))?;
        let listing = slimfit(&["check", "--repo", &st.worktree, "--mutants"]).map_err(fail("slimfit"))?;
        let listed = serde_json::from_str(&listing).map_err(|e| Refusal::new("slimfit", format!("{e}: {listing}")))?;
        let question = pick::question(&goal, &diff, &listing);

        let (mut failed, mut last): (Vec<(String, String)>, Option<Refusal>) = (Vec::new(), None);
        let (on, (picked, asked, took_ms)) = loop {
            let asks =
                |key: &str| !failed.iter().any(|(h, _)| h == key) && Harness::read(key).is_some_and(Harness::asks);
            let on = match self.pick_by(|kit, now, here| kit.choose_picker(&st.harness, asks, now, here)).await {
                Ok(on) => on,
                Err(none) => return Err(last.unwrap_or(none)),
            };
            let dir = self.data().pick(uuid::Uuid::now_v7());
            let answered = self.ask_in(&dir, &on, &question, &listed).await;
            let _ = std::fs::remove_dir_all(&dir);
            let why = match answered {
                Ok(answered) => break (on, answered),
                Err(why) => why,
            };
            failed.push((on.harness.key().into(), why.message.clone()));
            last = Some(why);
        };
        let author = format!("{}:{worker}:{}", st.harness, st.model.as_deref().unwrap_or("default"));
        let (harness, account, fell_back) = (on.harness.key(), on.account.as_deref(), &failed);
        let (session, model, tokens_in, tokens_out) = (&asked.session, &asked.model, asked.tokens_in, asked.tokens_out);
        let taken = pick::Taken { harness, session, model, account, author, took_ms, tokens_in, tokens_out, fell_back };
        let (answer, cost) = pick::answer(&picked, &taken, factorium_harness::quoted);
        self.note(&st.checkout, st.number, Author::Factorium, &cost).await?;
        Ok(answer)
    }

    /// What a fresh session on `on`, alone in `dir`, answered `question`,
    /// taken against slimfit's `listed`, and the ms it took; refused when it
    /// fails, passes `pick_bound` or answers off the form.
    async fn ask_in(&self, dir: &Path, on: &Pick, question: &str, listed: &Value) -> Answer<(Picked, Asked, u64)> {
        let bound = Duration::from_millis(self.settings().await?.ms(Time::PickBound) as u64);
        let (q, a, user_dir) = (dir.join("question.md"), dir.join("answer.json"), dir.join("home"));
        let made = std::fs::create_dir_all(&user_dir).and_then(|()| std::fs::write(&q, question));
        made.map_err(|e| Refusal::new("io", e.to_string()))?;
        let home = self.host.home();
        on.harness.ask_home(&home, on.account.as_deref(), &user_dir).map_err(fail("picker_failed"))?;
        let secret = self.write_secrets(&on.secrets)?;
        let path = crate::session::program_dir(&on.command, &home).into_iter();
        let path: Vec<String> = path.chain([std::env::var("PATH").unwrap_or_default()]).collect();
        let (q, a, s) = (q.to_string_lossy(), a.to_string_lossy(), secret.map(|f| f.display().to_string()));
        let (command, model, path, secrets) = (&on.command, on.model.as_deref(), &path.join(":"), s.as_deref());
        let ask = Ask { command, model, home: &home, path, user_dir: &user_dir, question: &q, answer: &a, secrets };
        let (at, line) = (dir.to_path_buf(), on.harness.ask_line(&ask).expect("one that asks"));
        let run = move || factorium_host::run(&at, &line, &[], bound, Priority::Normal, &|_| {}, &|| false);
        let ran = tokio::task::spawn_blocking(run).await.map_err(|e| Refusal::new("picker_failed", e.to_string()));
        s.map(std::fs::remove_file);
        let (ran, said) = (ran?, std::fs::read_to_string(dir.join("answer.json")).unwrap_or_default());
        if !ran.ok {
            let (code, how) = match ran.timed_out {
                true => ("picker_timed_out", format!("ran past pick_bound ({}s) and was ended", bound.as_secs())),
                false => ("picker_failed", format!("exited {:?}", ran.code)),
            };
            return Err(Refusal::new(code, format!("the {} picker {how}: {}\n{said}", on.harness.key(), ran.tail)));
        }
        let asked = on.harness.asked(&said, on.model.as_deref()).map_err(fail("picker_failed"))?;
        Ok((pick::read(&asked.text, listed).map_err(fail("picker_answer"))?, asked, ran.took_ms))
    }
}
