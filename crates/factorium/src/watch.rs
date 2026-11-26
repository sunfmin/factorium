//! The watch as the app runs it: the look, the rounds sent to the watch's
//! session, and the watch's own commands.

use factorium_domain::doctor;
use factorium_domain::issue::{Author, Issue};
use factorium_domain::outbox::{Outcome, Sender};
use factorium_domain::settings::{Setting, Settings, Time};
use factorium_domain::tracker;
use factorium_domain::watch::{self, FileReport, Filed, Look, LookNow, StartWatch, Watches, watch_id};
use factorium_domain::worker::{self as w, Status};
use factum::{Uuid, Value};
use serde_json::json;

use crate::life::{End, brief_head};
use crate::session::{Opened, Role};
use crate::{Answer, App, Live, Refusal, Want};

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
                // Briefed with rules this binary does not hold: between
                // rounds, never in one, it is closed for a fresh session.
                Ok(false) if before.round.is_some() || before.rules.as_deref() == Some(&self.watch_rules_hash()) => {
                    return Ok(Some(s.terminal().into()));
                }
                Ok(false) => _ = self.close_watch_session().await?,
                // Not known to be gone: no second session; none to type at now.
                Err(_) => return Ok(None),
                Ok(true) => {}
            }
        }
        let pick = match self.pick("", &Default::default()).await {
            Ok(pick) => pick,
            Err(r) if r.code == "no_harness" => return Ok(None),
            Err(r) => return Err(r),
        };
        let Opened { terminal, typed, rules, .. } = self.open_session(Role::Watch, &pick)?;
        self.submit::<StartWatch>(watch::Started {
            terminal: terminal.clone(),
            harness: pick.harness.key().into(),
            account: pick.account.clone(),
            rules,
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
            let on = self.issue(&w.checkout, w.number).await?;
            if w.state.since > last_look || on.heard_since(w.state.since).any(|n| n.at > Some(last_look)) {
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
                self.retry(&[&s]).await?;
                self.watch_ready(&started).await?
            }
            _ => None,
        };
        let ready = session.is_some();
        let (bound, again) = (settings.ms(Time::RoundBound), settings.ms(Time::RoundAgain));
        let rounds_max = settings.get(Setting::RoundsMax) as u32;
        self.submit::<LookNow>(Look { found: found.clone(), bound, again, rounds_max, ready }).await?;
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
            let before = match &e.ended {
                None => String::new(),
                Some(d) => {
                    let did = d.report.as_deref().map_or("your round ran past its bound unreported".into(), |t| {
                        format!("you reported: \"{t}\"")
                    });
                    format!(" — Round {} on this finding: last time {did}; the finding is still there", e.rounds)
                }
            };
            let line = format!(
                "{}{heard}{before} — How to fix: {} — Act with factorium's commands (see `factorium help`). \
                 When done, file `factorium watch report --text \"what you found and did\"` (add --human \"...\" \
                 only for what needs the owner: spending money). `factorium watch next` shows this round again.",
                e.what, e.fix
            );
            // Missed, it is passed again on the next look.
            self.tell(s, Sender::Round(r.n), &line, false).await?;
        }
        // The look read the store through: no outage, and where the watch is reached without it.
        self.keep_reach(&after);
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
        self.restart(worker, Author::Watch, text, now).await
    }

    /// Sends a waiting worker back to work, told `text`, kept as `by`'s comment.
    pub(crate) async fn restart(&self, worker: Uuid, by: Author, text: &str, now: bool) -> Answer {
        let live = self.live(worker, Want::Any).await?;
        // What the owner said while it waited went to the watch, not to it: it goes with this line.
        let heard: Vec<String> = match live.hears_through_watch() {
            true => self
                .issue(&live.started.checkout, live.started.number)
                .await?
                .heard_since(live.since)
                .map(|n| format!("\"{}\"", n.text))
                .collect(),
            false => Vec::new(),
        };
        let Live { started: st, session, .. } = live;
        let from = if by == Author::Watch { Sender::Watch } else { Sender::Factorium };
        // Kept on its issue as it is typed: this line is its delivery.
        self.note(&st.checkout, st.number, by, text).await?;
        if self.gone(&session).map_err(|why| Refusal::new("orca", why))? {
            self.end_worker(worker, End::because("restarted: its session was gone", true)).await?;
            let issue = format!("{}#{}", st.checkout, st.number);
            return Ok(json!({ "worker": worker, "ended": true, "issue": issue }));
        }
        let heard = match heard.as_slice() {
            [] => String::new(),
            said => format!(" Comments on your issue while you waited: {}.", said.join(" / ")),
        };
        let line = format!("{text}{heard} Then go on, and run `factorium worker done` when the work is committed.");
        // A brief never handed goes first: the word is about its work.
        let delivery = self.tell(&session, from, &line, now).await.and_then(crate::session::heard)?;
        if let Outcome::Missed { why } = &delivery {
            return Ok(json!({ "worker": worker, "resumed": false, "delivery": delivery.word(), "missed": why }));
        }
        // Its engine is at its limit: it stays parked, and the reset sends it back.
        if let Outcome::Held { until, .. } = &delivery {
            return Ok(
                json!({ "worker": worker, "resumed": false, "delivery": delivery.word(), "until": until, "missed": delivery.why() }),
            );
        }
        self.submit::<w::Resume>(w::Of { worker }).await?;
        Ok(json!({ "worker": worker, "resumed": true, "delivery": delivery.word() }))
    }

    /// `factorium watch tell`: types a line at a worker's session; answers
    /// whether it started a turn (`submitted`) or waits behind the running
    /// one (`queued`). `now`: first stops the running turn. To a worker that
    /// waits for the watch it is [`App::watch_restart`]: it ends the wait.
    pub async fn watch_tell(&self, worker: Uuid, text: &str, now: bool) -> Answer {
        let Live { started: st, session, waiting, .. } = self.live(worker, Want::Any).await?;
        if waiting {
            return self.restart(worker, Author::Watch, text, now).await;
        }
        // Kept on its issue as it is typed: this line is its delivery.
        self.note(&st.checkout, st.number, Author::Watch, text).await?;
        let delivery = self.tell(&session, Sender::Watch, text, now).await.and_then(crate::session::heard)?;
        Ok(json!({ "worker": worker, "told": text, "delivery": delivery.word(), "missed": delivery.why() }))
    }

    /// A comment on a worker's issue, passed to the worker exactly once.
    /// Waiting for the watch, it is not typed at: its watch episode reads
    /// the comment from the issue (`Issue::heard_since`), and the line that
    /// sends it back carries it ([`App::restart`]); parked on its
    /// engine's limit, it is held, typed once the limit resets. Else it goes
    /// through its session's outbox: into its brief while that is still to be typed, or typed behind any running
    /// turn. Answers how (`submitted`, `queued`, `brief`, `held`, `missed`, `watch`,
    /// or `none`), and why it did not reach the worker; the issue
    /// keeps the comment for the next brief either way.
    pub(crate) async fn pass_comment(&self, worker: Uuid, text: &str) -> Answer<(&'static str, Option<String>)> {
        let Some(live) = self.live_or_none(worker, Want::Any).await? else { return Ok(("none", None)) };
        // Parked on its engine's limit, it is held and typed at the reset.
        if live.hears_through_watch() {
            return Ok(("watch", None));
        }
        let d = self.tell(&live.session, Sender::Comment, text, false).await?;
        Ok((d.word(), d.why().map(str::to_string)))
    }

    /// An edit of a worker's issue, from `was` to `is`, reaching the
    /// worker: the head of its brief (title and body) is rewritten to the
    /// issue as it is now, so the brief never shows an old body as current;
    /// a body changed is then told to it as a comment is
    /// ([`App::pass_comment`]), pointing at its brief. Answers how, as
    /// `pass_comment` does (`brief`: a waiting worker's brief was
    /// rewritten, and it is not typed at), when the body changed.
    pub(crate) async fn pass_edit(
        &self,
        worker: Uuid,
        was: &Issue,
        is: &Issue,
    ) -> Answer<Option<(&'static str, Option<String>)>> {
        let (old, new) = (brief_head(was), brief_head(is));
        if old == new {
            return Ok(None);
        }
        let Some(live) = self.live_or_none(worker, Want::Any).await? else { return Ok(None) };
        let brief = &live.started.brief;
        let rewritten =
            std::fs::read_to_string(brief).map_err(|e| e.to_string()).and_then(|b| match b.strip_prefix(&old) {
                Some(rest) => std::fs::write(brief, format!("{new}{rest}")).map_err(|e| e.to_string()),
                None => Err("it does not open with the body it holds".into()),
            });
        if was.body == is.body {
            return Ok(None);
        }
        if let Err(why) = rewritten {
            return Ok(Some(("none", Some(format!("its brief {brief}: {why}")))));
        }
        if live.hears_through_watch() {
            return Ok(Some(("brief", None)));
        }
        let text = format!(
            "the owner edited your issue's body; your brief {brief} now holds it as it is. Read it again and go by it, not by the body you started from."
        );
        let d = self.tell(&live.session, Sender::Body, &text, false).await?;
        Ok(Some((d.word(), d.why().map(str::to_string))))
    }
}
