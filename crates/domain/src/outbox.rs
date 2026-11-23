//! A session's outbox: every message passed to a worker's or the watch's
//! session, who it is from, and how its delivery went, in one record. The
//! brief is a session's first message: until it is typed, what comes after
//! it waits on it.

use factum_core::{Aggregate, Uuid};
use serde::{Deserialize, Serialize};

use crate::{command, id};

/// The stream of the outbox of the session in terminal `terminal`.
pub fn outbox_id(terminal: &str) -> Uuid {
    id(&format!("outbox:{terminal}"))
}

/// Who a message is from, and so how it opens on the session's screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "n")]
pub enum Sender {
    /// Its brief: the line its harness's start line left to be typed.
    Brief,
    /// factorium itself: a landing's red.
    Factorium,
    /// The watch: what it tells a worker, or restarts it with.
    Watch,
    /// A comment on the worker's issue.
    Comment,
    /// factorium: the worker's role directory changed under it.
    Role,
    /// factorium: the body of the worker's issue was edited, and its
    /// brief rewritten to it.
    Body,
    /// The watch's round `n`.
    Round(u64),
}

impl Sender {
    /// The words a message from it opens with.
    pub fn prefix(&self) -> String {
        match self {
            Sender::Brief => String::new(),
            Sender::Factorium => "factorium: ".into(),
            Sender::Watch => "factorium (the watch): ".into(),
            Sender::Comment => "factorium (a comment on your issue): ".into(),
            Sender::Role => "factorium (your role changed): ".into(),
            Sender::Body => "factorium (your issue's body changed): ".into(),
            Sender::Round(n) => format!("factorium round {n}: "),
        }
    }

    /// Whether a message from it that its session cannot take now is kept
    /// and tried again on the session's next look. The rest are refused at
    /// once, and the sender hears why.
    pub fn holds(&self) -> bool {
        *self == Sender::Brief
    }

    /// Whether a message from it tells of nothing in its worker's task: word
    /// that its role changed. Typed and missed, it is kept and typed again on
    /// the session's next look, and its worker goes to no one over it; so
    /// no look waits on it past Orca's window (factorium#281).
    pub fn notice(&self) -> bool {
        *self == Sender::Role
    }

    /// Whether no one hears at once that a message from it was refused, so
    /// a worker it does not reach goes to the watch: factorium's own word.
    fn unheard(&self) -> bool {
        *self == Sender::Factorium
    }

    fn noun(&self) -> &'static str {
        match self {
            Sender::Brief => "its brief",
            Sender::Factorium => "factorium's word",
            Sender::Watch => "the watch's word",
            Sender::Comment => "a comment on its issue",
            Sender::Role => "word that its role changed",
            Sender::Body => "word that its issue's body changed",
            Sender::Round(_) => "the round",
        }
    }
}

/// How a message's delivery went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Outcome {
    /// Typed at its harness's prompt; no turn is watched for (a brief).
    Typed,
    /// It started a turn: the session reads it now.
    Submitted,
    /// It waits behind the session's running turn.
    Queued,
    /// Written into the session's brief, still to be typed.
    Brief,
    /// Not typed: the session could not take it now (its harness not up,
    /// its terminal gone, Orca refused), with the refusal's code.
    NotTyped { code: String, why: String },
    /// Typed, and it started no turn and none runs: something other than
    /// the session's prompt took it.
    Missed { why: String },
    /// Not typed: its engine is at its limit until then (ms). Kept, and
    /// tried again once the limit resets.
    Held { until: i64, why: String },
}

impl Outcome {
    /// It reached the session, or its brief.
    pub fn arrived(&self) -> bool {
        matches!(self, Outcome::Typed | Outcome::Submitted | Outcome::Queued | Outcome::Brief)
    }

    /// The word an answer gives for it.
    pub fn word(&self) -> &'static str {
        match self {
            Outcome::Typed => "typed",
            Outcome::Submitted => "submitted",
            Outcome::Queued => "queued",
            Outcome::Brief => "brief",
            Outcome::NotTyped { .. } => "none",
            Outcome::Missed { .. } => "missed",
            Outcome::Held { .. } => "held",
        }
    }

    /// Why it did not arrive.
    pub fn why(&self) -> Option<&str> {
        match self {
            Outcome::NotTyped { why, .. } | Outcome::Missed { why } | Outcome::Held { why, .. } => Some(why),
            _ => None,
        }
    }
}

/// One message, as given and as its delivery went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub n: u64,
    pub from: Sender,
    /// What is typed: one line, opened as its sender's.
    pub text: String,
    /// When it was given, in ms.
    pub at: i64,
    /// How its last try went; `None`: not tried yet.
    pub outcome: Option<Outcome>,
}

impl Message {
    /// Still to be tried: never tried, held, or a notice missed.
    pub fn pending(&self) -> bool {
        match &self.outcome {
            None => true,
            Some(Outcome::Missed { .. }) => self.from.notice(),
            Some(Outcome::NotTyped { .. }) => self.from.holds(),
            Some(Outcome::Held { .. }) => true,
            Some(_) => false,
        }
    }

    /// Why the worker it was for goes to the watch over it: typed and
    /// missed, not a notice, or refused with no one to hear it.
    pub fn lost(&self) -> Option<String> {
        let why = match &self.outcome {
            Some(Outcome::Missed { why }) if !self.from.notice() => why,
            Some(Outcome::NotTyped { why, .. }) if self.from.unheard() => why,
            _ => return None,
        };
        Some(format!("{} did not reach it: {why}", self.from.noun()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Outbox {
    pub messages: Vec<Message>,
}

impl Outbox {
    /// Its brief, while it is still to be typed.
    pub fn held(&self) -> Option<&Message> {
        self.messages.iter().find(|m| m.from.holds() && m.pending())
    }

    /// Whether a message from `from` arrived.
    pub fn arrived(&self, from: &Sender) -> bool {
        self.messages.iter().any(|m| m.from == *from && m.outcome.as_ref().is_some_and(Outcome::arrived))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum OutboxEvent {
    Given { n: u64, from: Sender, text: String, at: i64 },
    Tried { n: u64, outcome: Outcome },
}

pub struct Outboxes;

impl Aggregate for Outboxes {
    const NAME: &'static str = "outbox";
    type State = Outbox;
    type Event = OutboxEvent;

    fn evolve(s: &mut Outbox, event: OutboxEvent) {
        match event {
            OutboxEvent::Given { n, from, text, at } => s.messages.push(Message { n, from, text, at, outcome: None }),
            OutboxEvent::Tried { n, outcome } => {
                if let Some(m) = s.messages.iter_mut().find(|m| m.n == n) {
                    m.outcome = Some(outcome);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Give {
    pub terminal: String,
    pub from: Sender,
    pub text: String,
}

// A message for the session in `terminal`, numbered one past its last.
command!(GiveMessage, "outbox.give", Outboxes, Give, |p| outbox_id(&p.terminal), |s, p, now| Ok(vec![
    OutboxEvent::Given { n: s.messages.len() as u64 + 1, from: p.from.clone(), text: p.text.clone(), at: now.0 }
]));

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Try {
    pub terminal: String,
    pub n: u64,
    pub outcome: Outcome,
}

// How a try at message `n` went; a try that went as the last did is
// not recorded again.
command!(TryMessage, "outbox.tried", Outboxes, Try, |p| outbox_id(&p.terminal), |s, p, _now| {
    let Some(m) = s.messages.iter().find(|m| m.n == p.n) else {
        return crate::no("no_message", format!("no message {}", p.n));
    };
    Ok(if m.outcome.as_ref() == Some(&p.outcome) {
        Vec::new()
    } else {
        vec![OutboxEvent::Tried { n: p.n, outcome: p.outcome.clone() }]
    })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HoldTaken {
    pub terminal: String,
    /// Submitted at or after then (ms), or queued at or after `queued`.
    pub submitted: i64,
    pub queued: i64,
    pub until: i64,
}

// Its engine was found at its limit: what it took just before is no
// proof it read it (agy takes a message, starts a turn, and ends it at
// once on its quota), so a message submitted since `submitted`, or
// queued since `queued`, is held again, typed at the reset.
command!(HoldTakenMessages, "outbox.hold_taken", Outboxes, HoldTaken, |p| outbox_id(&p.terminal), |s, p, _now| {
    let why = format!("its engine reached its limit as it took it; typed again once it resets at {} (ms)", p.until);
    let held = Outcome::Held { until: p.until, why };
    let taken = |m: &&Message| match m.outcome {
        Some(Outcome::Submitted) => m.at >= p.submitted,
        Some(Outcome::Queued) => m.at >= p.queued,
        _ => false,
    };
    Ok::<_, factum_core::Rejection>(
        s.messages.iter().filter(taken).map(|m| OutboxEvent::Tried { n: m.n, outcome: held.clone() }).collect(),
    )
});
