//! The heavy queue's own records of a run, as a trace of its model
//! (models/HeavyQueue.tla; `models/heavy.sh trace` checks one with
//! `truefit model heavy-run --trace`): each event of the queue's stream, in
//! the order it was recorded, with the queue after it as the stream's own
//! `evolve` makes it. Times are ms from the first record; an entry's id is
//! its slot, by the order of the ids (which breaks a tie of `queued_at`);
//! rules are ranked oldest first, 0 for none.

use factorium_domain::heavy::{Entry, Heavy, Queue, older};
use factorium_domain::neutral::queue::Queued;
use factum::Aggregate;
use serde_json::{Value, json};

use super::World;

/// The environment variable naming where a scenario keeps its run's trace.
const TRACE: &str = "FACTORIUM_HEAVY_TRACE";

impl World {
    /// The queue's records so far, one JSON line each.
    pub fn heavy_trace(&self) -> String {
        let db = self.sql();
        let mut rows = db.prepare("SELECT kind, data, meta FROM events WHERE kind LIKE 'Heavy%' ORDER BY seq").unwrap();
        let rows: Vec<(String, Value, i64)> = rows
            .query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .map(|(kind, data, meta)| {
                let now = serde_json::from_str::<Value>(&meta).unwrap()["now"].as_i64().unwrap();
                (kind, serde_json::from_str(&data).unwrap(), now)
            })
            .collect();
        let mut ids: Vec<String> = rows.iter().filter_map(|(_, d, _)| d["id"].as_str().map(String::from)).collect();
        ids.sort_by_key(|id| id.parse::<factum::Uuid>().unwrap());
        ids.dedup();
        let mut versions: Vec<String> =
            rows.iter().filter_map(|(_, d, _)| d["version"].as_str().map(String::from)).collect();
        versions.sort_by(|a, b| match (older(a, b), older(b, a)) {
            (true, _) => std::cmp::Ordering::Less,
            (_, true) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
        versions.dedup();
        let t0 = rows.first().map_or(0, |r| r.2);
        let mut q = Queue::default();
        let mut out = String::new();
        for (step, (kind, data, now)) in rows.into_iter().enumerate() {
            Heavy::evolve(&mut q, serde_json::from_value(json!({ "type": kind, "data": data })).unwrap());
            let rel = |t: i64| t - t0;
            let slot = |id: &String| {
                let e = q.entries.iter().find(|e| &e.id.to_string() == id);
                e.map_or_else(absent, |e| present(e, rel))
            };
            let rules = versions.iter().position(|v| *v == q.rules).map_or(0, |p| p + 1);
            let state = json!({
                "q": ids.iter().map(slot).collect::<Vec<_>>(),
                // Never started: the code's 0, the model's -1.
                "last_start": if q.last_start == 0 { -1 } else { rel(q.last_start) },
                "rules": rules,
            });
            out += &json!({ "step": step + 1, "event": kind, "now": rel(now), "state": state }).to_string();
            out.push('\n');
        }
        out
    }

    /// Writes the run's trace where [`TRACE`] names, if it does: each world
    /// does as it ends, passed or failed, so a run of one scenario keeps its own.
    pub fn keep_heavy_trace(&self) {
        if let Ok(path) = std::env::var(TRACE) {
            std::fs::write(path, self.heavy_trace()).unwrap();
        }
    }
}

fn absent() -> Value {
    json!({ "in": false, "queued_at": -1, "started_at": -1, "yields": false, "bound_at": -1, "noted": false })
}

fn present(e: &Entry, rel: impl Fn(i64) -> i64) -> Value {
    json!({
        "in": true,
        "queued_at": rel(e.queued_at),
        "started_at": e.started_at.map_or(-1, &rel),
        "yields": e.yields(),
        "bound_at": e.bound_at.map_or(-1, &rel),
        "noted": e.session.is_some(),
    })
}
