//! An issue in the host's tracker. Its number is the next of its checkout;
//! its stream is derived from the two, so a number is taken once.

use factum_core::{Aggregate, By, Command, Decision, Instant, Rejection, Uuid};
use serde::{Deserialize, Serialize};

use crate::issue_id;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Ready,
    Parked,
    Closed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ready => "ready",
            Status::Parked => "parked",
            Status::Closed => "closed",
        }
    }
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
    pub comments: Vec<String>,
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
    IssueCommented { text: String },
}

pub struct Issues;

impl Aggregate for Issues {
    const NAME: &'static str = "issue";
    type State = Issue;
    type Event = IssueEvent;

    fn evolve(s: &mut Issue, event: IssueEvent) {
        match event {
            IssueEvent::IssueOpened { checkout, number, title, body, parked } => {
                *s = Issue {
                    checkout,
                    number,
                    title,
                    body,
                    status: if parked { Status::Parked } else { Status::Ready },
                    ..Issue::default()
                };
            }
            IssueEvent::IssueEdited { title, body } => {
                s.title = title;
                s.body = body;
            }
            IssueEvent::BlockedBy { number } => s.blocked_by.push(number),
            IssueEvent::Unblocked { number } => s.blocked_by.retain(|n| *n != number),
            IssueEvent::IssueParked { reason } => {
                s.status = Status::Parked;
                s.reason = Some(reason);
            }
            IssueEvent::IssueReadied => {
                s.status = Status::Ready;
                s.reason = None;
            }
            IssueEvent::IssueClosed { reason } => {
                s.status = Status::Closed;
                s.reason = Some(reason);
                s.worker = None;
            }
            IssueEvent::IssueTaken { worker } => {
                s.worker = Some(worker);
                s.attempts += 1;
            }
            IssueEvent::IssueReleased => s.worker = None,
            IssueEvent::IssueCommented { text } => s.comments.push(text),
        }
    }

    fn ending(kind: &str) -> bool {
        kind == "IssueClosed"
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
}

pub struct OpenIssue;

impl Command for OpenIssue {
    const NAME: &'static str = "issue.open";
    type Aggregate = Issues;
    type Params = Open;

    fn stream(p: &Open) -> Uuid {
        issue_id(&p.checkout, p.number)
    }

    fn decide(s: &Issue, p: &Open, _: &By, _: Instant) -> Result<Decision<IssueEvent>, Rejection> {
        if s.opened() {
            return Err(Rejection::new("number_taken", format!("{}#{} is taken", p.checkout, p.number)));
        }
        if p.number == 0 {
            return Err(Rejection::new("invalid_number", "issue numbers start at 1"));
        }
        if p.title.trim().is_empty() {
            return Err(Rejection::new("no_title", "an issue needs a title"));
        }
        Ok(vec![IssueEvent::IssueOpened {
            checkout: p.checkout.clone(),
            number: p.number,
            title: p.title.trim().to_string(),
            body: p.body.clone(),
            parked: p.parked,
        }]
        .into())
    }
}

/// Defines a command on one existing issue: its params carry `checkout` and
/// `number`, and it is refused for an issue never opened.
macro_rules! on_issue {
    ($command:ident, $name:literal, $params:ty, |$s:ident, $p:ident| $decide:expr) => {
        pub struct $command;

        impl Command for $command {
            const NAME: &'static str = $name;
            type Aggregate = Issues;
            type Params = $params;

            fn stream(p: &$params) -> Uuid {
                issue_id(&p.checkout, p.number)
            }

            fn decide($s: &Issue, $p: &$params, _: &By, _: Instant) -> Result<Decision<IssueEvent>, Rejection> {
                if !$s.opened() {
                    return Err(Rejection::new("no_issue", format!("{}#{} does not exist", $p.checkout, $p.number)));
                }
                $decide.map(Decision::from)
            }
        }
    };
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edit {
    pub checkout: String,
    pub number: u64,
    pub title: Option<String>,
    pub body: Option<String>,
}

on_issue!(EditIssue, "issue.edit", Edit, |s, p| {
    let title = p.title.clone().unwrap_or_else(|| s.title.clone());
    let body = p.body.clone().unwrap_or_else(|| s.body.clone());
    if title.trim().is_empty() {
        Err(Rejection::new("no_title", "an issue needs a title"))
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

on_issue!(BlockIssue, "issue.block", Block, |s, p| {
    if p.by == p.number {
        Err(Rejection::new("blocks_itself", "an issue cannot wait for itself"))
    } else if s.blocked_by.contains(&p.by) {
        Ok(Vec::new())
    } else {
        Ok(vec![IssueEvent::BlockedBy { number: p.by }])
    }
});

on_issue!(UnblockIssue, "issue.unblock", Block, |s, p| {
    if s.blocked_by.contains(&p.by) { Ok(vec![IssueEvent::Unblocked { number: p.by }]) } else { Ok(Vec::new()) }
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Because {
    pub checkout: String,
    pub number: u64,
    pub reason: String,
}

on_issue!(ParkIssue, "issue.park", Because, |s, p| match s.status {
    Status::Parked => Ok(Vec::new()),
    _ => Ok(vec![IssueEvent::IssueParked { reason: p.reason.clone() }]),
});

on_issue!(ReadyIssue, "issue.ready", Ref, |s, _p| match s.status {
    Status::Parked => Ok(vec![IssueEvent::IssueReadied]),
    _ => Ok(Vec::new()),
});

on_issue!(CloseIssue, "issue.close", Because, |_s, p| Ok(vec![IssueEvent::IssueClosed { reason: p.reason.clone() }]));

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Take {
    pub checkout: String,
    pub number: u64,
    pub worker: Uuid,
}

on_issue!(TakeIssue, "issue.take", Take, |s, p| match (s.status, s.worker) {
    (Status::Ready, None) => Ok(vec![IssueEvent::IssueTaken { worker: p.worker }]),
    (_, Some(_)) => Err(Rejection::new("taken", "a worker holds it already")),
    _ => Err(Rejection::new("not_ready", format!("it is {}", s.status.as_str()))),
});

on_issue!(ReleaseIssue, "issue.release", Take, |s, p| if s.worker == Some(p.worker) {
    Ok(vec![IssueEvent::IssueReleased])
} else {
    Ok(Vec::new())
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    pub checkout: String,
    pub number: u64,
    pub text: String,
}

on_issue!(CommentIssue, "issue.comment", Comment, |_s, p| if p.text.trim().is_empty() {
    Err(Rejection::new("no_text", "a comment needs text"))
} else {
    Ok(vec![IssueEvent::IssueCommented { text: p.text.clone() }])
});
