//! An issue in the host's tracker. Its number is the next of its checkout;
//! its stream is derived from the two, so a number is taken once.

use factum_core::{Aggregate, Rejection, Upcast, Uuid};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{command, issue_id, named, no};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Ready,
    Parked,
    Closed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub checkout: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub status: Status,
    /// The numbers of the issues of its checkout it waits for.
    pub blocked_by: Vec<u64>,
    /// Why it is parked or closed.
    pub reason: Option<String>,
    /// The worker that holds it now.
    pub worker: Option<Uuid>,
    /// How many workers have taken it.
    pub attempts: u32,
    /// The one record of what was said about it, oldest first.
    pub comments: Vec<Note>,
}

/// Who wrote a comment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Owner,
    Worker(Uuid),
    Watch,
    Factorium,
}

impl std::fmt::Display for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Author::Owner => f.write_str("owner"),
            Author::Worker(id) => write!(f, "worker {id}"),
            Author::Watch => f.write_str("watch"),
            Author::Factorium => f.write_str("factorium"),
        }
    }
}

/// A comment: what was said, who said it, and when, in ms (unknown for
/// comments made before authors and times were kept).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub text: String,
    pub by: Author,
    pub at: Option<i64>,
}

impl Issue {
    pub fn opened(&self) -> bool {
        self.number > 0
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IssueEvent {
    IssueOpened { checkout: String, number: u64, title: String, body: String, parked: bool },
    IssueEdited { title: String, body: String },
    BlockedBy { number: u64 },
    Unblocked { number: u64 },
    IssueParked { reason: String },
    IssueReadied,
    IssueClosed { reason: String },
    IssueTaken { worker: Uuid },
    IssueReleased,
    IssueCommented { text: String, by: Author, at: Option<i64> },
}

/// Where an issue stands, and why it is parked or closed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    pub status: Status,
    pub reason: Option<String>,
}

impl IssueEvent {
    /// Where the issue stands after this event, if the event moves it: the
    /// one place an issue's status changes, for the issue and its row.
    pub fn standing(&self) -> Option<Standing> {
        let (status, reason) = match self {
            IssueEvent::IssueOpened { parked: true, .. } => (Status::Parked, None),
            IssueEvent::IssueOpened { .. } | IssueEvent::IssueReadied => (Status::Ready, None),
            IssueEvent::IssueParked { reason } => (Status::Parked, Some(reason.clone())),
            IssueEvent::IssueClosed { reason } => (Status::Closed, Some(reason.clone())),
            _ => return None,
        };
        Some(Standing { status, reason })
    }
}

/// What a comment was before it kept its author and time: only factorium's
/// start-over wrote one besides the owner, and it is known by its words.
fn authored(mut d: Value) -> Value {
    let factorium = d["text"].as_str().is_some_and(|t| t.starts_with("Started over: attempt "));
    d["by"] = json!(if factorium { "factorium" } else { "owner" });
    d["at"] = Value::Null;
    d
}

pub struct Issues;

impl Aggregate for Issues {
    const NAME: &'static str = "issue";
    type State = Issue;
    type Event = IssueEvent;

    fn evolve(s: &mut Issue, event: IssueEvent) {
        let standing = event.standing();
        match event {
            IssueEvent::IssueOpened { checkout, number, title, body, .. } => {
                *s = Issue { checkout, number, title, body, ..Issue::default() };
            }
            IssueEvent::IssueEdited { title, body } => (s.title, s.body) = (title, body),
            IssueEvent::BlockedBy { number } => s.blocked_by.push(number),
            IssueEvent::Unblocked { number } => s.blocked_by.retain(|n| *n != number),
            IssueEvent::IssueTaken { worker } => {
                s.worker = Some(worker);
                s.attempts += 1;
            }
            IssueEvent::IssueReleased | IssueEvent::IssueClosed { .. } => s.worker = None,
            IssueEvent::IssueCommented { text, by, at } => s.comments.push(Note { text, by, at }),
            IssueEvent::IssueParked { .. } | IssueEvent::IssueReadied => {}
        }
        if let Some(Standing { status, reason }) = standing {
            (s.status, s.reason) = (status, reason);
        }
    }

    fn ending(kind: &str) -> bool {
        kind == "IssueClosed"
    }

    fn upcasts() -> Vec<(&'static str, Upcast)> {
        vec![("IssueCommented", authored)]
    }

    /// "2": comments keep their author and time.
    fn code() -> &'static str {
        "2"
    }
}

/// Which issue: a checkout and a number.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ref {
    pub checkout: String,
    pub number: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Open {
    pub checkout: String,
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub parked: bool,
    /// The issues of its checkout it waits for from the start.
    #[serde(default)]
    pub blocked_by: Vec<u64>,
}

command!(OpenIssue, "issue.open", Issues, Open, |p| issue_id(&p.checkout, p.number), |s, p, now| {
    if s.opened() {
        return no("number_taken", format!("{}#{} is taken", p.checkout, p.number));
    }
    if p.number == 0 {
        return no("invalid_number", "issue numbers start at 1");
    }
    if p.title.trim().is_empty() {
        return no("no_title", "an issue needs a title");
    }
    if p.blocked_by.contains(&p.number) {
        return no("blocks_itself", "an issue cannot wait for itself");
    }
    let mut events = vec![IssueEvent::IssueOpened {
        checkout: p.checkout.clone(),
        number: p.number,
        title: p.title.trim().to_string(),
        body: p.body.clone(),
        parked: p.parked,
    }];
    let mut blockers = p.blocked_by.clone();
    blockers.sort_unstable();
    blockers.dedup();
    events.extend(blockers.into_iter().map(|number| IssueEvent::BlockedBy { number }));
    Ok(events)
});

/// Refused for an issue never opened.
fn opened(s: &Issue, checkout: &str, number: u64) -> Result<(), Rejection> {
    if s.opened() { Ok(()) } else { no("no_issue", format!("{checkout}#{number} does not exist")) }
}

/// Defines a command on one existing issue: its params carry `checkout` and
/// `number`, and it is refused for an issue never opened.
macro_rules! on_issue {
    ($command:ident, $name:literal, $params:ty, |$s:ident, $p:ident, $now:ident| $decide:expr) => {
        command!($command, $name, Issues, $params, |p| issue_id(&p.checkout, p.number), |$s, $p, $now| {
            opened($s, &$p.checkout, $p.number)?;
            $decide
        });
    };
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edit {
    pub checkout: String,
    pub number: u64,
    pub title: Option<String>,
    pub body: Option<String>,
}

on_issue!(EditIssue, "issue.edit", Edit, |s, p, now| {
    let title = p.title.clone().unwrap_or_else(|| s.title.clone());
    let body = p.body.clone().unwrap_or_else(|| s.body.clone());
    if title.trim().is_empty() {
        no("no_title", "an issue needs a title")
    } else if title == s.title && body == s.body {
        Ok(Vec::new())
    } else {
        Ok(vec![IssueEvent::IssueEdited { title, body }])
    }
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub checkout: String,
    pub number: u64,
    pub by: u64,
}

on_issue!(BlockIssue, "issue.block", Block, |s, p, now| {
    if p.by == p.number {
        no("blocks_itself", "an issue cannot wait for itself")
    } else if s.blocked_by.contains(&p.by) {
        Ok(Vec::new())
    } else {
        Ok(vec![IssueEvent::BlockedBy { number: p.by }])
    }
});

on_issue!(UnblockIssue, "issue.unblock", Block, |s, p, now| {
    Ok(if s.blocked_by.contains(&p.by) { vec![IssueEvent::Unblocked { number: p.by }] } else { Vec::new() })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Because {
    pub checkout: String,
    pub number: u64,
    pub reason: String,
}

on_issue!(ParkIssue, "issue.park", Because, |s, p, now| {
    Ok(match s.status {
        Status::Parked => Vec::new(),
        _ => vec![IssueEvent::IssueParked { reason: p.reason.clone() }],
    })
});

on_issue!(ReadyIssue, "issue.ready", Ref, |s, p, now| {
    Ok(match s.status {
        Status::Parked => vec![IssueEvent::IssueReadied],
        _ => Vec::new(),
    })
});

on_issue!(CloseIssue, "issue.close", Because, |s, p, now| {
    Ok(vec![IssueEvent::IssueClosed { reason: p.reason.clone() }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Take {
    pub checkout: String,
    pub number: u64,
    pub worker: Uuid,
}

on_issue!(TakeIssue, "issue.take", Take, |s, p, now| match (s.status, s.worker) {
    (Status::Ready, None) => Ok(vec![IssueEvent::IssueTaken { worker: p.worker }]),
    (_, Some(_)) => no("taken", "a worker holds it already"),
    _ => no("not_ready", format!("it is {}", named(s.status))),
});

on_issue!(ReleaseIssue, "issue.release", Take, |s, p, now| {
    Ok(if s.worker == Some(p.worker) { vec![IssueEvent::IssueReleased] } else { Vec::new() })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    pub checkout: String,
    pub number: u64,
    pub text: String,
    pub by: Author,
}

on_issue!(CommentIssue, "issue.comment", Comment, |s, p, now| {
    if p.text.trim().is_empty() {
        return no("no_text", "a comment needs text");
    }
    Ok(vec![IssueEvent::IssueCommented { text: p.text.clone(), by: p.by.clone(), at: Some(now.0) }])
});
