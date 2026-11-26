//! A checkout's line's own records of a run, as a trace of its model
//! (models/Line.tla; `models/line.sh trace` checks one with `truefit model
//! line-run --trace`): each record of a landing, its stack, a promotion or a
//! cut, in the order it was recorded, with the line after it as the line's
//! own `apply` makes it, the workers' landing status and the closed issues,
//! proving's tip and the commits whose fast gate went green.
//!
//! The model's steps the code records nothing of are put in where the
//! records say they were taken: a push before what is recorded after it
//! (the first landing it records, or the stack let go when it answered an
//! error: refused, or lost after it took effect, when proving's
//! first-parent history holds the commit it pushed), a promotion's read and
//! gate before its record. A tick that ends in the middle of a push's
//! records or of a red stopped there (`Fail`).
//!
//! A commit is named as the model names it: the workers' slots (by issue
//! number) merged in the order they stacked, a 0 after a merge whose gate
//! amended regenerated records into it; `[0]` is none.

use std::collections::{HashMap, HashSet};

use factorium_domain::line::{Line, LineEvent};
use serde_json::{Value, json};

use super::World;

/// The environment variable naming where a scenario keeps its line's trace.
const TRACE: &str = "FACTORIUM_LINE_TRACE";

/// What the trace reads of the store: each event's seq, stream, kind and data.
type Row = (i64, String, String, Value);

/// A layer of the stack, as the model names it.
#[derive(Clone)]
struct Layer {
    slot: usize,
    on: Vec<usize>,
    merged: Vec<usize>,
    /// Once its gate is green: the commit it lands as, and its name.
    green: Option<(String, Vec<usize>)>,
    red: bool,
}

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
    /// Each commit's name.
    names: HashMap<String, Vec<usize>>,
    /// Proving's tip and the commits whose fast gate went green, as the
    /// model names them; and every commit proving's tip ever held, on the remotes.
    proving: Vec<usize>,
    fast: Vec<Vec<usize>>,
    proving_commits: HashSet<String>,
    stack: Vec<Layer>,
    /// The daemon's push in flight (or its red), as the model's steps: the
    /// layers it pushed, and the one being recorded.
    push: &'static str,
    pushed: Vec<Layer>,
    at: usize,
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

    /// Stack::landable: from the bottom, each layer whose gate is green.
    fn landable(&self) -> usize {
        self.stack.iter().take_while(|l| l.green.is_some()).count()
    }

    fn layer_of(&self, data: &Value) -> Option<usize> {
        let slot = self.slots.get(data["worker"].as_str()?)?;
        self.stack.iter().position(|l| l.slot == *slot)
    }

    /// The push's steps up to `to`, from where it is, none of them recorded:
    /// the push itself, of what is landable, and the record of a landing
    /// held already.
    fn push_to(&mut self, to: &'static str) {
        if self.push == "idle" && to != "idle" {
            self.pushed = self.stack[..self.landable()].to_vec();
            self.at = 0;
            self.proving = self.pushed.last().and_then(|l| l.green.clone()).map(|g| g.1).unwrap_or_default();
            self.step("StackPush");
            self.push = "pushed";
        }
        if self.push == "pushed" && to == "recorded" {
            self.step("LandRecord");
            self.push = "recorded";
        }
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
            "Stacked" => self.stacked(data),
            "LayerGreen" | "LayerRed" => {
                let Some(i) = self.layer_of(data) else { return };
                if kind == "LayerRed" {
                    self.stack[i].red = true;
                } else {
                    let commit = data["commit"].as_str().unwrap().to_string();
                    let l = &self.stack[i];
                    let gated = !self.fast.contains(&l.merged);
                    let mut name = l.merged.clone();
                    if self.names.get(&commit) != Some(&l.merged) {
                        name.push(0);
                    }
                    if gated && !self.fast.contains(&name) {
                        self.fast.push(name.clone());
                    }
                    self.names.insert(commit.clone(), name.clone());
                    self.stack[i].green = Some((commit, name));
                }
                self.step("StackGate");
            }
            "Unstacked" => self.unstacked(data),
            "Landed" => {
                self.push_to("pushed");
                self.line(kind, data);
                self.step("LandRecord");
                self.push = "recorded";
            }
            "WorkerLanded" => {
                let Some(w) = self.slot(stream) else { return };
                self.push_to("recorded");
                self.status[w - 1] = "landed";
                self.asks.retain(|a| *a != w);
                self.step("LandEnd");
                self.push = "ended";
            }
            "IssueClosed" if self.push == "ended" => {
                self.closed.extend(self.issues.get(stream));
                self.step("LandClose");
                self.at += 1;
                self.push = if self.at < self.pushed.len() { "pushed" } else { "landed" };
            }
            "StackLanded" => {
                let n = self.layer_of(data).map_or(0, |i| i + 1);
                self.stack.drain(..n);
                let pushed = self.proving.clone();
                let stale = self.stack.first().is_some_and(|l| l.on != pushed);
                self.push = if stale { "stale" } else { "idle" };
                self.step("StackLanded");
            }
            "LandingRed" => {
                let Some(w) = self.slot(stream) else { return };
                self.status[w - 1] = "working";
                self.asks.retain(|a| *a != w);
                self.step("TellRed");
                self.push = "idle";
            }
            "GateRan" if data["kind"] == "fast" => {}
            "GateRan" if data["green"] == true => self.prom_to("gated"),
            "GateRan" => self.prom_to("read"),
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

    /// StackLanding: the worker's branch merged onto the top's merge, or
    /// onto proving's tip on an empty stack (that commit itself, when it
    /// holds the branch already).
    fn stacked(&mut self, data: &Value) {
        let slot = data["number"].as_u64().unwrap() as usize;
        let on = self.stack.last().map_or(self.proving.clone(), |l| l.merged.clone());
        let mut merged = on.clone();
        if !merged.contains(&slot) {
            merged.push(slot);
        }
        self.names.insert(data["merged"].as_str().unwrap().into(), merged.clone());
        self.stack.push(Layer { slot, on, merged, green: None, red: false });
        self.step("StackOn");
    }

    /// Unstacked: the layer of a worker and those above it left the stack,
    /// as the step the model names for why: on a stale merge after a push,
    /// a red told next, or a push that answered an error.
    fn unstacked(&mut self, data: &Value) {
        let Some(i) = self.layer_of(data) else { return };
        let landable = self.landable();
        let action = if self.push == "stale" {
            "StackStale"
        } else if i == landable && self.stack[i].red {
            self.push = "red";
            "StackRed"
        } else if i < landable && self.status[self.stack[i].slot - 1] != "landing" {
            "StackPush"
        } else {
            // Refused, or lost after it took effect.
            let tip = self.stack[..landable].last().and_then(|l| l.green.clone());
            if let Some((commit, name)) = tip.filter(|t| self.proving_commits.contains(&t.0)) {
                self.proving = name;
                self.names.insert(commit, self.proving.clone());
            }
            "StackPush"
        };
        if action == "StackStale" {
            self.push = "idle";
        }
        self.stack.truncate(i);
        self.step(action);
    }

    /// A tick ended: a push's records or a red it left mid-way stopped there.
    fn tick_ended(&mut self) {
        if self.push != "idle" {
            self.step("Fail");
            self.push = "idle";
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
        let mut t = Trace { push: "idle", prom: "idle", proving_commits: self.proving_commits(), ..Trace::default() };
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

    /// Every commit proving's tip held, on each remote: its first-parent history.
    fn proving_commits(&self) -> HashSet<String> {
        let remotes = std::fs::read_dir(self.dir.join("remotes")).into_iter().flatten().flatten();
        remotes
            .filter_map(|r| self.git(&r.path(), &["log", "--first-parent", "--format=%H", "proving"]).ok())
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
