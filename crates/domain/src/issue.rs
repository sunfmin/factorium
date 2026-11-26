//! An issue in the host's tracker. Its number is the next of its checkout;
//! its stream is derived from the two, so a number is taken once.

use factum_core::{Aggregate, Rejection, Upcast, Uuid};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::decided::{self, Mark, Numbering};
use crate::idle::Task;
use crate::{command, issue_id, named, no, only};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Ready,
    /// `parked` before factorium#95, as snapshots kept before it hold it.
    #[serde(alias = "parked")]
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
    /// The issues it waits for, of its checkout or of others.
    pub waits: Vec<Across>,
    /// The time (ms) it waits for: no worker takes it before then.
    pub not_before: Option<i64>,
    /// Why it waits for a person, or why it is closed.
    pub reason: Option<String>,
    /// The worker that holds it now.
    pub worker: Option<Uuid>,
    /// The one process acting on its worktrees now ([`Claim`]), if one is.
    #[serde(default)]
    pub claim: Option<Claim>,
    /// How many workers have taken it.
    pub attempts: u32,
    /// The one record of what was said about it, oldest first.
    pub comments: Vec<Note>,
    /// The numbers of the Decided records its body holds (SPEC §2).
    #[serde(default)]
    pub decided: Vec<Mark>,
    /// The task it was filed for, by idle work (§1.16).
    pub task: Option<Task>,
    /// What its worker plays and where it runs, as the one who filed it
    /// decided: none, the kit's and the host's choice.
    pub on: On,
    /// The owner's urgent mark ([`urgent_first`]): kept after it closes, as a record.
    #[serde(default)]
    pub urgent: bool,
}

/// `items` in the order the urgent mark gives: the urgent ones first, each
/// part in the order it had. The one step urgency adds in front of an
/// ordinary order: which ready issue starts, which landing of a checkout
/// lands next.
pub fn urgent_first<T>(items: &mut [T], urgent: impl Fn(&T) -> bool) {
    items.sort_by_key(|i| !urgent(i));
}

/// The urgent issues of `issues` that halt heavy commands not urgent (`web#3`):
/// each open one marked urgent that a worker has taken, from its first
/// worker's start until it closes or its mark is taken off.
pub fn halts(issues: &[Issue]) -> impl Iterator<Item = &Issue> {
    issues.iter().filter(|i| i.urgent && i.status != Status::Closed && i.attempts > 0)
}

/// The role an issue's worker plays and the account and model it starts on, each
/// named by the one who filed it (Guild, for one): unnamed, the worker's
/// role is factorium's alone and its account and model stablehand's pick.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct On {
    /// A directory holding the role's rules (`CLAUDE.md`) and skills
    /// (`.claude/skills`), which its worker starts with beside factorium's
    /// worker rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The engine account its worker starts on, by its name in stablehand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The model its worker starts on, by the id its engine starts it with
    /// (`stablehand pick --model`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl On {
    fn is_none(&self) -> bool {
        self.role.is_none() && self.account.is_none() && self.model.is_none()
    }
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
    /// Its number, if it is a Decided record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<u64>,
}

/// An issue an issue waits for, of its checkout or another. It is met once
/// that issue is closed; with `cut`, once a cut of that checkout holds its
/// landing too, installed where the repo's cut names a formula (a tool used
/// from another repo is needed as the installed release).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Across {
    pub checkout: String,
    pub number: u64,
    #[serde(default)]
    pub cut: bool,
    /// Why it waits for it, as the one who blocked it said (factorium#285);
    /// none for a block made before a why was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// How a block made before a why was asked for reads.
pub const NO_REASON: &str = "(no reason given)";

impl Across {
    fn same(&self, other: &Across) -> bool {
        (self.checkout.as_str(), self.number) == (other.checkout.as_str(), other.number)
    }

    /// Why it is waited for, or [`NO_REASON`].
    pub fn why(&self) -> &str {
        self.why.as_deref().unwrap_or(NO_REASON)
    }
}

impl std::fmt::Display for Across {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}{}", self.checkout, self.number, if self.cut { "@cut" } else { "" })
    }
}

/// What an issue waits for, as the CLI names it: `N`, an issue of its own
/// checkout; `<checkout>#N`, one of another, met when it is closed;
/// `<checkout>#N@cut`, met once a cut holds its landing; and a time (ms,
/// `--not-before`/`--until`), met once it has come. It waits for one time
/// at most: another replaces it, and undone, any is undone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blocker {
    Here(u64),
    Across(Across),
    NotBefore(i64),
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
        Ok(Blocker::Across(Across { checkout: checkout.into(), number, cut, why: None }))
    }
}

/// A blocker and why it blocks: a block of an issue is refused without a
/// why (factorium#285), so whoever reads it can tell a code dependency from
/// a shared device; a time needs none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    pub by: Blocker,
    #[serde(default)]
    pub why: Option<String>,
}

impl Wait {
    pub fn new(by: impl Into<Blocker>, why: &str) -> Wait {
        Wait { by: by.into(), why: Some(why.into()) }
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

/// Why an issue closed when its worker landed it: this, then the commit.
const LANDED_AS: &str = "landed as ";

/// The reason an issue closes with when it landed as `commit`.
pub fn landed_as(commit: &str) -> String {
    format!("{LANDED_AS}{commit}")
}

impl Issue {
    pub fn opened(&self) -> bool {
        self.number > 0
    }

    /// The commit it landed as, if it closed so.
    pub fn landed_as(&self) -> Option<&str> {
        self.reason.as_deref().filter(|_| self.status == Status::Closed)?.strip_prefix(LANDED_AS)
    }

    /// Why a worker on it ends, once it holds none to work: it waits for a
    /// person, or it closed. `None` while it is ready.
    pub fn left_why(&self) -> Option<String> {
        let reason = self.reason.as_deref().unwrap_or_default();
        match self.status {
            Status::Ready => None,
            Status::WaitingForHuman => Some(format!("its issue waits for a person: {reason}")),
            Status::Closed => Some(format!("its issue closed: {reason}")),
        }
    }

    /// Whether `a` is an issue of its own checkout, waited for until it
    /// closes: the CLI names it `N`.
    fn here(&self, a: &Across) -> bool {
        a.checkout == self.checkout && !a.cut
    }

    /// The numbers of the issues of its own checkout it waits for.
    pub fn blocked_by(&self) -> Vec<u64> {
        self.waits.iter().filter(|a| self.here(a)).map(|a| a.number).collect()
    }

    /// What else it waits for: issues of other checkouts, and cuts.
    pub fn blocked_across(&self) -> Vec<String> {
        self.waits.iter().filter(|a| !self.here(a)).map(ToString::to_string).collect()
    }

    /// What a worker waiting since `since` (ms) heard: the owner's comments
    /// made since then, oldest first. Comments of no known time, made
    /// before times were kept, are left out.
    pub fn heard_since(&self, since: i64) -> impl Iterator<Item = &Note> {
        self.comments.iter().filter(move |n| n.by == Author::Owner && n.at.is_some_and(|at| at >= since))
    }

    /// Everything it waits for, each named as the CLI names it: one of its
    /// own checkout as `#N`, with the issue it is as one of any checkout.
    /// Those of its own checkout come first.
    pub fn waits_for(&self) -> Vec<(String, Across)> {
        let (here, across): (Vec<_>, Vec<_>) = self.waits.iter().partition(|a| self.here(a));
        let here = here.into_iter().map(|a| (format!("#{}", a.number), a.clone()));
        here.chain(across.into_iter().map(|a| (a.to_string(), a.clone()))).collect()
    }

    /// The events that make it wait for `by`, for `why`, or, with `undo`,
    /// stop: none when it already does (or does not). Waiting for an issue
    /// otherwise (for its cut, for another why, or no longer) replaces how
    /// it waited for it. An issue waited for with no why is refused.
    fn wait(&self, by: Blocker, why: Option<&str>, undo: bool) -> Result<Vec<IssueEvent>, Rejection> {
        let why = why.map(str::trim).filter(|w| !w.is_empty()).map(String::from);
        let by = match by.of(&self.checkout) {
            Blocker::Here(number) => Across { checkout: self.checkout.clone(), number, cut: false, why },
            Blocker::Across(a) => Across { why, ..a },
            Blocker::NotBefore(at) => {
                let at = (!undo).then_some(at);
                return Ok(if at == self.not_before { Vec::new() } else { vec![IssueEvent::NotBefore { at }] });
            }
        };
        if !undo && by.checkout == self.checkout && by.number == self.number {
            return no("blocks_itself", "an issue cannot wait for itself");
        }
        if !undo && by.why.is_none() {
            let why = format!(
                "say why it waits for {by} (--why <text>): the code it needs from it; a device or host both \
                 use is a resource (worker hold / exclusive --resource), not a block"
            );
            return no("no_why", why);
        }
        let held = self.waits.iter().find(|a| a.same(&by));
        let unblock = held.map(|a| match self.here(a) {
            true => IssueEvent::Unblocked { number: a.number },
            false => IssueEvent::UnblockedAcross { checkout: a.checkout.clone(), number: a.number },
        });
        let block = || match self.here(&by) {
            true => IssueEvent::BlockedBy { number: by.number, why: by.why.clone() },
            false => IssueEvent::BlockedAcross(by.clone()),
        };
        Ok(match (held, undo) {
            (_, true) => unblock.into_iter().collect(),
            (Some(a), false) if *a == by => Vec::new(),
            (_, false) => unblock.into_iter().chain([block()]).collect(),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IssueEvent {
    IssueOpened {
        checkout: String,
        number: u64,
        title: String,
        body: String,
        /// `parked` before factorium#95.
        #[serde(alias = "parked")]
        waiting_for_human: bool,
        /// Why it waits for a person from the start, if it does and was
        /// given one (factorium#276).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<Task>,
        #[serde(default, skip_serializing_if = "On::is_none")]
        on: On,
        /// The numbers of the Decided records its body holds.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decided: Vec<Mark>,
        /// Marked urgent by the owner from the start.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        urgent: bool,
    },
    /// The owner marked it urgent, or took the mark away.
    UrgentMarked {
        urgent: bool,
    },
    /// Its title and body now; `decided`, the numbers of the body's records.
    IssueEdited {
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decided: Vec<Mark>,
    },
    /// The role its worker starts with from now: a directory, or, `None`, none.
    RoleNamed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    BlockedBy {
        number: u64,
        /// Why, if it was given one (factorium#285).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    Unblocked {
        number: u64,
    },
    BlockedAcross(Across),
    UnblockedAcross {
        checkout: String,
        number: u64,
    },
    /// It waits for this time (ms), or, `None`, for none.
    NotBefore {
        at: Option<i64>,
    },
    /// `IssueParked` before factorium#95.
    #[serde(alias = "IssueParked")]
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
    IssueClaimed {
        claim: Claim,
    },
    IssueUnclaimed,
    IssueCommented {
        text: String,
        by: Author,
        at: Option<i64>,
        /// Its number, if it is a Decided record.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decided: Option<u64>,
    },
    /// Records made before records were numbered take theirs: the body's
    /// (`body`, added to its marks) and comments' (by index).
    DecidedNumbered {
        body: Vec<Mark>,
        comments: Vec<(usize, u64)>,
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
            IssueEvent::IssueOpened { waiting_for_human: true, why, .. } => (Status::WaitingForHuman, why.clone()),
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
            IssueEvent::IssueOpened { checkout, number, title, body, task, on, decided, urgent, .. } => {
                *s = Issue { checkout, number, title, body, task, on, decided, urgent, ..Issue::default() };
            }
            IssueEvent::UrgentMarked { urgent } => s.urgent = urgent,
            IssueEvent::IssueEdited { title, body, decided } => (s.title, s.body, s.decided) = (title, body, decided),
            IssueEvent::RoleNamed { role } => s.on.role = role,
            // An own-checkout issue waited for until it closes is written as
            // `BlockedBy` and `Unblocked`; any other as `…Across`.
            IssueEvent::BlockedBy { number, why } => {
                s.waits.push(Across { checkout: s.checkout.clone(), number, cut: false, why })
            }
            IssueEvent::Unblocked { number } => {
                let own = s.checkout.clone();
                s.waits.retain(|a| !(a.checkout == own && !a.cut && a.number == number))
            }
            IssueEvent::BlockedAcross(a) => s.waits.push(a),
            IssueEvent::UnblockedAcross { checkout, number } => {
                let own = s.checkout.clone();
                s.waits.retain(|a| {
                    (a.checkout == own && !a.cut) || (a.checkout.as_str(), a.number) != (checkout.as_str(), number)
                })
            }
            IssueEvent::NotBefore { at } => s.not_before = at,
            IssueEvent::IssueTaken { worker } => {
                s.worker = Some(worker);
                s.attempts += 1;
            }
            IssueEvent::IssueReleased => s.worker = None,
            IssueEvent::IssueClaimed { claim } => s.claim = Some(claim),
            IssueEvent::IssueUnclaimed => s.claim = None,
            // Closed, it takes no start: no claim is needed.
            IssueEvent::IssueClosed { .. } => (s.worker, s.claim) = (None, None),
            IssueEvent::IssueCommented { text, by, at, decided } => s.comments.push(Note { text, by, at, decided }),
            IssueEvent::DecidedNumbered { body, comments } => s.place(&body, &comments),
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

    /// "5": it keeps what it waits for, of its checkout or another, in one list.
    fn code() -> &'static str {
        "5"
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
    /// Why it waits for a person from the start; read only with `waiting_for_human`.
    #[serde(default)]
    pub why: Option<String>,
    /// What it waits for from the start, each with why: issues, of its
    /// checkout or others, and a time.
    #[serde(default)]
    pub blocked_by: Vec<Wait>,
    /// The task it is filed for, by idle work.
    #[serde(default)]
    pub task: Option<Task>,
    /// The role and account its worker is to start with.
    #[serde(default)]
    pub on: On,
    /// The numbers for the Decided records its body holds, in order.
    #[serde(default)]
    pub decided: Vec<u64>,
    /// The owner's urgent mark, from the start (`issue add --urgent`).
    #[serde(default)]
    pub urgent: bool,
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
    let Some(decided) = decided::remarked(&decided::in_body(&p.body), &[], &p.decided) else {
        return no("no_numbers", "a number is needed for each Decided record its body holds");
    };
    let mut events = vec![IssueEvent::IssueOpened {
        checkout: p.checkout.clone(),
        number: p.number,
        title: p.title.trim().to_string(),
        body: p.body.clone(),
        waiting_for_human: p.waiting_for_human,
        why: p.why.clone(),
        task: p.task.clone(),
        on: p.on.clone(),
        decided,
        urgent: p.urgent,
    }];
    let mut opened = Issue::default();
    Issues::evolve(&mut opened, events[0].clone());
    for w in &p.blocked_by {
        for e in opened.wait(w.by.clone(), w.why.as_deref(), false)? {
            Issues::evolve(&mut opened, e.clone());
            events.push(e);
        }
    }
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
    /// Numbers for the body's Decided records that have none yet, in order.
    #[serde(default)]
    pub fresh: Vec<u64>,
}

/// How many of `body`'s Decided records need a number, edited into `issue`.
pub fn unnumbered(issue: &Issue, body: &str) -> usize {
    decided::numbers(&decided::in_body(body), &issue.decided).iter().filter(|n| n.is_none()).count()
}

on_issue!(EditIssue, "issue.edit", Edit, |s, p, now| {
    let title = p.title.clone().unwrap_or_else(|| s.title.clone());
    let body = p.body.clone().unwrap_or_else(|| s.body.clone());
    if title.trim().is_empty() {
        no("no_title", "an issue needs a title")
    } else if title == s.title && body == s.body {
        Ok(Vec::new())
    } else {
        // A record whose head stays keeps its number; one new takes a fresh one.
        let Some(decided) = decided::remarked(&decided::in_body(&body), &s.decided, &p.fresh) else {
            return no("no_numbers", "the body changed since its numbers were given: number it again");
        };
        Ok(vec![IssueEvent::IssueEdited { title, body, decided }])
    }
});

/// Issue `number` of `checkout` names `role` for the workers that start
/// from now, or, `None`, no role.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NameRole {
    pub checkout: String,
    pub number: u64,
    pub role: Option<String>,
}

on_issue!(NameIssueRole, "issue.name_role", NameRole, |s, p, now| {
    only(s.on.role != p.role, IssueEvent::RoleNamed { role: p.role.clone() })
});

/// Issue `number` of `checkout` marked urgent by the owner, or no longer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Urgency {
    pub checkout: String,
    pub number: u64,
    pub urgent: bool,
}

on_issue!(MarkUrgent, "issue.mark_urgent", Urgency, |s, p, now| {
    only(s.urgent != p.urgent, IssueEvent::UrgentMarked { urgent: p.urgent })
});

/// Issue `number` of `checkout` waits for `by`, for `why`, or, with
/// `undo`, stops.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub checkout: String,
    pub number: u64,
    pub by: Blocker,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub undo: bool,
}

on_issue!(BlockIssue, "issue.block", Block, |s, p, now| s.wait(p.by.clone(), p.why.as_deref(), p.undo));

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Because {
    pub checkout: String,
    pub number: u64,
    pub reason: String,
}

// Only a person moves it again, with `issue ready`: for a decision only a
// person can make.
on_issue!(WaitForHuman, "issue.wait_for_human", Because, |s, p, now| {
    only(s.status != Status::WaitingForHuman, IssueEvent::IssueWaitingForHuman { reason: p.reason.clone() })
});

on_issue!(ReadyIssue, "issue.ready", Ref, |s, p, now| {
    only(s.status == Status::WaitingForHuman, IssueEvent::IssueReadied)
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

/// What acts on an issue's worktrees in more than one step: a start (from
/// its take to its worker recorded started, or undone) and the commands
/// that abandon a worktree. One at a time per issue (factorium#189), so
/// none acts on what it read while another changed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Claimant {
    Start,
    StartOver,
    Close,
    Discard,
}

/// An issue's claim: who holds it, and the process it runs in, which,
/// gone, holds it no longer (the next to claim it, or the tick, clears it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub by: Claimant,
    pub pid: u32,
}

impl std::fmt::Display for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is under way on it (process {})", named(self.by), self.pid)
    }
}

/// A worker's take of an issue: its start claims it, in process `pid`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Taking {
    pub checkout: String,
    pub number: u64,
    pub worker: Uuid,
    pub pid: u32,
}

on_issue!(TakeIssue, "issue.take", Taking, |s, p, now| match (s.status, s.worker, &s.claim) {
    (_, _, Some(c)) => no("claimed", c.to_string()),
    (Status::Ready, None, None) => Ok(vec![
        IssueEvent::IssueTaken { worker: p.worker },
        IssueEvent::IssueClaimed { claim: Claim { by: Claimant::Start, pid: p.pid } },
    ]),
    (_, Some(_), _) => no("taken", "a worker holds it already"),
    _ => no("not_ready", format!("it is {}", named(s.status))),
});

/// `claim` of issue `number` of `checkout`, to claim it or to free it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claiming {
    pub checkout: String,
    pub number: u64,
    pub claim: Claim,
}

// Refused while another holds it; a start-over's, while a worker is on it.
on_issue!(ClaimIssue, "issue.claim", Claiming, |s, p, now| match &s.claim {
    Some(c) => no("claimed", c.to_string()),
    None if p.claim.by == Claimant::StartOver && s.worker.is_some() => no(
        "live_worker",
        format!(
            "a worker is on {}#{}: leave it to a person first (factorium issue wait-for-human {} {} --why …)",
            p.checkout, p.number, p.checkout, p.number
        ),
    ),
    None => Ok(vec![IssueEvent::IssueClaimed { claim: p.claim.clone() }]),
});

// Frees the claim, if `claim` holds it still.
on_issue!(UnclaimIssue, "issue.unclaim", Claiming, |s, p, now| {
    only(s.claim.as_ref() == Some(&p.claim), IssueEvent::IssueUnclaimed)
});

on_issue!(ReleaseIssue, "issue.release", Take, |s, p, now| {
    only(s.worker == Some(p.worker), IssueEvent::IssueReleased)
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    pub checkout: String,
    pub number: u64,
    pub text: String,
    pub by: Author,
    /// Its number, if it is a Decided record.
    #[serde(default)]
    pub decided: Option<u64>,
}

on_issue!(CommentIssue, "issue.comment", Comment, |s, p, now| {
    if p.text.trim().is_empty() {
        return no("no_text", "a comment needs text");
    }
    let record = decided::in_comment(&p.text).is_some();
    if record && p.decided.is_none() {
        return no("no_numbers", "a Decided record needs its number");
    }
    let decided = p.decided.filter(|_| record);
    Ok(vec![IssueEvent::IssueCommented { text: p.text.clone(), by: p.by.clone(), at: Some(now.0), decided }])
});

/// Where a Decided record is in its issue: its body, or comment `i`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Body,
    Comment(usize),
}

impl Issue {
    /// Each Decided record it holds, its body's then its comments', with
    /// where it is and its number, if it has one.
    pub fn records(&self) -> Vec<(Place, Option<u64>, decided::Record)> {
        let body = decided::in_body(&self.body);
        let ids = decided::numbers(&body, &self.decided);
        let comments = (self.comments.iter().enumerate())
            .filter_map(|(i, n)| Some((Place::Comment(i), n.decided, decided::in_comment(&n.text)?)));
        body.into_iter().zip(ids).map(|(r, id)| (Place::Body, id, r)).chain(comments).collect()
    }

    /// Numbers its records: `body`'s marks, and comments' by index.
    fn place(&mut self, body: &[Mark], comments: &[(usize, u64)]) {
        self.decided.extend(body.iter().cloned());
        for &(i, id) in comments {
            if let Some(c) = self.comments.get_mut(i) {
                c.decided.get_or_insert(id);
            }
        }
    }

    /// The issue with the numbers `given` placed for its records outside
    /// it: an issue closed before records were numbered takes no event.
    pub fn numbered(mut self, given: &decided::Given) -> Issue {
        let at = (self.checkout.clone(), self.number);
        for n in given.placed.iter().filter(|n| (n.checkout.as_str(), n.number) == (at.0.as_str(), at.1)) {
            self.place(&n.body, &n.comments);
        }
        self
    }
}

// Only what still has no number takes one: a sweep that raced another gives none twice.
on_issue!(NumberDecided, "issue.number_decided", Numbering, |s, p, now| {
    let mut open: Vec<_> = s.records().into_iter().filter(|(_, id, _)| id.is_none()).collect();
    let mut take = |at: Place, m: Option<&Mark>| {
        let i = open.iter().position(|(p, _, r)| *p == at && m.is_none_or(|m| r.marked_by(m)));
        i.map(|i| open.remove(i)).is_some()
    };
    let body: Vec<Mark> = p.body.iter().filter(|m| take(Place::Body, Some(m))).cloned().collect();
    let comments: Vec<_> = p.comments.iter().filter(|(i, _)| take(Place::Comment(*i), None)).copied().collect();
    only(!body.is_empty() || !comments.is_empty(), IssueEvent::DecidedNumbered { body, comments })
});

/// Who left an issue to a person: the watch, a session (the owner, or a
/// session of the owner's), or the one who added it so (`--waiting-for-human`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Asker {
    Watch,
    Session,
    Added,
}

/// When (ms) an issue last started waiting for a person, and who left it
/// so, from its events, each with when it was decided and whether the
/// watch submitted it; none if it does not wait for one now.
pub fn asked(history: impl IntoIterator<Item = (IssueEvent, i64, bool)>) -> Option<(i64, Asker)> {
    let mut last = None;
    for (event, at, by_watch) in history {
        let Some(standing) = event.standing() else { continue };
        last = (standing.status == Status::WaitingForHuman).then_some(match (event, by_watch) {
            (IssueEvent::IssueOpened { .. }, _) => (at, Asker::Added),
            (_, true) => (at, Asker::Watch),
            _ => (at, Asker::Session),
        });
    }
    last
}

/// The question an issue left to a person asks: the first line of its body
/// after any "Based on …" line.
pub fn question(body: &str) -> &str {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.find(|l| !l.starts_with("Based on ")).unwrap_or("")
}
