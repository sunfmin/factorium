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

use crate::checkout::{Add, Checkout, CheckoutEvent};
use crate::heavy::{HeavyEvent, Outcome};
use crate::issue::{IssueEvent, Standing, Status as IssueStatus};
use crate::worker::{self, Status as WorkerStatus, WorkerEvent};

pub const TRACKER: &str = "tracker";
const CHECKOUTS: &str = "checkouts";
const ISSUES: &str = "issues";
const WORKERS: &str = "workers";
const RUNS: &str = "heavy_runs";

/// A checkout's row is the checkout.
pub type CheckoutRow = Checkout;

/// An issue's row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueRow {
    pub checkout: String,
    pub number: u64,
    pub title: String,
    /// When it became ready (ms): filed, or readied from waiting for a
    /// person, whichever is later; the log's time of that event.
    #[serde(default)]
    pub ready_at: i64,
    #[serde(flatten)]
    pub standing: Standing,
    #[serde(flatten)]
    pub holder: Holder,
    /// The owner's urgent mark.
    #[serde(default)]
    pub urgent: bool,
    /// It starts ahead of the ordinary ones, behind the urgent: a promotion's
    /// mutants issue (`crate::mutants::ahead`).
    #[serde(default)]
    pub ahead: bool,
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
    pub state: worker::Standing,
    /// Its terminal was closed and Orca no longer lists it.
    #[serde(default)]
    pub closed: bool,
    /// Why closing its terminal failed, the last time it was tried.
    #[serde(default)]
    pub close_failed: Option<String>,
}

/// A heavy command's row, by its entry: when it was asked (the place it
/// took: one kept from its asker's entry that left unstarted is that one's
/// ask), granted and ended, ms, and how its steps ran (factorium#308).
/// Asked again under a newer binary's rules, its ask is that one's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRow {
    pub entry: Uuid,
    pub what: String,
    /// The worker it is of: its own command's, or the one whose landing it
    /// gates; the column it is queried by.
    pub of: Option<Uuid>,
    /// It gates `of`'s landing.
    #[serde(default)]
    pub lands: bool,
    /// A hold across commands (`worker hold`), not a command.
    #[serde(default)]
    pub hold: bool,
    pub asked_at: i64,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub outcome: Option<Outcome>,
}

/// Every heavy command of `worker`'s, and each gate of its landings, as asked.
pub fn runs_of(worker: Uuid) -> Rows<RunRow> {
    Rows::of(Query::table(RUNS).filter("of", Op::Eq, worker.to_string()).order_by("asked_at", false))
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
        q = q.filter("status", Op::Ne, json!(IssueStatus::Closed));
    }
    Rows::of(q.order_by("checkout", false).order_by("number", false))
}

/// The open issues a worker holds, by checkout and number.
pub fn held() -> Rows<IssueRow> {
    let q = Query::table(ISSUES).filter("status", Op::Ne, json!(IssueStatus::Closed)).filter("taken", Op::Eq, true);
    Rows::of(q.order_by("checkout", false).order_by("number", false))
}

/// The ready issues no worker holds, by checkout and number; the order
/// they start in is [`crate::next::order`]'s.
pub fn untaken() -> Rows<IssueRow> {
    let q = Query::table(ISSUES).filter("status", Op::Eq, json!(IssueStatus::Ready)).filter("taken", Op::Eq, false);
    Rows::of(q.order_by("checkout", false).order_by("number", false))
}

/// The issues of every checkout that wait for a person.
pub fn asking() -> Rows<IssueRow> {
    Rows::of(Query::table(ISSUES).filter("status", Op::Eq, json!(IssueStatus::WaitingForHuman)))
}

/// The last-numbered issue of `checkout`.
pub fn last_issue(checkout: &str) -> Rows<IssueRow> {
    Rows::of(Query::table(ISSUES).filter("checkout", Op::Eq, checkout).order_by("number", true).limit(1))
}

/// The workers in `status`, or the live ones, or with `all` every one;
/// oldest status first.
pub fn workers(status: Option<WorkerStatus>, all: bool) -> Rows<WorkerRow> {
    let q = match status {
        Some(s) => Query::table(WORKERS).filter("status", Op::Eq, json!(s)),
        None if all => Query::table(WORKERS),
        None => Query::table(WORKERS).filter("status", Op::Ne, json!(WorkerStatus::Ended)),
    };
    Rows::of(q.order_by("since", false))
}

pub struct Tracker;

/// An event as its aggregate's own type.
pub fn decode<E: DeserializeOwned>(e: &Event) -> Result<E, String> {
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

    /// "6": rows rebuilt from the log, so a row kept with a status of an
    /// older name (`parked`, before factorium#95) holds the current one.
    /// "7": the heavy runs' rows (factorium#308), built from the log.
    /// "8": an issue's row holds its urgent mark.
    /// "9": and whether it starts ahead of the ordinary ones.
    /// "10": each issue's row keeps when it became ready, from the log.
    fn version(&self) -> u32 {
        10
    }

    fn tables(&self) -> Vec<Table> {
        vec![
            Table::new(CHECKOUTS, &["name", "listed"]).index(&["name"]),
            Table::new(ISSUES, &["checkout", "number", "status", "taken"]).index(&["checkout", "status", "number"]),
            Table::new(WORKERS, &["checkout", "number", "status", "since", "started_at"]).index(&["status"]),
            Table::new(RUNS, &["of", "asked_at"]).index(&["of"]),
        ]
    }

    fn project(&self, e: &Event) -> Result<Vec<RowOp>, String> {
        let key = e.stream.id;
        let put = |table: &str, row: Value| vec![RowOp::Put { table: table.into(), key: key.to_string(), row }];
        let merge =
            |table: &str, fields: Value| vec![RowOp::Merge { table: table.into(), key: key.to_string(), fields }];
        Ok(match e.stream.aggregate.as_str() {
            "checkout" => match decode(e)? {
                CheckoutEvent::CheckoutAdded(Add { name, path, base, proving, role }) => {
                    put(CHECKOUTS, row(Checkout { name, path, base, proving, listed: true, role, idle_off: false }))
                }
                CheckoutEvent::CheckoutRemoved => merge(CHECKOUTS, json!({ "listed": false })),
                CheckoutEvent::RoleNamed { role } => merge(CHECKOUTS, json!({ "role": role })),
                CheckoutEvent::IdleTurned { off } => merge(CHECKOUTS, json!({ "idle_off": off })),
            },
            "issue" => {
                let event: IssueEvent = decode(e)?;
                let mut standing = event.standing();
                let mut ops = match event {
                    IssueEvent::IssueOpened { checkout, number, title, urgent, task, .. } => {
                        let (standing, holder) = (standing.take().unwrap_or_default(), Holder::of(None));
                        let ahead = crate::mutants::ahead(task.as_ref());
                        let ready_at = e.meta.now.0;
                        put(
                            ISSUES,
                            row(IssueRow { checkout, number, title, ready_at, standing, holder, urgent, ahead }),
                        )
                    }
                    IssueEvent::UrgentMarked { urgent } => merge(ISSUES, json!({ "urgent": urgent })),
                    IssueEvent::IssueEdited { title, .. } => merge(ISSUES, json!({ "title": title })),
                    IssueEvent::IssueReadied => merge(ISSUES, json!({ "ready_at": e.meta.now.0 })),
                    IssueEvent::IssueTaken { worker } => merge(ISSUES, row(Holder::of(Some(worker)))),
                    IssueEvent::IssueReleased | IssueEvent::IssueClosed { .. } => merge(ISSUES, row(Holder::of(None))),
                    _ => Vec::new(),
                };
                ops.extend(standing.into_iter().flat_map(|st| merge(ISSUES, row(st))));
                ops
            }
            "worker" => {
                let event: WorkerEvent = decode(e)?;
                match (event.standing(), event) {
                    (Some(state), WorkerEvent::WorkerStarted(s)) => put(
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
                            state,
                            closed: false,
                            close_failed: None,
                        }),
                    ),
                    (Some(state), _) => merge(WORKERS, row(state)),
                    (None, WorkerEvent::SessionClosed { failed }) => {
                        merge(WORKERS, json!({ "closed": failed.is_none(), "close_failed": failed }))
                    }
                    (None, _) => Vec::new(),
                }
            }
            "heavy" => {
                let at = e.meta.now.0;
                let merge_run = |entry: Uuid, fields: Value| {
                    vec![RowOp::Merge { table: RUNS.into(), key: entry.to_string(), fields }]
                };
                match decode(e)? {
                    HeavyEvent::HeavyQueued { ask, at, place } => {
                        let of = ask.worker.or(ask.lands);
                        merge_run(
                            ask.id,
                            json!({ "entry": ask.id, "what": ask.what, "of": of, "lands": ask.lands.is_some(),
                                "hold": ask.hold_for.is_some(), "asked_at": place.unwrap_or(at) }),
                        )
                    }
                    HeavyEvent::HeavyStarted { id, at } => merge_run(id, json!({ "started_at": at })),
                    HeavyEvent::HeavyEnded { id, outcome, .. } => {
                        merge_run(id, json!({ "ended_at": at, "outcome": outcome }))
                    }
                    HeavyEvent::HeavyRuns { .. }
                    | HeavyEvent::HeavyRules { .. }
                    | HeavyEvent::HeavyMarked { .. }
                    | HeavyEvent::HeavyHalted { .. } => Vec::new(),
                }
            }
            _ => Vec::new(),
        })
    }
}
