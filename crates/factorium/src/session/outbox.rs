//! The one route of every message to a session. A caller says who it is
//! from and what it says; the outbox formats it, gives it to the session's
//! record, and tries what the session has to be typed, oldest first: typed
//! only once its harness is up, the running turn stopped first only when
//! asked, and checked against the session's turn. What is held is tried
//! again on the session's next look. A worker a message is lost on goes to
//! the watch at once.

use factorium_domain::outbox::{
    Give, GiveMessage, Message, Outbox, Outboxes, Outcome, Sender, Try, TryMessage, outbox_id,
};
use factorium_domain::watch::{Watches, watch_id};
use factorium_domain::worker as w;
use factorium_harness::Turn;
use factorium_host::{Delivery, Term};
use factum::Uuid;
use serde_json::json;

use super::Session;
use crate::{Answer, App, Refusal};

impl App {
    /// Passes `text` from `from` to session `s`, behind what it holds; answers
    /// how it went. `now`: first stops the running turn with the harness's
    /// own key, so a session stuck in a call reads it at once.
    pub(crate) async fn tell(&self, s: &Session, from: Sender, text: &str, now: bool) -> Answer<Outcome> {
        let n = self.give(s, from, text).await?;
        Ok(self.flush(&[(s, Some((n, now)))]).await?.remove(0).expect("the message given is tried"))
    }

    /// Gives `text` from `from` to session `s`'s record, behind what it
    /// holds, untried: its number, for [`App::flush`] to try.
    pub(crate) async fn give(&self, s: &Session, from: Sender, text: &str) -> Answer<u64> {
        let text = format_message(&from, text);
        self.submit::<GiveMessage>(Give { terminal: s.terminal.clone(), from, text }).await?;
        Ok(self.outbox(s).await?.messages.len() as u64)
    }

    /// Hands a session the line its start left to be typed, once its
    /// harness has had up to a minute to settle.
    pub(crate) async fn hand_brief(&self, s: &Session, typed: &str) -> Answer<Outcome> {
        let _ = self.host.orca().ready(&s.terminal);
        self.tell(s, Sender::Brief, typed, false).await
    }

    /// Tries again what each of `sessions` holds, the sessions at once: on
    /// each look at them.
    pub(crate) async fn retry(&self, sessions: &[&Session]) -> Answer<()> {
        let asked: Vec<_> = sessions.iter().map(|s| (*s, None)).collect();
        self.flush(&asked).await.map(drop)
    }

    /// `factorium outbox`: what was passed to worker `worker`'s session, or
    /// the watch's, and how each delivery went, oldest first.
    pub async fn outbox_of(&self, worker: Option<Uuid>) -> Answer {
        let terminal = match worker {
            Some(id) => self.state::<w::Workers>(id).await?.started.map(|st| st.terminal),
            None => self.state::<Watches>(watch_id()).await?.terminal,
        };
        let terminal = terminal.ok_or_else(|| Refusal::new("no_session", "no such session"))?;
        let messages = self.state::<Outboxes>(outbox_id(&terminal)).await?.messages;
        Ok(json!({ "terminal": terminal, "messages": messages }))
    }

    /// Session `s`'s outbox.
    pub(crate) async fn outbox(&self, s: &Session) -> Answer<Outbox> {
        self.state::<Outboxes>(outbox_id(&s.terminal)).await
    }

    /// Tries each message each session has still to be tried, oldest first;
    /// `now`: its message `n` stops the running turn first. What comes after
    /// a brief still held waits on it: a comment goes into the brief,
    /// anything else is refused as the brief was. The sessions are typed at
    /// at once, each one's next message on a thread of its own: Orca waits
    /// on a busy agent for its whole window (factorium#279). Answers, by
    /// session, how its message `n` went.
    pub(crate) async fn flush(&self, asked: &[(&Session, Option<(u64, bool)>)]) -> Answer<Vec<Option<Outcome>>> {
        let mut each = Vec::new();
        for (s, now) in asked {
            let limit = match s.worker {
                Some(id) => self.state::<w::Workers>(id).await?.limit,
                None => None,
            };
            let pending = self.outbox(s).await?.messages.into_iter().filter(Message::pending).collect();
            each.push(Flushing { s, now: *now, limit, pending, blocked: None, told: None });
        }
        loop {
            let next: Vec<(usize, Message)> =
                each.iter_mut().enumerate().filter_map(|(i, f)| f.pending.pop_front().map(|m| (i, m))).collect();
            if next.is_empty() {
                break;
            }
            let outcomes: Vec<Outcome> = match &next[..] {
                [(i, m)] => vec![self.try_one(&each[*i], m)],
                _ => std::thread::scope(|scope| {
                    let each = &each;
                    let running: Vec<_> =
                        next.iter().map(|(i, m)| scope.spawn(move || self.try_one(&each[*i], m))).collect();
                    running.into_iter().map(|r| r.join().expect("a try does not panic")).collect()
                }),
            };
            for ((i, m), outcome) in next.into_iter().zip(outcomes) {
                let f = &mut each[i];
                self.submit::<TryMessage>(Try { terminal: f.s.terminal.clone(), n: m.n, outcome: outcome.clone() })
                    .await?;
                let m = Message { outcome: Some(outcome.clone()), ..m };
                if m.pending() {
                    f.blocked = Some((m.from.clone(), outcome.clone()));
                }
                if let (Some(worker), Some(why)) = (f.s.worker, m.lost()) {
                    match self.submit::<w::Wait>(w::Because { worker, why }).await {
                        // An ended worker waits for no one.
                        Err(r) if r.code == "ended" => {}
                        other => other?,
                    }
                }
                if f.now.is_some_and(|(n, _)| n == m.n) {
                    f.told = Some(outcome);
                }
            }
        }
        Ok(each.into_iter().map(|f| f.told).collect())
    }

    /// How one try of message `m` of the session `f` flushes goes.
    fn try_one(&self, f: &Flushing, m: &Message) -> Outcome {
        let s = f.s;
        match (&f.blocked, &m.from) {
            (Some((Sender::Brief, _)), Sender::Comment | Sender::Role) => add_to_brief(&s.brief, &m.text)
                .map_or_else(|e| not_typed("io", format!("its brief {}: {e}", s.brief)), |()| Outcome::Brief),
            (Some((_, held)), _) => held.clone(),
            (None, _) => match self.held(s, f.limit.as_ref()) {
                Some(held) => held,
                None => self.deliver(s, m, f.now == Some((m.n, true))),
            },
        }
    }

    /// Whether what is for session `s` is held, not typed: its engine is at
    /// its limit, as its worker's `limit` recorded it or its screen says
    /// now in other words; typed there, a message would start no turn. A
    /// worker's only: its reset is what sends it back.
    fn held(&self, s: &Session, limit: Option<&w::Limit>) -> Option<Outcome> {
        s.worker?;
        let until = match limit.filter(|l| l.until > self.now()) {
            Some(l) => l.until,
            None => {
                let Term::Running(screen) = self.host.orca().screen(&s.terminal) else { return None };
                self.new_limit(s.harness.stopped(&screen), limit.map(|l| l.said.as_str()))?.1
            }
        };
        Some(Outcome::Held { until, why: format!("its engine is at its limit until it resets at {until} (ms)") })
    }

    /// Types one message at its session. A brief is typed and no more; any
    /// other is watched for the turn it starts, and checked against the
    /// session's turn: it waits behind a running turn, or it missed.
    fn deliver(&self, s: &Session, m: &Message, now: bool) -> Outcome {
        if let Err(r) = self.up(s) {
            return not_typed(r.code, r.message);
        }
        let orca = self.host.orca();
        if m.from == Sender::Brief {
            return orca.send(&s.terminal, &m.text).map_or_else(|e| not_typed("orca", e), |()| Outcome::Typed);
        }
        let since = (self.now() / 1000).saturating_sub(1) as u64;
        let key = now.then(|| s.harness.interrupt());
        let seen = match orca.deliver(&s.terminal, &m.text, key) {
            Ok(seen) => seen,
            Err(e) => return not_typed("orca", e),
        };
        if seen == Delivery::Submitted {
            return Outcome::Submitted;
        }
        // Orca misses a turn that starts right after an interrupt or ends
        // within a second; the session's own file, where its harness keeps
        // one, says whether the text was taken as a prompt. One taken at the
        // end of Orca's window is written a moment after it: the file is
        // read again for a few seconds before the text is called missed.
        let home = self.session_home(s.harness, s.account.as_deref());
        for waited in 0..=GRACE_SECS {
            if s.harness.prompted(&home, &s.cwd, &s.brief, &m.text, since) == Some(true) {
                return Outcome::Submitted;
            }
            if matches!(self.turn(s), Some(Turn::Open { .. })) {
                return Outcome::Queued;
            }
            if waited < GRACE_SECS {
                self.host.programs().run("sleep", &["1"]);
            }
        }
        let shows = match orca.screen(&s.terminal) {
            Term::Running(screen) => {
                screen.iter().rev().take(3).rev().map(|r| r.trim()).collect::<Vec<_>>().join(" / ")
            }
            _ => "unreadable".into(),
        };
        Outcome::Missed { why: format!("typed, it started no turn and none runs; its screen ends: {shows}") }
    }
}

/// A session being flushed: what it has still to be tried this flush, what
/// holds the rest up, and how the message it was asked of went.
struct Flushing<'a> {
    s: &'a Session,
    now: Option<(u64, bool)>,
    limit: Option<w::Limit>,
    pending: std::collections::VecDeque<Message>,
    blocked: Option<(Sender, Outcome)>,
    told: Option<Outcome>,
}

/// How long a session has, after Orca's window, to write the prompt it took.
const GRACE_SECS: u32 = 3;

/// What a sender who waits on the answer hears: the refusal of a message
/// not typed.
pub(crate) fn heard(outcome: Outcome) -> Answer<Outcome> {
    match outcome {
        Outcome::NotTyped { code, why } => Err(Refusal::new(&code, why)),
        other => Ok(other),
    }
}

fn not_typed(code: impl Into<String>, why: impl Into<String>) -> Outcome {
    Outcome::NotTyped { code: code.into(), why: why.into() }
}

/// A message as it is typed: opened as its sender's, on one line.
fn format_message(from: &Sender, text: &str) -> String {
    if *from == Sender::Brief {
        return text.into();
    }
    let text = format!("{}{text}", from.prefix());
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" | ")
}

/// Adds a comment, or word that its role changed, to a brief not yet
/// handed, under its own heading: a comment without its prefix, the word
/// with its own.
fn add_to_brief(brief: &str, typed: &str) -> std::io::Result<()> {
    const HEAD: &str = "\n## Comments since\n\n";
    let text = typed.strip_prefix(&Sender::Comment.prefix()).unwrap_or(typed);
    let mut b = std::fs::read_to_string(brief)?;
    if !b.contains(HEAD) {
        b.push_str(HEAD);
    }
    b.push_str(&format!("- {text}\n"));
    std::fs::write(brief, b)
}
