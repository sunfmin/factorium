//! The watch as the app runs it: the look, the rounds sent to the watch's
//! session, and the watch's own commands.

use factorium_domain::doctor;
use factorium_domain::settings::{Settings, Time};
use factorium_domain::tracker;
use factorium_domain::watch::{self, FileReport, Filed, Look, LookNow, SendRound, Watches, watch_id};
use factorium_domain::worker::{self as w, Status, Workers};
use factum::{Uuid, Value};
use serde_json::json;

use crate::line::one_line;
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

    /// `factorium watch look`: looks now.
    pub async fn look(&self, settings: &Settings) -> Answer {
        let before = self.state::<Watches>(watch_id()).await?;
        let found = doctor::findings(&self.seen().await?, settings, self.now());
        let terminal = self.ensure_watch(&before).await?;
        if terminal.is_some() {
            self.hand_watch_brief(false).await?;
        }
        let started = self.state::<Watches>(watch_id()).await?;
        let session = terminal.as_ref().and_then(|_| self.watch_ready(&started));
        let ready = session.is_some();
        self.submit::<LookNow>(Look { found: found.clone(), bound: settings.ms(Time::RoundBound), ready }).await?;
        let mut after = self.state::<Watches>(watch_id()).await?;
        // The running round, typed once at each watch session, when it can take it.
        let unsent = after.round.as_ref().filter(|r| r.sent != terminal);
        if let (Some(r), Some(t), Some(s)) = (unsent, &terminal, &session)
            && let Some(e) = after.episodes.get(&r.episode)
        {
            let heard = match e.heard.as_slice() {
                [] => String::new(),
                said => format!(" — Comments on its issue since: {}", said.join(" / ")),
            };
            let line = format!(
                "factorium round {}: {}{heard} — How to fix: {} — Act with factorium's commands (see `factorium help`). \
                 When done, file `factorium watch report --text \"what you found and did\"` (add --human \"...\" \
                 only for what needs the owner: spending money). `factorium watch next` shows this round again.",
                r.n, e.what, e.fix
            );
            self.type_at(s, &one_line(&line))?;
            self.submit::<SendRound>(watch::Sent { n: r.n, terminal: t.clone() }).await?;
            after = self.state::<Watches>(watch_id()).await?;
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
    /// its issue is started again. `now` as for `watch_tell`.
    pub async fn watch_restart(&self, worker: Uuid, text: &str, now: bool) -> Answer {
        let wk = self.state::<Workers>(worker).await?;
        let st = wk.started.ok_or_else(|| Refusal::new("no_worker", format!("no worker {worker}")))?;
        if wk.status == Status::Ended {
            return Err(Refusal::new("ended", "this worker has ended"));
        }
        let session = Self::worker_session(&st)?;
        match self.gone(&session) {
            Ok(false) => {}
            Err(why) => return Err(Refusal::new("orca", why)),
            Ok(true) => {
                self.end_worker(worker, "restarted by the watch: its session was gone", false).await?;
                return Ok(
                    json!({ "worker": worker, "ended": true, "issue": format!("{}#{}", st.checkout, st.number) }),
                );
            }
        }
        let line = format!(
            "factorium (the watch): {text} Then go on, and run `factorium worker done` when the work is committed."
        );
        // A brief never handed goes first: the word is about its work.
        self.hand_worker_brief(worker, false).await?;
        let delivery = self.deliver_at(&session, &one_line(&line), now)?;
        self.submit::<w::Resume>(w::Of { worker }).await?;
        Ok(json!({ "worker": worker, "resumed": true, "delivery": delivery.word() }))
    }

    /// `factorium watch tell`: types a line at a worker's session; answers
    /// whether it started a turn (`submitted`) or waits behind the running
    /// one (`queued`). `now`: first stops the running turn.
    pub async fn watch_tell(&self, worker: Uuid, text: &str, now: bool) -> Answer {
        let st =
            self.state::<Workers>(worker).await?.started.ok_or_else(|| Refusal::new("no_worker", "no such worker"))?;
        let line = one_line(&format!("factorium (the watch): {text}"));
        let delivery = self.deliver_at(&Self::worker_session(&st)?, &line, now)?;
        Ok(json!({ "worker": worker, "told": text, "delivery": delivery.word() }))
    }

    /// A comment on a worker's issue, passed to the worker exactly once.
    /// Waiting for the watch, it is not typed at: its watch episode carries
    /// the comment. Its brief still to be handed, the comment goes into
    /// that brief, read once handed. Else it is typed at its session behind
    /// any running turn. Answers how (`submitted`, `queued`, `brief`,
    /// `watch`, or `none`), and why it did not reach the worker; the issue
    /// keeps the comment for the next brief either way.
    pub(crate) async fn pass_comment(&self, worker: Uuid, text: &str) -> Answer<(&'static str, Option<String>)> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.as_ref().filter(|_| wk.status != Status::Ended) else { return Ok(("none", None)) };
        if wk.status == Status::Waiting {
            self.submit::<w::Hear>(w::Heard { worker, text: text.into() }).await?;
            return Ok(("watch", None));
        }
        if wk.unhanded().is_some() {
            return Ok(match add_to_brief(&st.brief, text) {
                Ok(()) => ("brief", None),
                Err(e) => ("none", Some(format!("its brief {}: {e}", st.brief))),
            });
        }
        let line = one_line(&format!("factorium (a comment on your issue): {text}"));
        Ok(match Self::worker_session(st).and_then(|s| self.deliver_at(&s, &line, false)) {
            Ok(d) => (d.word(), None),
            Err(r) => ("none", Some(r.message)),
        })
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

/// Adds a comment to a brief not yet handed, under its own heading.
fn add_to_brief(brief: &str, text: &str) -> std::io::Result<()> {
    const HEAD: &str = "\n## Comments since\n\n";
    let mut b = std::fs::read_to_string(brief)?;
    if !b.contains(HEAD) {
        b.push_str(HEAD);
    }
    b.push_str(&format!("- {text}\n"));
    std::fs::write(brief, b)
}
