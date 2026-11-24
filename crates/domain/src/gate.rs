//! A gate's record: green runs are kept against the tree they ran on, so
//! the same tree is never gated twice.

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::declared::Kind;
use crate::{command, id, named};

/// The words that name the record of `kind` on `tree` in `checkout`.
pub fn record_key(checkout: &str, tree: &str, kind: Kind) -> String {
    format!("gate:{checkout}:{tree}:{}", named(kind))
}

/// The stream of the record of `kind` on `tree` in `checkout`.
pub fn record_id(checkout: &str, tree: &str, kind: Kind) -> factum_core::Uuid {
    id(&record_key(checkout, tree, kind))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub green: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GateEvent {
    GateRan { checkout: String, tree: String, kind: Kind, green: bool, took_ms: u64, tail: String },
}

pub struct Gates;

impl Aggregate for Gates {
    const NAME: &'static str = "gate";
    type State = Record;
    type Event = GateEvent;

    fn evolve(s: &mut Record, GateEvent::GateRan { green, .. }: GateEvent) {
        s.green |= green;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ran {
    pub checkout: String,
    pub tree: String,
    pub kind: Kind,
    pub green: bool,
    pub took_ms: u64,
    pub tail: String,
}

command!(RecordGate, "gate.record", Gates, Ran, |p| record_id(&p.checkout, &p.tree, p.kind), |s, p, now| {
    Ok(vec![GateEvent::GateRan {
        checkout: p.checkout.clone(),
        tree: p.tree.clone(),
        kind: p.kind,
        green: p.green,
        took_ms: p.took_ms,
        tail: p.tail.clone(),
    }])
});
