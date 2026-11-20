//! `factorium worker pick-mutants`: a worker's mutants picked by a second
//! agent factorium starts, a fresh headless session that is not the
//! worker's, on another harness of the kit where one can be asked. It is
//! given slimfit's goal, the change's diff and slimfit's candidates and
//! nothing of the worker's; its cost is kept on the issue. A picker that
//! fails hands the pick to the next harness the kit has, the worker's own
//! last, and the record says which failed and why.

use factorium_domain::issue::Author;
use factorium_domain::pick;
use factorium_domain::settings::Time;
use factorium_harness::{Ask, Asked, Harness};
use factorium_host::Priority;
use factum::Uuid;
use serde_json::Value;
use std::time::Duration;

use crate::session::Pick;
use crate::{Answer, App, Refusal, fail};

/// What one picker answered, as taken, and how long it took.
struct Answered {
    asked: Asked,
    picked: pick::Picked,
    took_ms: u64,
}

impl App {
    /// Has the worker's change's mutants picked, and answers the picks, or
    /// why none, by whom, with the commands that run and record them. A
    /// picker that fails, passes `pick_bound` or answers off the form hands
    /// the pick to the next harness of the kit that asks; when none is left
    /// the last one's refusal is answered, and nothing is kept of any.
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
        let question = pick::question(&goal, &diff, &listing);
        let bound = Duration::from_millis(self.settings().await?.ms(Time::PickBound) as u64);

        let mut failed: Vec<(String, Refusal)> = Vec::new();
        let (on, answered) = loop {
            let tried: Vec<&str> = failed.iter().map(|(h, _)| h.as_str()).collect();
            let asks = |key: &str| !tried.contains(&key) && Harness::read(key).is_some_and(Harness::asks);
            let on = match self.pick_by(|kit, now, here| kit.choose_picker(&st.harness, asks, now, here)).await {
                Ok(on) => on,
                Err(none) => return Err(failed.pop().map(|(_, last)| last).unwrap_or(none)),
            };
            match self.ask_picker(&on, &question, &listed, bound).await {
                Ok(answered) => break (on, answered),
                Err(why) => failed.push((on.harness.key().to_string(), why)),
            }
        };
        let fell_back: Vec<(&str, String)> =
            failed.iter().map(|(h, r)| (h.as_str(), r.message.lines().next().unwrap_or_default().to_string())).collect();
        let taken = pick::Taken {
            harness: on.harness.key(),
            session: &answered.asked.session,
            model: &answered.asked.model,
            account: on.account.as_deref(),
            author: format!("{}:{worker}:{}", st.harness, st.model.as_deref().unwrap_or("default")),
            took_ms: answered.took_ms,
            tokens_in: answered.asked.tokens_in,
            tokens_out: answered.asked.tokens_out,
            fell_back: &fell_back,
        };
        let (answer, cost) = pick::answer(&answered.picked, &taken, factorium_harness::quoted);
        self.note(&st.checkout, st.number, Author::Factorium, &cost).await?;
        Ok(answer)
    }

    /// Asks one fresh session on `on` `question`, alone in a directory of
    /// its own that is removed after, and takes its answer against slimfit's
    /// `listed`; refused when it fails, passes `bound` or answers off the form.
    async fn ask_picker(&self, on: &Pick, question: &str, listed: &Value, bound: Duration) -> Answer<Answered> {
        let dir = self.data().pick(uuid::Uuid::now_v7());
        let asked = self.ask_in(&dir, on, question, bound).await;
        let _ = std::fs::remove_dir_all(&dir);
        let (said, took_ms) = asked?;
        let asked = on.harness.asked(&said, on.model.as_deref()).map_err(fail("picker_failed"))?;
        let picked = pick::read(&asked.text, listed).map_err(fail("picker_answer"))?;
        Ok(Answered { asked, picked, took_ms })
    }

    /// What a session on `on` asked `question` in `dir` printed, and how
    /// long it took.
    async fn ask_in(&self, dir: &std::path::Path, on: &Pick, question: &str, bound: Duration) -> Answer<(String, u64)> {
        let io = |e: std::io::Error| Refusal::new("io", e.to_string());
        let (q, a, user_dir) = (dir.join("question.md"), dir.join("answer.json"), dir.join("home"));
        std::fs::create_dir_all(&user_dir).and_then(|()| std::fs::write(&q, question)).map_err(io)?;
        let home = self.host.home();
        on.harness.ask_home(&home, on.account.as_deref(), &user_dir).map_err(fail("picker_failed"))?;
        let secret = self.write_secrets(&on.secrets)?;
        let path = crate::session::program_dir(&on.command, &home).into_iter();
        let path: Vec<String> = path.chain([std::env::var("PATH").unwrap_or_default()]).collect();
        let (q, a, s) = (q.to_string_lossy(), a.to_string_lossy(), secret.map(|f| f.display().to_string()));
        let ask = Ask {
            command: &on.command,
            model: on.model.as_deref(),
            home: &home,
            path: &path.join(":"),
            user_dir: &user_dir,
            question: &q,
            answer: &a,
            secrets: s.as_deref(),
        };
        let line = on.harness.ask_line(&ask).expect("one that asks");
        let at = dir.to_path_buf();
        let ran = tokio::task::spawn_blocking(move || {
            factorium_host::run(&at, &line, &[], bound, Priority::Normal, &|_| {}, &|| false)
        });
        let ran = ran.await.map_err(|e| Refusal::new("picker_failed", e.to_string()));
        s.map(std::fs::remove_file);
        let ran = ran?;
        let said = std::fs::read_to_string(dir.join("answer.json")).unwrap_or_default();
        if !ran.ok {
            let (code, how) = match ran.timed_out {
                true => ("picker_timed_out", format!("ran past pick_bound ({}s) and was ended", bound.as_secs())),
                false => ("picker_failed", format!("exited {:?}", ran.code)),
            };
            return Err(Refusal::new(code, format!("the {} picker {how}: {}\n{said}", on.harness.key(), ran.tail)));
        }
        Ok((said, ran.took_ms))
    }
}
