//! `factorium worker pick-mutants`: a worker's mutants picked by a second
//! agent factorium starts, a fresh headless session that is not the
//! worker's, on another harness of the kit where one can be asked. It is
//! given slimfit's goal, the change's diff and slimfit's candidates and
//! nothing of the worker's; its cost is kept on the issue.

use factorium_domain::issue::Author;
use factorium_domain::pick;
use factorium_domain::settings::Time;
use factorium_harness::{Ask, Harness};
use factorium_host::Priority;
use factum::Uuid;
use std::time::Duration;

use crate::{Answer, App, Refusal, fail};

impl App {
    /// Has the worker's change's mutants picked, and answers the picks, or
    /// why none, by whom, with the commands that run and record them. A
    /// picker that fails, passes `pick_bound` or answers off the form is
    /// refused, and nothing is kept of it.
    pub async fn worker_pick_mutants(&self, worker: Uuid) -> Answer {
        let (_, st) = self.started(worker).await?;
        let proving = self.checkout(&st.checkout).await?.fetched_proving();
        let git = self.host.git(&st.worktree);
        let from = git.run(&["merge-base", "HEAD", &proving]).map_err(fail("git"))?;
        let diff = git.run(&["diff", from.trim()]).map_err(fail("git"))?;
        let slimfit = |args: &[&str]| self.host.programs().run("slimfit", args).result("slimfit", args);
        let goal = slimfit(&["help", "picker"]).map_err(fail("slimfit"))?;
        let listing = slimfit(&["check", "--repo", &st.worktree, "--mutants"]).map_err(fail("slimfit"))?;
        let listed = serde_json::from_str(&listing).map_err(|e| Refusal::new("slimfit", format!("{e}: {listing}")))?;

        let asks = |key: &str| Harness::read(key).is_some_and(Harness::asks);
        let on = self.pick_by(|kit, now, here| kit.choose_picker(&st.harness, asks, now, here)).await?;

        let dir = self.data().pick(uuid::Uuid::now_v7());
        let (question, answer) = (dir.join("question.md"), dir.join("answer.json"));
        let asked = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&question, pick::question(&goal, &diff, &listing)));
        asked.map_err(|e| Refusal::new("io", e.to_string()))?;
        let secret = self.write_secrets(&on.secrets)?;
        let path = crate::session::program_dir(&on.command, &self.host.home()).into_iter();
        let path: Vec<String> = path.chain([std::env::var("PATH").unwrap_or_default()]).collect();
        let (q, a, s) = (question.to_string_lossy(), answer.to_string_lossy(), secret.map(|f| f.display().to_string()));
        let home = self.host.home();
        let ask = Ask {
            command: &on.command,
            model: on.model.as_deref(),
            home: &home,
            path: &path.join(":"),
            question: &q,
            answer: &a,
            secrets: s.as_deref(),
        };
        let line = on.harness.ask_line(&ask).expect("one that asks");
        let bound = Duration::from_millis(self.settings().await?.ms(Time::PickBound) as u64);
        let at = dir.clone();
        let ran = tokio::task::spawn_blocking(move || {
            factorium_host::run(&at, &line, &[], bound, Priority::Normal, &|_| {}, &|| false)
        });
        let ran = ran.await.map_err(|e| Refusal::new("picker_failed", e.to_string()))?;
        let said = std::fs::read_to_string(&answer).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        s.map(std::fs::remove_file);
        if !ran.ok {
            let (code, how) = match ran.timed_out {
                true => ("picker_timed_out", format!("ran past pick_bound ({}s) and was ended", bound.as_secs())),
                false => ("picker_failed", format!("exited {:?}", ran.code)),
            };
            return Err(Refusal::new(code, format!("the {} picker {how}: {}\n{said}", on.harness.key(), ran.tail)));
        }
        let asked = on.harness.asked(&said).map_err(fail("picker_failed"))?;
        let picked = pick::read(&asked.text, &listed).map_err(fail("picker_answer"))?;
        let taken = pick::Taken {
            harness: on.harness.key(),
            session: &asked.session,
            model: &asked.model,
            account: on.account.as_deref(),
            author: format!("{}:{worker}:{}", st.harness, st.model.as_deref().unwrap_or("default")),
            took_ms: ran.took_ms,
            tokens_in: asked.tokens_in,
            tokens_out: asked.tokens_out,
        };
        let (answered, cost) = pick::answer(&picked, &taken, factorium_harness::quoted);
        self.note(&st.checkout, st.number, Author::Factorium, &cost).await?;
        Ok(answered)
    }
}
