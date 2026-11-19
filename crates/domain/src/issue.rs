//! An issue in the host's tracker. Its number is the next of its checkout;
//! its stream is derived from the two, so a number is taken once.

use factum_core::{Aggregate, Rejection, Upcast, Uuid};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::task::Task;
use crate::{command, issue_id, named, no};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Ready,
    WaitingForHuman,
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
    /// The issues of other checkouts it waits for.
    pub across: Vec<Across>,
    /// Why it waits for a person, or why it is closed.
    pub reason: Option<String>,
    /// The worker that holds it now.
    pub worker: Option<Uuid>,
    /// How many workers have taken it.
    pub attempts: u32,
    /// The one record of what was said about it, oldest first.
    pub comments: Vec<Note>,
    /// The task it was filed for, by idle work (§1.16).
    pub task: Option<Task>,
    /// When it was closed, in ms (unknown for issues closed before it was kept).
    pub closed_at: Option<i64>,
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

/// An issue of another checkout an issue waits for. It is met once that
/// issue is closed; with `cut`, once a cut of that checkout holds its
/// landing too, installed where the repo's cut names a formula (a tool used
/// from another repo is needed as the installed release).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Across {
    pub checkout: String,
    pub number: u64,
    #[serde(default)]
    pub cut: bool,
}

impl Across {
    fn same(&self, other: &Across) -> bool {
        (self.checkout.as_str(), self.number) == (other.checkout.as_str(), other.number)
    }
}

impl std::fmt::Display for Across {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}{}", self.checkout, self.number, if self.cut { "@cut" } else { "" })
    }
}

/// What an issue waits for, as the CLI names it: `N`, an issue of its own
/// checkout; `<checkout>#N`, one of another, met when it is closed;
/// `<checkout>#N@cut`, met once a cut holds its landing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    Here(u64),
    Across(Across),
}

impl From<u64> for Blocker {
    fn from(n: u64) -> Self {
        Blocker::Here(n)
    }
}

impl std::str::FromStr for Blocker {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("`{s}`: name an issue as N, <checkout>#N or <checkout>#N@cut");
        let Some((checkout, rest)) = s.split_once('#') else { return s.parse().map(Blocker::Here).map_err(|_| bad()) };
        let (number, cut) = match rest.split_once('@') {
            Some((n, "cut")) => (n, true),
            Some(_) => return Err(bad()),
            None => (rest, false),
        };
        let number = number.parse().map_err(|_| bad())?;
        if checkout.is_empty() {
            return Err(bad());
        }
        Ok(Blocker::Across(Across { checkout: checkout.into(), number, cut }))
    }
}

impl Blocker {
    /// As an issue of `checkout` waits for it: one of its own checkout
    /// named in full is one of its own, unless it waits for a cut.
    pub fn of(self, checkout: &str) -> Blocker {
        match self {
            Blocker::Across(a) if a.checkout == checkout && !a.cut => Blocker::Here(a.number),
            b => b,
        }
    }
}

impl Issue {
    pub fn opened(&self) -> bool {
        self.number > 0
    }

    /// Everything it waits for, each named as the CLI names it: one of its
    /// own checkout as `#N`, with the issue it is as one of any checkout.
    pub fn waits_for(&self) -> Vec<(String, Across)> {
        let here = self
            .blocked_by
            .iter()
            .map(|&number| (format!("#{number}"), Across { checkout: self.checkout.clone(), number, cut: false }));
        here.chain(self.across.iter().map(|a| (a.to_string(), a.clone()))).collect()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IssueEvent {
    IssueOpened {
        checkout: String,
        number: u64,
        title: String,
        body: String,
        waiting_for_human: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<Task>,
    },
    IssueEdited {
        title: String,
        body: String,
    },
    BlockedBy {
        number: u64,
    },
    Unblocked {
        number: u64,
    },
    BlockedAcross(Across),
    UnblockedAcross {
        checkout: String,
        number: u64,
    },
    IssueWaitingForHuman {
        reason: String,
    },
    IssueReadied,
    IssueClosed {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<i64>,
    },
    IssueTaken {
        worker: Uuid,
    },
    IssueReleased,
    IssueCommented {
        text: String,
        by: Author,
        at: Option<i64>,
    },
}

/// Where an issue stands, and why it waits for a person or is closed.
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
            IssueEvent::IssueOpened { waiting_for_human: true, .. } => (Status::WaitingForHuman, None),
            IssueEvent::IssueOpened { .. } | IssueEvent::IssueReadied => (Status::Ready, None),
            IssueEvent::IssueWaitingForHuman { reason } => (Status::WaitingForHuman, Some(reason.clone())),
            IssueEvent::IssueClosed { reason, .. } => (Status::Closed, Some(reason.clone())),
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
            IssueEvent::IssueOpened { checkout, number, title, body, task, .. } => {
                *s = Issue { checkout, number, title, body, task, ..Issue::default() };
            }
            IssueEvent::IssueEdited { title, body } => (s.title, s.body) = (title, body),
            IssueEvent::BlockedBy { number } => s.blocked_by.push(number),
            IssueEvent::Unblocked { number } => s.blocked_by.retain(|n| *n != number),
            IssueEvent::BlockedAcross(a) => s.across.push(a),
            IssueEvent::UnblockedAcross { checkout, number } => {
                s.across.retain(|a| (a.checkout.as_str(), a.number) != (checkout.as_str(), number))
            }
            IssueEvent::IssueTaken { worker } => {
                s.worker = Some(worker);
                s.attempts += 1;
            }
            IssueEvent::IssueReleased => s.worker = None,
            IssueEvent::IssueClosed { at, .. } => (s.worker, s.closed_at) = (None, at),
            IssueEvent::IssueCommented { text, by, at } => s.comments.push(Note { text, by, at }),
            IssueEvent::IssueWaitingForHuman { .. } | IssueEvent::IssueReadied => {}
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

    /// "4": it keeps the task it was filed for, and when it closed.
    fn code() -> &'static str {
        "4"
    }
}

/// Which issue: a checkout and a number.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ref {
    pub checkout: String,
    pub number: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Open {
    pub checkout: String,
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub waiting_for_human: bool,
    /// The issues of its checkout it waits for from the start.
    #[serde(default)]
    pub blocked_by: Vec<u64>,
    /// The issues of other checkouts it waits for from the start.
    #[serde(default)]
    pub across: Vec<Across>,
    /// The task it is filed for, by idle work.
    #[serde(default)]
    pub task: Option<Task>,
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
    if p.blocked_by.contains(&p.number) || p.across.iter().any(|a| (a.checkout == p.checkout) && (a.number == p.number))
    {
        return no("blocks_itself", "an issue cannot wait for itself");
    }
    let mut events = vec![IssueEvent::IssueOpened {
        checkout: p.checkout.clone(),
        number: p.number,
        title: p.title.trim().to_string(),
        body: p.body.clone(),
        waiting_for_human: p.waiting_for_human,
        task: p.task.clone(),
    }];
    let mut blockers = p.blocked_by.clone();
    blockers.sort_unstable();
    blockers.dedup();
    events.extend(blockers.into_iter().map(|number| IssueEvent::BlockedBy { number }));
    let mut across: Vec<&Across> = Vec::new();
    for a in &p.across {
        if !across.iter().any(|b| b.same(a)) {
            across.push(a);
        }
    }
    events.extend(across.into_iter().cloned().map(IssueEvent::BlockedAcross));
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

/// Issue `number` of `checkout` waits for `by`, of another checkout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockAcross {
    pub checkout: String,
    pub number: u64,
    pub by: Across,
}

on_issue!(BlockIssueAcross, "issue.block_across", BlockAcross, |s, p, now| {
    if (p.by.checkout == p.checkout) && (p.by.number == p.number) {
        return no("blocks_itself", "an issue cannot wait for itself");
    }
    Ok(match s.across.iter().find(|a| a.same(&p.by)) {
        Some(a) if *a == p.by => Vec::new(),
        // Waits for it otherwise now: for its cut, or no longer.
        Some(_) => vec![
            IssueEvent::UnblockedAcross { checkout: p.by.checkout.clone(), number: p.by.number },
            IssueEvent::BlockedAcross(p.by.clone()),
        ],
        None => vec![IssueEvent::BlockedAcross(p.by.clone())],
    })
});

on_issue!(UnblockIssueAcross, "issue.unblock_across", BlockAcross, |s, p, now| {
    Ok(if s.across.iter().any(|a| a.same(&p.by)) {
        vec![IssueEvent::UnblockedAcross { checkout: p.by.checkout.clone(), number: p.by.number }]
    } else {
        Vec::new()
    })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Because {
    pub checkout: String,
    pub number: u64,
    pub reason: String,
}

// Only a person moves it again, with `issue ready`: for a decision only a
// person can make.
on_issue!(WaitForHuman, "issue.wait_for_human", Because, |s, p, now| {
    Ok(match s.status {
        Status::WaitingForHuman => Vec::new(),
        _ => vec![IssueEvent::IssueWaitingForHuman { reason: p.reason.clone() }],
    })
});

on_issue!(ReadyIssue, "issue.ready", Ref, |s, p, now| {
    Ok(match s.status {
        Status::WaitingForHuman => vec![IssueEvent::IssueReadied],
        _ => Vec::new(),
    })
});

on_issue!(CloseIssue, "issue.close", Because, |s, p, now| {
    Ok(vec![IssueEvent::IssueClosed { reason: p.reason.clone(), at: Some(now.0) }])
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
