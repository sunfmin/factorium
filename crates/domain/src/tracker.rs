//! The tracker: every checkout, issue and worker as rows the CLI lists and
//! queries. A row's key is its stream's id, so every event of the stream
//! finds it; names and numbers are columns, queried like any other.

use factum_core::{Event, Projection, RowOp, Table};
use serde_json::{Value, json};

pub const TRACKER: &str = "tracker";
pub const CHECKOUTS: &str = "checkouts";
pub const ISSUES: &str = "issues";
pub const WORKERS: &str = "workers";

pub struct Tracker;

impl Projection for Tracker {
    fn name(&self) -> &str {
        TRACKER
    }

    fn version(&self) -> u32 {
        3
    }

    fn tables(&self) -> Vec<Table> {
        vec![
            Table::new(CHECKOUTS, &["name", "listed"]).index(&["name"]),
            Table::new(ISSUES, &["checkout", "number", "status", "taken"]).index(&["checkout", "status", "number"]),
            Table::new(WORKERS, &["checkout", "number", "status", "since", "started_at"]).index(&["status"]),
        ]
    }

    fn project(&self, e: &Event) -> Result<Vec<RowOp>, String> {
        let (d, key) = (&e.data, e.stream.id.to_string());
        let put = |table: &str, row| vec![RowOp::Put { table: table.into(), key: key.clone(), row }];
        let merge = |table: &str, fields| vec![RowOp::Merge { table: table.into(), key: key.clone(), fields }];
        let status = |s: &str, why: &Value| merge(WORKERS, json!({ "status": s, "why": why, "since": d["at"] }));
        Ok(match (e.stream.aggregate.as_str(), e.kind.as_str()) {
            ("checkout", "CheckoutAdded") => put(
                CHECKOUTS,
                json!({ "name": d["name"], "path": d["path"], "base": d["base"], "proving": d["proving"], "listed": true }),
            ),
            ("checkout", "CheckoutRemoved") => merge(CHECKOUTS, json!({ "listed": false })),
            ("issue", "IssueOpened") => put(
                ISSUES,
                json!({
                    "checkout": d["checkout"],
                    "number": d["number"],
                    "title": d["title"],
                    "status": if d["parked"] == true { "parked" } else { "ready" },
                    "taken": false,
                }),
            ),
            ("issue", "IssueEdited") => merge(ISSUES, json!({ "title": d["title"] })),
            ("issue", "IssueParked") => merge(ISSUES, json!({ "status": "parked", "reason": d["reason"] })),
            ("issue", "IssueReadied") => merge(ISSUES, json!({ "status": "ready", "reason": null })),
            ("issue", "IssueClosed") => {
                merge(ISSUES, json!({ "status": "closed", "reason": d["reason"], "taken": false, "worker": null }))
            }
            ("issue", "IssueTaken") => merge(ISSUES, json!({ "taken": true, "worker": d["worker"] })),
            ("issue", "IssueReleased") => merge(ISSUES, json!({ "taken": false, "worker": null })),
            ("worker", "WorkerStarted") => put(
                WORKERS,
                json!({
                    "worker": key, "checkout": d["checkout"], "number": d["number"], "attempt": d["attempt"],
                    "harness": d["harness"], "account": d["account"], "terminal": d["terminal"],
                    "worktree": d["worktree"], "branch": d["branch"], "status": "working", "since": d["at"],
                    "started_at": d["at"],
                }),
            ),
            ("worker", "LandingAsked") => status("landing", &Value::Null),
            ("worker", "LandingRed") => status("working", &d["why"]),
            ("worker", "WorkerLanded") => status("ended", &json!("landed")),
            ("worker", "WorkerWaits") => status("waiting", &d["why"]),
            ("worker", "WorkerResumed") => status("working", &Value::Null),
            ("worker", "WorkerEnded") => status("ended", &d["why"]),
            _ => Vec::new(),
        })
    }
}
