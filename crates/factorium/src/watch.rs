//! The watch as the app runs it: the look, the rounds sent to the watch's
//! session, and the watch's own commands.

use factorium_domain::doctor;
use factorium_domain::settings::Settings;
use factorium_domain::watch::{self, FileReport, Filed, Look, LookNow, Watches, watch_id};
use factorium_domain::worker::{self as w, Status, Workers};
use factorium_harness::Harness;
use factorium_harness::turn::Turn;
use factum::{Uuid, Value};
use serde_json::json;

use crate::line::one_line;
use crate::session::turn_of;
use crate::{Answer, App, Refusal};

impl App {
    /// The look, if one is due: doctor's findings become episodes, and a new
    /// round is sent to the watch's session.
    pub(crate) async fn look_if_due(&self, settings: &Settings) -> Answer<Option<Value>> {
        let before = self.state::<Watches>(watch_id()).await?;
        if before.last_look > 0 && self.now() - before.last_look < settings.ms("look_every") {
            return Ok(None);
        }
        self.look(settings).await.map(Some)
    }

    /// `factorium watch look`: looks now.
    pub async fn look(&self, settings: &Settings) -> Answer {
        let before = self.state::<Watches>(watch_id()).await?;
        let found = doctor::findings(&self.seen().await?, settings, self.now());
        let terminal = self.ensure_watch(&before).await?;
        let ready = match (&terminal, self.state::<Watches>(watch_id()).await?.harness) {
            (Some(_), Some(h)) => Harness::read(&h).is_some_and(|h| {
                let home = self.dir(&["watch"]);
                let brief = home.join("BRIEF.md");
                let turn = turn_of(h, &self.session_root(h), &home.to_string_lossy(), &brief.to_string_lossy());
                matches!(turn, Some(Turn::Closed { .. }))
            }),
            _ => false,
        };
        self.submit::<LookNow>(Look { found: found.clone(), bound: settings.ms("round_bound"), ready }).await?;
        let after = self.state::<Watches>(watch_id()).await?;
        let fresh = after.round.as_ref().filter(|r| before.round.as_ref().is_none_or(|b| b.n != r.n));
        if let (Some(r), Some(t)) = (fresh, &terminal) {
            let e = &after.episodes[&r.episode];
            let line = format!(
                "factorium round {}: {} — How to fix: {} — Act with factorium's commands (see `factorium help`). \
                 When done, file `factorium watch report --text \"what you found and did\"` (add --human \"...\" \
                 only for what needs the owner: spending money). `factorium watch next` shows this round again.",
                r.n, e.what, e.fix
            );
            self.host.orca().send(t, &one_line(&line)).map_err(crate::fail("orca"))?;
        }
        Ok(
            json!({ "findings": found.len(), "episodes": after.episodes.len(), "round": after.round, "watch": terminal }),
        )
    }

    /// `factorium watch next`: the round running now and every open episode.
    pub async fn watch_next(&self) -> Answer {
        let s = self.state::<Watches>(watch_id()).await?;
        let episode = s.round.as_ref().and_then(|r| s.episodes.get(&r.episode));
        Ok(json!({ "round": s.round, "episode": episode, "episodes": s.episodes, "held": s.held }))
    }

    /// `factorium watch report`: ends the round with what the watch found
    /// and did; `human` is what only the owner can do.
    pub async fn watch_report(&self, text: &str, human: Option<&str>) -> Answer {
        self.submit::<FileReport>(Filed { text: text.into(), human: human.map(Into::into) }).await?;
        let s = self.state::<Watches>(watch_id()).await?;
        Ok(json!({ "reported": s.reports.last() }))
    }

    /// The watch's reports, newest last.
    pub async fn watch_reports(&self) -> Answer {
        let s = self.state::<Watches>(watch_id()).await?;
        Ok(json!(s.reports))
    }

    /// `factorium watch restart`: sends a waiting worker back to work, told
    /// `text`; a worker whose session is gone ends, keeping its worktree, and
    /// its issue is started again.
    pub async fn watch_restart(&self, worker: Uuid, text: &str) -> Answer {
        let wk = self.state::<Workers>(worker).await?;
        let st = wk.started.ok_or_else(|| Refusal::new("no_worker", format!("no worker {worker}")))?;
        if wk.status == Status::Ended {
            return Err(Refusal::new("ended", "this worker has ended"));
        }
        if self.host.orca().screen(&st.terminal).is_none() {
            self.end_worker(worker, "restarted by the watch: its session was gone", false).await?;
            return Ok(json!({ "worker": worker, "ended": true, "issue": format!("{}#{}", st.checkout, st.number) }));
        }
        self.submit::<w::Resume>(w::Of { worker }).await?;
        let line = format!(
            "factorium (the watch): {text} Then go on, and run `factorium worker done` when the work is committed."
        );
        self.host.orca().send(&st.terminal, &one_line(&line)).map_err(crate::fail("orca"))?;
        Ok(json!({ "worker": worker, "resumed": true }))
    }

    /// `factorium watch tell`: types a line at a worker's session.
    pub async fn watch_tell(&self, worker: Uuid, text: &str) -> Answer {
        let st =
            self.state::<Workers>(worker).await?.started.ok_or_else(|| Refusal::new("no_worker", "no such worker"))?;
        self.host
            .orca()
            .send(&st.terminal, &one_line(&format!("factorium (the watch): {text}")))
            .map_err(crate::fail("orca"))?;
        Ok(json!({ "worker": worker, "told": text }))
    }

    /// The reports' owner part, for `doctor`.
    pub async fn human(&self) -> Answer<Vec<watch::Report>> {
        let s = self.state::<Watches>(watch_id()).await?;
        Ok(s.reports.into_iter().filter(|r| r.human.is_some()).collect())
    }
}

impl App {
    /// `factorium watch look`.
    pub async fn look_now(&self) -> Answer {
        let settings = self.settings().await?;
        self.look(&settings).await
    }
}
