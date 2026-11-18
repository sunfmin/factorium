//! The watch as the app runs it: the look, the rounds sent to the watch's
//! session, and the watch's own commands.

use factorium_domain::doctor;
use factorium_domain::issue::Author;
use factorium_domain::outbox::{Outcome, Sender};
use factorium_domain::settings::{Settings, Time};
use factorium_domain::tracker;
use factorium_domain::watch::{self, FileReport, Filed, Look, LookNow, StartWatch, Watches, watch_id};
use factorium_domain::worker::{self as w, Status, Workers};
use factum::{Uuid, Value};
use serde_json::json;

use crate::life::End;
use crate::session::{Opened, Role};
use crate::{Answer, App, Refusal};

impl App {
    /// The look, if one is due: doctor's findings become episodes, and a new
    /// round is sent to the watch's session. It is due every `look_every`,
    /// the heartbeat for what nothing announces, and at once when a worker
    /// has started waiting for the watch since the last look: however many
    /// started, one look.
    pub(crate) async fn look_if_due(&self, settings: &Settings) -> Answer<Option<Value>> {
        let before = self.state::<Watches>(watch_id()).await?;
        if self.now() - before.last_look < settings.ms(Time::LookEvery) && !self.waits_unseen(before.last_look).await? {
            return Ok(None);
        }
        self.look(settings).await.map(Some)
    }

    /// Starts the watch's session if the kit has a harness and none runs:
    /// answers its terminal, or `None` when it cannot run.
    pub(crate) async fn ensure_watch(&self, before: &watch::Watch) -> Answer<Option<String>> {
        if let Some(s) = self.watch_session(before) {
            match self.gone(&s) {
                Ok(false) => return Ok(Some(s.terminal().into())),
                // Not known to be gone: no second session; none to type at now.
                Err(_) => return Ok(None),
                Ok(true) => {}
            }
        }
        let pick = match self.pick("").await {
            Ok(pick) => pick,
            Err(r) if r.code == "no_harness" => return Ok(None),
            Err(r) => return Err(r),
        };
        let Opened { terminal, typed, .. } = self.open_session(Role::Watch, &pick)?;
        self.submit::<StartWatch>(watch::Started {
            terminal: terminal.clone(),
            harness: pick.harness.key().into(),
            account: pick.account.clone(),
        })
        .await?;
        let started = self.state::<Watches>(watch_id()).await?;
        if let (Some(s), Some(typed)) = (self.watch_session(&started), typed) {
            self.hand_brief(&s, &typed).await?;
        }
        Ok(Some(terminal))
    }

    /// Whether a worker started waiting for the watch after `last_look`, or
    /// its issue was commented on since.
    async fn waits_unseen(&self, last_look: i64) -> Answer<bool> {
        for w in self.rows(tracker::workers(Some(Status::Waiting), false)).await? {
            let heard = self.state::<Workers>(w.worker).await?.heard;
            if w.state.since > last_look || heard.iter().any(|(at, _)| *at > last_look) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Looks now: findings to episodes, a round if none runs.
    pub async fn look(&self, settings: &Settings) -> Answer {
        let before = self.state::<Watches>(watch_id()).await?;
        let found = doctor::findings(&self.seen().await?, settings, self.now());
        let terminal = self.ensure_watch(&before).await?;
        let started = self.state::<Watches>(watch_id()).await?;
        let session = match (&terminal, self.watch_session(&started)) {
            (Some(_), Some(s)) => {
                self.retry(&s).await?;
                self.watch_ready(&started).await?
            }
            _ => None,
        };
        let ready = session.is_some();
        self.submit::<LookNow>(Look { found: found.clone(), bound: settings.ms(Time::RoundBound), ready }).await?;
        let after = self.state::<Watches>(watch_id()).await?;
        // The running round, passed once to each watch session, when it can take it.
        if let (Some(r), Some(s)) = (&after.round, &session)
            && !self.outbox(s).await?.arrived(&Sender::Round(r.n))
            && let Some(e) = after.episodes.get(&r.episode)
        {
            let heard = match e.heard.as_slice() {
                [] => String::new(),
                said => format!(" — Comments on its issue since: {}", said.join(" / ")),
            };
            let line = format!(
                "{}{heard} — How to fix: {} — Act with factorium's commands (see `factorium help`). \
                 When done, file `factorium watch report --text \"what you found and did\"` (add --human \"...\" \
                 only for what needs the owner: spending money). `factorium watch next` shows this round again.",
                e.what, e.fix
            );
            // Missed, it is passed again on the next look.
            self.tell(s, Sender::Round(r.n), &line, false).await?;
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

    /// `factorium watch restart`: sends a waiting worker back to work, told
    /// `text`; a worker whose session is gone ends, keeping its worktree, and
    /// its issue is started again. `now` as for `watch_tell`.
    pub async fn watch_restart(&self, worker: Uuid, text: &str, now: bool) -> Answer {
        let wk = self.state::<Workers>(worker).await?;
        let st = wk.started.ok_or_else(|| Refusal::new("no_worker", format!("no worker {worker}")))?;
        if wk.status == Status::Ended {
            return Err(Refusal::new("ended", "this worker has ended"));
        }
        // Kept on its issue as it is typed: this line is its delivery.
        self.note(&st.checkout, st.number, Author::Watch, text).await?;
        let session = Self::worker_session(worker, &st)?;
        match self.gone(&session) {
            Ok(false) => {}
            Err(why) => return Err(Refusal::new("orca", why)),
            Ok(true) => {
                self.end_worker(worker, End::because("restarted by the watch: its session was gone", true)).await?;
                return Ok(
                    json!({ "worker": worker, "ended": true, "issue": format!("{}#{}", st.checkout, st.number) }),
                );
            }
        }
        let line = format!("{text} Then go on, and run `factorium worker done` when the work is committed.");
        // A brief never handed goes first: the word is about its work.
        let delivery = self.tell(&session, Sender::Watch, &line, now).await.and_then(crate::session::heard)?;
        if let Outcome::Missed { why } = &delivery {
            return Ok(json!({ "worker": worker, "resumed": false, "delivery": delivery.word(), "missed": why }));
        }
        self.submit::<w::Resume>(w::Of { worker }).await?;
        Ok(json!({ "worker": worker, "resumed": true, "delivery": delivery.word() }))
    }

    /// `factorium watch tell`: types a line at a worker's session; answers
    /// whether it started a turn (`submitted`) or waits behind the running
    /// one (`queued`). `now`: first stops the running turn.
    pub async fn watch_tell(&self, worker: Uuid, text: &str, now: bool) -> Answer {
        let st =
            self.state::<Workers>(worker).await?.started.ok_or_else(|| Refusal::new("no_worker", "no such worker"))?;
        // Kept on its issue as it is typed: this line is its delivery.
        self.note(&st.checkout, st.number, Author::Watch, text).await?;
        let delivery = self
            .tell(&Self::worker_session(worker, &st)?, Sender::Watch, text, now)
            .await
            .and_then(crate::session::heard)?;
        Ok(json!({ "worker": worker, "told": text, "delivery": delivery.word(), "missed": delivery.why() }))
    }

    /// A comment on a worker's issue, passed to the worker exactly once.
    /// Waiting for the watch, it is not typed at: its watch episode carries
    /// the comment. Else it goes through its session's outbox: into its
    /// brief while that is still to be typed, or typed behind any running
    /// turn. Answers how (`submitted`, `queued`, `brief`, `missed`, `watch`,
    /// or `none`), and why it did not reach the worker; the issue
    /// keeps the comment for the next brief either way.
    pub(crate) async fn pass_comment(&self, worker: Uuid, text: &str) -> Answer<(&'static str, Option<String>)> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.as_ref().filter(|_| wk.status != Status::Ended) else { return Ok(("none", None)) };
        if wk.status == Status::Waiting {
            self.submit::<w::Hear>(w::Heard { worker, text: text.into() }).await?;
            return Ok(("watch", None));
        }
        let d = self.tell(&Self::worker_session(worker, st)?, Sender::Comment, text, false).await?;
        Ok((d.word(), d.why().map(str::to_string)))
    }
}
