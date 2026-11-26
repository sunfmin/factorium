//! A checkout's landing stack (docs/adr/0019-stacked-landing-gates.md): the
//! landings whose fast gates run at once, the first merged onto the proving
//! tip and each next onto the merge below it; and what of it lands now, the
//! deepest merge whose gate and every gate below it are green
//! ([`Stack::landable`]), and whose red is told now, the first above them
//! ([`Stack::red`]): a merge stacked on a red one is red with it. The app
//! merges, gates and pushes; the stack records each layer, how its gate
//! ended, and what left it.

use factum_core::{Aggregate, Uuid};
use serde::{Deserialize, Serialize};

use crate::worker::RedKind;
use crate::{command, id, no};

/// The stream of `checkout`'s landing stack.
pub fn stack_id(checkout: &str) -> Uuid {
    id(&format!("stack:{checkout}"))
}

/// A landing in the stack: its worker's branch merged onto the layer below
/// (the proving tip, for the first), its fast gate run in a work dir of its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub worker: Uuid,
    /// Its issue's number.
    pub number: u64,
    /// The commit it merged onto: the proving tip, or the merge below.
    pub on: String,
    /// The worker's branch head it merged: moved since, it lands again.
    pub head: String,
    pub merged: String,
    pub message: String,
    /// The records the gate regenerates (`[gate] regenerated`) this merge
    /// took from the copy below, with those the layers below took: its gate
    /// rewrites them, and they are amended into its merge.
    pub regenerated: Vec<String>,
    /// The work dir its gate runs in, and the slot of the build cache it took.
    pub dir: String,
    pub slot: usize,
    /// Once its gate is green: the commit it lands as, its merge with the
    /// regenerated records amended.
    pub green: Option<String>,
    /// Once its gate is red: why, and what kind of red; told once every gate below it is green.
    #[serde(default)]
    pub red: Option<(String, RedKind)>,
}

impl Layer {
    /// Its gate ended, green or red: its work dir is gone.
    pub fn gated(&self) -> bool {
        self.green.is_some() || self.red.is_some()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    /// The bottom first.
    pub layers: Vec<Layer>,
}

impl Stack {
    /// The layers that land now: from the bottom, each whose gate is green,
    /// up to the first that is not. Proving moves to the last one's commit.
    pub fn landable(&self) -> &[Layer] {
        let n = self.layers.iter().take_while(|l| l.green.is_some()).count();
        &self.layers[..n]
    }

    /// The red layer whose worker is told now: the first that is not green,
    /// if its gate is red, every gate below it green.
    pub fn red(&self) -> Option<&Layer> {
        self.layers.get(self.landable().len()).filter(|l| l.red.is_some())
    }

    /// The commit the next layer merges onto: the top's merge, or none
    /// (the proving tip) on an empty stack.
    pub fn top(&self) -> Option<&str> {
        self.layers.last().map(|l| l.merged.as_str())
    }

    /// The layer of `worker`, if it is stacked.
    pub fn of(&self, worker: Uuid) -> Option<&Layer> {
        self.layers.iter().find(|l| l.worker == worker)
    }

    /// The lowest cache slot no layer whose gate has not ended holds: an
    /// ended one's work dir is gone.
    pub fn free_slot(&self) -> usize {
        let held = |s: usize| self.layers.iter().any(|l| !l.gated() && l.slot == s);
        (0..).find(|s| !held(*s)).expect("a free slot")
    }

    /// The layer of `worker` and those above it, top first: what leaves when it does.
    pub fn from(&self, worker: Uuid) -> Vec<Layer> {
        let at = self.layers.iter().position(|l| l.worker == worker).unwrap_or(self.layers.len());
        self.layers[at..].iter().rev().cloned().collect()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StackEvent {
    Stacked(Layer),
    LayerGreen {
        worker: Uuid,
        commit: String,
    },
    LayerRed {
        worker: Uuid,
        why: String,
        kind: RedKind,
    },
    /// The layer of `worker` left, with every layer above it.
    Unstacked {
        worker: Uuid,
    },
    /// The layers up to `worker`'s landed.
    StackLanded {
        worker: Uuid,
    },
}

pub struct Stacks;

impl Aggregate for Stacks {
    const NAME: &'static str = "stack";
    type State = Stack;
    type Event = StackEvent;

    fn evolve(s: &mut Stack, event: StackEvent) {
        let at = |s: &Stack, w: Uuid| s.layers.iter().position(|l| l.worker == w);
        match event {
            StackEvent::Stacked(layer) => s.layers.push(layer),
            StackEvent::LayerGreen { worker, commit } => {
                if let Some(l) = s.layers.iter_mut().find(|l| l.worker == worker) {
                    l.green = Some(commit);
                }
            }
            StackEvent::LayerRed { worker, why, kind } => {
                if let Some(l) = s.layers.iter_mut().find(|l| l.worker == worker) {
                    l.red = Some((why, kind));
                }
            }
            StackEvent::Unstacked { worker } => {
                let n = at(s, worker).unwrap_or(s.layers.len());
                s.layers.truncate(n);
            }
            StackEvent::StackLanded { worker } => {
                let n = at(s, worker).map_or(0, |i| i + 1);
                s.layers.drain(..n);
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stacking {
    pub checkout: String,
    pub layer: Layer,
}

// A layer goes on top, merged onto the top's merge; the first, onto any tip.
command!(StackLanding, "stack.push", Stacks, Stacking, |p| stack_id(&p.checkout), |s, p, now| {
    if s.of(p.layer.worker).is_some() {
        return no("stacked", "the worker's landing is in the stack already");
    }
    if s.top().is_some_and(|top| top != p.layer.on) {
        return no("not_on_top", format!("merged onto {}, not the top of the stack", p.layer.on));
    }
    Ok(vec![StackEvent::Stacked(p.layer.clone())])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Green {
    pub checkout: String,
    pub worker: Uuid,
    pub commit: String,
}

command!(GreenLayer, "stack.green", Stacks, Green, |p| stack_id(&p.checkout), |s, p, now| {
    if s.of(p.worker).is_none() {
        return no("not_stacked", "the worker's landing is not in the stack");
    }
    Ok(vec![StackEvent::LayerGreen { worker: p.worker, commit: p.commit.clone() }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Red {
    pub checkout: String,
    pub worker: Uuid,
    pub why: String,
    pub kind: RedKind,
}

command!(RedLayer, "stack.red", Stacks, Red, |p| stack_id(&p.checkout), |s, p, now| {
    if s.of(p.worker).is_none() {
        return no("not_stacked", "the worker's landing is not in the stack");
    }
    Ok(vec![StackEvent::LayerRed { worker: p.worker, why: p.why.clone(), kind: p.kind }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OfLayer {
    pub checkout: String,
    pub worker: Uuid,
}

// It leaves with those above it; one not stacked, nothing.
command!(Unstack, "stack.drop", Stacks, OfLayer, |p| stack_id(&p.checkout), |s, p, now| {
    Ok(s.of(p.worker).map(|_| StackEvent::Unstacked { worker: p.worker }).into_iter().collect())
});

// Only what is landable lands: it and every layer below it green.
command!(LandStack, "stack.landed", Stacks, OfLayer, |p| stack_id(&p.checkout), |s, p, now| {
    if !s.landable().iter().any(|l| l.worker == p.worker) {
        return no("not_landable", "the layer or one below it is not green");
    }
    Ok(vec![StackEvent::StackLanded { worker: p.worker }])
});
