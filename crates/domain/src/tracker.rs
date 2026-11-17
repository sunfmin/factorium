//! The tracker: every checkout and issue as rows the CLI lists and queries.
//! A row's key is its stream's id, so every event of the stream finds it;
//! the name and the number are columns, queried like any other.

use factum_core::{Event, Projection, RowOp, Table};
use serde_json::json;

pub const TRACKER: &str = "tracker";
pub const CHECKOUTS: &str = "checkouts";
pub const ISSUES: &str = "issues";

pub struct Tracker;

impl Projection for Tracker {
    fn name(&self) -> &str {
        TRACKER
    }

    fn version(&self) -> u32 {
        1
    }

    fn tables(&self) -> Vec<Table> {
        vec![
            Table::new(CHECKOUTS, &["name", "listed"]).index(&["name"]),
            Table::new(ISSUES, &["checkout", "number", "status"])
                .index(&["checkout", "status", "number"]),
        ]
    }

    fn project(&self, e: &Event) -> Result<Vec<RowOp>, String> {
        let (d, key) = (&e.data, e.stream.id.to_string());
        let put = |table: &str, row| {
            vec![RowOp::Put {
                table: table.into(),
                key: key.clone(),
                row,
            }]
        };
        let merge = |table: &str, fields| {
            vec![RowOp::Merge {
                table: table.into(),
                key: key.clone(),
                fields,
            }]
        };
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
                }),
            ),
            ("issue", "IssueEdited") => merge(ISSUES, json!({ "title": d["title"] })),
            ("issue", "IssueParked") => {
                merge(ISSUES, json!({ "status": "parked", "reason": d["reason"] }))
            }
            ("issue", "IssueReadied") => {
                merge(ISSUES, json!({ "status": "ready", "reason": null }))
            }
            ("issue", "IssueClosed") => {
                merge(ISSUES, json!({ "status": "closed", "reason": d["reason"] }))
            }
            _ => Vec::new(),
        })
    }
}
