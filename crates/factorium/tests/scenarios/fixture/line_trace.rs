//! A checkout's line's own records of a run, as a trace of its model
//! (models/Line.tla; `models/line.sh trace` checks one with `truefit model
//! line-run --trace`): each record of a landing, a promotion or a cut, in
//! the order it was recorded, with the line after it as the line's own
//! `apply` makes it, the workers' landing status and the closed issues,
//! proving's tip and the trees whose fast gate went green.
//!
//! The model's steps the code records nothing of are put in where the
//! records say they were taken: the tip read before a gate's record, a
//! gate green already (no record) before a landing's or a promotion's, a
//! push before what is recorded after it. A tick that ends with a gated
//! landing or promotion unpushed had its push answer an error (refused, or
//! lost after it took effect: it took effect when proving's first-parent
//! history holds its tree); one that ends mid-way, with the landing pushed
//! or recorded, died there.
//!
//! A commit is named as the model names it: the workers' slots (by issue
//! number) merged in the order they landed; `[0]` is none.

use std::collections::{HashMap, HashSet};

use factorium_domain::line::{Line, LineEvent};
use serde_json::{Value, json};

use super::World;

/// The environment variable naming where a scenario keeps its line's trace.
const TRACE: &str = "FACTORIUM_LINE_TRACE";

/// What the trace reads of the store: each event's seq, stream, kind and data.
type Row = (i64, String, String, Value);

#[derive(Default)]
struct Trace {
    line: Line,
    /// Each worker's slot, by its id, and each issue's, by its stream: the
    /// issue's number. And each slot's status.
    slots: HashMap<String, usize>,
    issues: HashMap<String, usize>,
    status: Vec<&'static str>,
    asks: Vec<usize>,
    closed: Vec<usize>,
    /// Each landed commit's name, and the last one's.
    names: HashMap<String, Vec<usize>>,
    last: Vec<usize>,
    /// Proving's tip, the trees whose fast gate went green, and the landing
    /// in flight's merge (its tree, when its gate ran), as the model names them.
    proving: Vec<usize>,
    fast: Vec<Vec<usize>>,
    merge: Vec<usize>,
    merge_tree: Option<String>,
    /// Every tree proving's tip ever held, on the remotes.
    proving_trees: HashSet<String>,
    /// Where the landing and the promotion in flight are, as the model's steps.
    land: &'static str,
    prom: &'static str,
    out: Vec<Value>,
}

impl Trace {
    fn name(&self, commit: &str) -> Vec<usize> {
        self.names.get(commit).cloned().unwrap_or_default()
    }

    fn named(&self, tip: Option<&String>) -> Vec<usize> {
        tip.map_or(vec![0], |t| self.name(t))
    }

    /// One step of the model, `action`, with the state as it stands.
    fn step(&mut self, action: &str) {
        let l = &self.line;
        let names =
            |ls: &[factorium_domain::line::Landing]| ls.iter().map(|x| self.name(&x.commit)).collect::<Vec<_>>();
        let state = json!({
            "status": self.status,
            "asks": self.asks,
            "closed": self.closed,
            "unpromoted": names(&l.unpromoted),
            "uncut": names(&l.uncut),
            "promoted": self.named(l.promoted.as_ref()),
            "red": self.named(l.red.as_ref().map(|r| &r.0)),
            "proving": self.proving,
            "fast": self.fast,
        });
        let step = self.out.len() + 1;
        self.out.push(json!({ "step": step, "event": action, "state": state }));
    }

    /// The landing's steps up to `to`, from where it is, none of them recorded.
    fn land_to(&mut self, to: &'static str) {
        let order = ["idle", "merged", "gated", "pushed"];
        let at = |p: &str| order.iter().position(|o| *o == p);
        while let (Some(i), Some(j)) = (at(self.land), at(to))
            && i < j
        {
            match i {
                0 => self.read_merge(),
                2 => self.proving = self.merge.clone(),
                _ => {}
            }
            self.step(["LandRead", "LandGate", "LandPush"][i]);
            self.land = order[i + 1];
        }
    }

    /// Line::landing: the first asking's branch merged onto proving's tip
    /// (proving itself when it holds the branch already).
    fn read_merge(&mut self) {
        let w = self.asks.first().copied().unwrap_or(0);
        self.merge = self.proving.clone();
        if !self.merge.contains(&w) {
            self.merge.push(w);
        }
        self.merge_tree = None;
    }

    fn prom_to(&mut self, to: &'static str) {
        let order = ["idle", "read", "gated", "pushed"];
        let at = |p: &str| order.iter().position(|o| *o == p);
        while let (Some(i), Some(j)) = (at(self.prom), at(to))
            && i < j
        {
            self.step(["PromRead", "PromGate", "PromPush"][i]);
            self.prom = order[i + 1];
        }
    }

    fn slot(&self, stream: &str) -> Option<usize> {
        self.slots.iter().find(|(w, _)| stream.contains(w.as_str())).map(|(_, s)| *s)
    }

    fn line(&mut self, kind: &str, data: &Value) {
        let event: LineEvent = serde_json::from_value(json!({ "type": kind, "data": data })).unwrap();
        self.line.apply(event);
    }

    fn event(&mut self, stream: &str, kind: &str, data: &Value) {
        match kind {
            "LandingAsked" => {
                let Some(w) = self.slot(stream) else { return };
                (self.status[w - 1], self.asks) = ("landing", [&self.asks[..], &[w]].concat());
                self.step("Done");
            }
            "LandingRed" => {
                let Some(w) = self.slot(stream) else { return };
                self.land_to("merged");
                self.status[w - 1] = "working";
                self.asks.retain(|a| *a != w);
                self.step("LandGate");
                self.land = "idle";
            }
            "GateRan" if data["green"] == true && data["kind"] == "fast" => {
                self.land_to("merged");
                self.merge_tree = data["tree"].as_str().map(String::from);
                if !self.fast.contains(&self.merge) {
                    self.fast.push(self.merge.clone());
                }
                self.land_to("gated");
            }
            "GateRan" if data["green"] == true => self.prom_to("gated"),
            "GateRan" if data["kind"] == "fast" => self.land_to("merged"),
            "GateRan" => self.prom_to("read"),
            "Landed" => {
                let commit = data["commit"].as_str().unwrap().to_string();
                let slot = data["number"].as_u64().unwrap() as usize;
                if !self.names.contains_key(&commit) {
                    self.last.push(slot);
                    self.names.insert(commit, self.last.clone());
                }
                self.land_to("pushed");
                self.line(kind, data);
                self.step("LandRecord");
                self.land = "recorded";
            }
            "WorkerLanded" => {
                let Some(w) = self.slot(stream) else { return };
                self.status[w - 1] = "landed";
                self.asks.retain(|a| *a != w);
                self.step("LandEnd");
                self.land = "ended";
            }
            "IssueClosed" if self.land == "ended" => {
                self.closed.extend(self.issues.get(stream));
                self.step("LandClose");
                self.land = "idle";
            }
            "PromotionRed" => {
                // Red at its gate, or at its push: the base moved outside factorium.
                let gated = self.prom == "gated";
                self.prom_to("read");
                self.line(kind, data);
                self.step(if gated { "PromPush" } else { "PromGate" });
                self.prom = "idle";
            }
            "Promoted" => {
                self.prom_to("pushed");
                self.line(kind, data);
                self.step("PromRecord");
                self.prom = "idle";
            }
            "CutMade" => {
                self.step("CutRead");
                self.line(kind, data);
                self.step("CutMake");
            }
            _ => {}
        }
    }

    /// A tick ended: a push it did not record answered an error; a landing
    /// left mid-way died there.
    fn tick_ended(&mut self) {
        if self.land == "gated" {
            // Lost after it took effect, or refused (a tree green already, of no record, is refused).
            if self.merge_tree.as_ref().is_some_and(|t| self.proving_trees.contains(t)) {
                self.proving = self.merge.clone();
            }
            self.step("LandPush");
            self.land = "idle";
        }
        if self.prom == "gated" {
            self.step("PromPush");
            self.prom = "idle";
        }
        if self.land != "idle" || self.prom != "idle" {
            self.step("Die");
            (self.land, self.prom) = ("idle", "idle");
        }
    }
}

impl World {
    /// The `seq` of the last event recorded.
    pub fn last_seq(&self) -> i64 {
        self.sql().query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0)).unwrap()
    }

    /// The line's records so far, one JSON line each.
    pub fn line_trace(&self) -> String {
        let db = self.sql();
        let mut rows = db.prepare("SELECT seq, stream, kind, data FROM events ORDER BY seq").unwrap();
        let rows: Vec<Row> = rows
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .map(|(seq, stream, kind, data)| (seq, stream, kind, serde_json::from_str(&data).unwrap()))
            .collect();
        // Each worker's slot: the number of the issue it took.
        let mut t = Trace { land: "idle", prom: "idle", proving_trees: self.proving_trees(), ..Trace::default() };
        for (_, stream, _, data) in rows.iter().filter(|r| r.2 == "IssueOpened") {
            t.issues.insert(stream.clone(), data["number"].as_u64().unwrap() as usize);
        }
        for (_, stream, _, data) in rows.iter().filter(|r| r.2 == "IssueTaken") {
            t.slots.insert(data["worker"].as_str().unwrap().into(), t.issues[stream]);
        }
        t.status = vec!["working"; t.issues.len()];
        let ticks = self.ticks.lock().unwrap().clone();
        let mut ends = ticks.iter().peekable();
        for (seq, stream, kind, data) in &rows {
            while ends.next_if(|e| **e < *seq).is_some() {
                t.tick_ended();
            }
            t.event(stream, kind, data);
        }
        if ends.next().is_some() {
            t.tick_ended();
        }
        t.out.iter().map(|r| r.to_string() + "\n").collect()
    }

    /// Every tree proving's tip held, on each remote: its first-parent history.
    fn proving_trees(&self) -> HashSet<String> {
        let remotes = std::fs::read_dir(self.dir.join("remotes")).into_iter().flatten().flatten();
        remotes
            .filter_map(|r| self.git(&r.path(), &["log", "--first-parent", "--format=%T", "proving"]).ok())
            .flat_map(|out| out.lines().map(String::from).collect::<Vec<_>>())
            .collect()
    }

    /// Writes the run's line trace where [`TRACE`] names, if it does.
    pub fn keep_line_trace(&self) {
        if let Ok(path) = std::env::var(TRACE) {
            std::fs::write(path, self.line_trace()).unwrap();
        }
    }
}
