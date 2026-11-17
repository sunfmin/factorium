//! The tracker: every checkout, issue and worker as rows the CLI lists and
//! queries. A row's key is its stream's id, so every event of the stream
//! finds it; names and numbers are columns, queried like any other.
//!
//! The rows' shapes live here and nowhere else: the projection writes them
//! from each event decoded as its aggregate's own type, and the app reads
//! them back as the same types through the selections below.

use std::marker::PhantomData;

use factum_core::{Event, Op, Projection, Query, RowOp, Table, Uuid};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::checkout::{Checkout, CheckoutEvent};
use crate::issue::{IssueEvent, Status as IssueStatus};
use crate::worker::{Status as WorkerStatus, WorkerEvent};

pub const TRACKER: &str = "tracker";
const CHECKOUTS: &str = "checkouts";
const ISSUES: &str = "issues";
const WORKERS: &str = "workers";

/// A checkout's row is the checkout.
pub type CheckoutRow = Checkout;

/// An issue's row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueRow {
    pub checkout: String,
    pub number: u64,
    pub title: String,
    #[serde(flatten)]
    pub standing: IssueStanding,
    #[serde(flatten)]
    pub holder: Holder,
}

/// Where an issue stands, and why it is parked or closed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueStanding {
    pub status: IssueStatus,
    pub reason: Option<String>,
}

/// The worker that holds an issue; `taken` is its queryable column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Holder {
    pub taken: bool,
    pub worker: Option<Uuid>,
}

impl Holder {
    fn of(worker: Option<Uuid>) -> Holder {
        Holder { taken: worker.is_some(), worker }
    }
}

/// A worker's row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerRow {
    pub worker: Uuid,
    pub checkout: String,
    pub number: u64,
    pub attempt: u32,
    pub harness: String,
    pub account: Option<String>,
    pub terminal: String,
    pub worktree: String,
    pub branch: String,
    /// When it started, in ms.
    pub started_at: i64,
    #[serde(flatten)]
    pub state: WorkerState,
    /// Its terminal was closed and Orca no longer lists it.
    #[serde(default)]
    pub closed: bool,
    /// Why closing its terminal failed, the last time it was tried.
    #[serde(default)]
    pub close_failed: Option<String>,
}

/// A worker's status, why, and since when, in ms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerState {
    pub status: WorkerStatus,
    pub why: Option<String>,
    pub since: i64,
}

/// What the CLI prints of a row: its columns kept only for queries left out.
pub fn shown<R: Serialize>(rows: &[R]) -> Value {
    let mut v = json!(rows);
    for row in v.as_array_mut().into_iter().flatten().filter_map(Value::as_object_mut) {
        row.remove("listed");
        row.remove("taken");
    }
    v
}

/// A query of the tracker whose rows are `R`s.
pub struct Rows<R> {
    pub query: Query,
    row: PhantomData<R>,
}

impl<R: DeserializeOwned> Rows<R> {
    fn of(query: Query) -> Rows<R> {
        Rows { query, row: PhantomData }
    }

    /// The rows the query found, or why one is not an `R`.
    pub fn read(found: Vec<Value>) -> Result<Vec<R>, String> {
        found.into_iter().map(|r| serde_json::from_value(r).map_err(|e| format!("a tracker row: {e}"))).collect()
    }
}

/// The checkouts on the list, by name.
pub fn listed() -> Rows<CheckoutRow> {
    Rows::of(Query::table(CHECKOUTS).filter("listed", Op::Eq, true).order_by("name", false))
}

/// The issues of `checkout`, or of every checkout, open ones unless
/// `closed`; by checkout and number.
pub fn issues(checkout: Option<&str>, closed: bool) -> Rows<IssueRow> {
    let mut q = Query::table(ISSUES);
    if let Some(c) = checkout {
        q = q.filter("checkout", Op::Eq, c);
    }
    if !closed {
        q = q.filter("status", Op::Ne, IssueStatus::Closed);
    }
    Rows::of(q.order_by("checkout", false).order_by("number", false))
}

/// The ready issues no worker holds, by checkout and number.
pub fn untaken() -> Rows<IssueRow> {
    let q = Query::table(ISSUES).filter("status", Op::Eq, IssueStatus::Ready).filter("taken", Op::Eq, false);
    Rows::of(q.order_by("checkout", false).order_by("number", false))
}

/// The last-numbered issue of `checkout`.
pub fn last_issue(checkout: &str) -> Rows<IssueRow> {
    Rows::of(Query::table(ISSUES).filter("checkout", Op::Eq, checkout).order_by("number", true).limit(1))
}

/// The workers in `status`, or the live ones, or with `all` every one;
/// oldest status first.
pub fn workers(status: Option<WorkerStatus>, all: bool) -> Rows<WorkerRow> {
    let q = match status {
        Some(s) => Query::table(WORKERS).filter("status", Op::Eq, s),
        None if all => Query::table(WORKERS),
        None => Query::table(WORKERS).filter("status", Op::Ne, WorkerStatus::Ended),
    };
    Rows::of(q.order_by("since", false))
}

pub struct Tracker;

/// An event as its aggregate's own type.
fn decode<E: DeserializeOwned>(e: &Event) -> Result<E, String> {
    let tagged = if e.data.is_null() { json!({ "type": e.kind }) } else { json!({ "type": e.kind, "data": e.data }) };
    serde_json::from_value(tagged).map_err(|err| format!("{}: {err}", e.kind))
}

fn row(of: impl Serialize) -> Value {
    serde_json::to_value(of).unwrap_or(Value::Null)
}

impl Projection for Tracker {
    fn name(&self) -> &str {
        TRACKER
    }

    fn version(&self) -> u32 {
        5
    }

    fn tables(&self) -> Vec<Table> {
        vec![
            Table::new(CHECKOUTS, &["name", "listed"]).index(&["name"]),
            Table::new(ISSUES, &["checkout", "number", "status", "taken"]).index(&["checkout", "status", "number"]),
            Table::new(WORKERS, &["checkout", "number", "status", "since", "started_at"]).index(&["status"]),
        ]
    }

    fn project(&self, e: &Event) -> Result<Vec<RowOp>, String> {
        let key = e.stream.id;
        let put = |table: &str, row: Value| vec![RowOp::Put { table: table.into(), key: key.to_string(), row }];
        let merge =
            |table: &str, fields: Value| vec![RowOp::Merge { table: table.into(), key: key.to_string(), fields }];
        let standing = |status, reason| merge(ISSUES, row(IssueStanding { status, reason }));
        let state = |status, why, since| merge(WORKERS, row(WorkerState { status, why, since }));
        Ok(match e.stream.aggregate.as_str() {
            "checkout" => match decode(e)? {
                CheckoutEvent::CheckoutAdded { name, path, base, proving } => {
                    put(CHECKOUTS, row(Checkout { name, path, base, proving, listed: true }))
                }
                CheckoutEvent::CheckoutRemoved => merge(CHECKOUTS, json!({ "listed": false })),
            },
            "issue" => match decode(e)? {
                IssueEvent::IssueOpened { checkout, number, title, parked, .. } => {
                    let status = if parked { IssueStatus::Parked } else { IssueStatus::Ready };
                    let standing = IssueStanding { status, reason: None };
                    put(ISSUES, row(IssueRow { checkout, number, title, standing, holder: Holder::of(None) }))
                }
                IssueEvent::IssueEdited { title, .. } => merge(ISSUES, json!({ "title": title })),
                IssueEvent::IssueParked { reason } => standing(IssueStatus::Parked, Some(reason)),
                IssueEvent::IssueReadied => standing(IssueStatus::Ready, None),
                IssueEvent::IssueClosed { reason } => {
                    let mut ops = standing(IssueStatus::Closed, Some(reason));
                    ops.extend(merge(ISSUES, row(Holder::of(None))));
                    ops
                }
                IssueEvent::IssueTaken { worker } => merge(ISSUES, row(Holder::of(Some(worker)))),
                IssueEvent::IssueReleased => merge(ISSUES, row(Holder::of(None))),
                IssueEvent::BlockedBy { .. } | IssueEvent::Unblocked { .. } | IssueEvent::IssueCommented { .. } => {
                    Vec::new()
                }
            },
            "worker" => match decode(e)? {
                WorkerEvent::WorkerStarted(s) => put(
                    WORKERS,
                    row(WorkerRow {
                        worker: key,
                        checkout: s.checkout,
                        number: s.number,
                        attempt: s.attempt,
                        harness: s.harness,
                        account: s.account,
                        terminal: s.terminal,
                        worktree: s.worktree,
                        branch: s.branch,
                        started_at: s.at,
                        state: WorkerState { status: WorkerStatus::Working, why: None, since: s.at },
                        closed: false,
                        close_failed: None,
                    }),
                ),
                WorkerEvent::LandingAsked { at } => state(WorkerStatus::Landing, None, at),
                WorkerEvent::LandingRed { why, at } => state(WorkerStatus::Working, Some(why), at),
                WorkerEvent::WorkerLanded { at, .. } => state(WorkerStatus::Ended, Some("landed".into()), at),
                WorkerEvent::WorkerWaits { why, at } => state(WorkerStatus::Waiting, Some(why), at),
                WorkerEvent::WorkerResumed { at } => state(WorkerStatus::Working, None, at),
                WorkerEvent::WorkerEnded { why, at } => state(WorkerStatus::Ended, Some(why), at),
                WorkerEvent::SessionClosed { failed } => {
                    merge(WORKERS, json!({ "closed": failed.is_none(), "close_failed": failed }))
                }
                WorkerEvent::WentOver { .. }
                | WorkerEvent::CameUnder
                | WorkerEvent::BriefHanded
                | WorkerEvent::RedTold { .. }
                | WorkerEvent::WentQuiet { .. }
                | WorkerEvent::Stirred
                | WorkerEvent::WorkerHeard { .. } => Vec::new(),
            },
            _ => Vec::new(),
        })
    }
}
