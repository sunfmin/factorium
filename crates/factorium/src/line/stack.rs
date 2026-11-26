//! A checkout's landings, stacked (docs/adr/0019-stacked-landing-gates.md): every worker that
//! asks to land is merged onto the top of its checkout's stack ([`factorium_domain::stack`]),
//! the first onto the proving tip, and its fast gate asks the heavy queue for its turn in a work
//! dir of its own, beside the ticks; the queue's room alone decides how many run at once. Each
//! tick, proving moves in one push to the deepest merge whose gate and every gate below it are
//! green, and each worker in it lands. A red gate's worker is told once every gate below it is
//! green (a merge stacked on a red one is red with it); the merges above it leave the stack, their
//! gates ended, and are stacked again without it on the next tick. A branch that
//! does not merge onto the top waits while it merges onto the proving tip, and is red as today
//! when it does not.

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::{Gating, Kind};
use factorium_domain::heavy::Ends;
use factorium_domain::line::{self, Due, Lines, Move};
use factorium_domain::line_id;
use factorium_domain::stack::{
    self as s, Green, GreenLayer, LandStack, Layer, OfLayer, Red, RedLayer, StackLanding, Stacking, Stacks, Unstack,
    stack_id,
};
use factorium_domain::worker::{self as w, Workers};
use factorium_host::{Git, Ran};
use factum::Uuid;
use serde_json::{Value, json};

use super::{Moving, Then};
use crate::gate::{At, Gated};
use crate::heavy::For;
use crate::life::End;
use crate::{Answer, App, LineDir, fail};

/// A stacked landing whose fast gate runs beside the ticks: its layer as stacked.
#[derive(Clone)]
pub(super) struct Landing {
    pub(super) layer: Layer,
}

impl App {
    /// The tick's pass over `c`'s landing stack: what is landable lands; each gate that waits its
    /// turn asks again; then each worker that asks to land and is not stacked goes on top.
    /// Answers what landed, and each red. Each step's future is boxed: the tick's would
    /// otherwise outgrow a thread's stack.
    pub(super) async fn landings(&self, c: &Checkout) -> Answer<Vec<Value>> {
        let git = self.host.git(&c.path);
        let mut did = Box::pin(self.land_stack(c, &git)).await?;
        for layer in self.state::<Stacks>(stack_id(&c.name)).await?.layers {
            if !layer.gated() && !self.gate_runs(&c.name, &layer) {
                did.extend(Box::pin(self.ask_gate(c, layer)).await?);
            }
        }
        let asks = match self.due_landings(c).await? {
            Some(asks) => asks,
            None => return Ok(did),
        };
        let stacked = self.state::<Stacks>(stack_id(&c.name)).await?;
        let tip = git.fetch(&c.proving).map_err(fail("git"))?;
        for worker in asks.into_iter().filter(|a| stacked.of(*a).is_none()) {
            did.extend(Box::pin(self.stack_on(c, &git, &tip, worker)).await?);
        }
        Ok(did)
    }

    /// Those that ask to land on `c`, in the order they stack, if landings are due.
    async fn due_landings(&self, c: &Checkout) -> Answer<Option<Vec<Uuid>>> {
        let (l, look) = self.line_look(c, false).await?;
        match l.due(Move::Land, &look, || Ok::<_, crate::Refusal>(String::new()))? {
            Ok(Due::Land(asks)) => Ok(Some(asks)),
            _ => Ok(None),
        }
    }

    /// Whether the gate of `layer` of `checkout` runs beside the ticks.
    fn gate_runs(&self, checkout: &str, layer: &Layer) -> bool {
        self.beside_runs(|f| of_layer(f, checkout, layer))
    }

    /// Merges `worker`'s branch onto the top of `c`'s stack (the proving `tip`, empty) in a work
    /// dir of its own, stacks it and asks for its gate. One that does not merge there waits while
    /// it merges onto the tip; one that does not merge onto the tip is red.
    async fn stack_on(&self, c: &Checkout, git: &Git, tip: &str, worker: Uuid) -> Answer<Vec<Value>> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.clone() else { return Ok(Vec::new()) };
        let stack = self.state::<Stacks>(stack_id(&c.name)).await?;
        let below = stack.layers.last();
        let on = below.map_or(tip, |l| l.merged.as_str()).to_string();
        let issue = self.issue(&c.name, st.number).await?;
        let (name, short) = (format!("{}#{}", c.name, st.number), &on[..on.len().min(7)]);
        let onto = if below.is_some() { "the landings stacked below it, at" } else { "the proving tip" };
        let about = format!(
            "factorium's landing of {name} ({}): its branch {} merged onto {onto} {short}, its fast gate run here.",
            issue.title, st.branch
        );
        let slot = stack.free_slot();
        let here = self.work_dir(c, LineDir::Landing, slot, &name, &about, &on)?;
        let message = format!("Land {name}: {}", issue.title);
        // Conflicts only in the records the gate regenerates take the copy below, rewritten by
        // the gate (#225).
        let records = self.declared(&here, &on)?.gate.regenerated;
        // Read before the merge: a branch that moves after it is merged again, never pushed unseen.
        let head = git.sha(&st.branch).map_err(fail("git"))?;
        let (merged, mut regenerated) = match here.merge(&st.branch, &message, &records) {
            Ok(merged) => merged,
            Err(e) => {
                let _ = self.work_dir_done(LineDir::Landing, &c.name, &here.dir);
                // It conflicts with a landing below it, not with proving: it waits for them.
                if below.is_some() && git.merges(tip, &st.branch, &records).is_ok() {
                    return Ok(Vec::new());
                }
                return Ok(vec![self.not_merging(git, worker, &wk, tip, e).await?]);
            }
        };
        for r in below.iter().flat_map(|l| &l.regenerated) {
            if !regenerated.contains(r) {
                regenerated.push(r.clone());
            }
        }
        let dir = here.dir.display().to_string();
        let (number, green, red) = (st.number, None, None);
        let layer = Layer { worker, number, on, head, merged, message, regenerated, dir, slot, green, red };
        self.submit::<StackLanding>(Stacking { checkout: c.name.clone(), layer: layer.clone() }).await?;
        self.ask_gate(c, layer).await
    }

    /// Asks for the fast gate of `layer` of `c` in its work dir: it runs beside the ticks once
    /// the queue gives it room. One whose tree is green already is green now. One whose work dir
    /// is gone leaves the stack, with those above it.
    async fn ask_gate(&self, c: &Checkout, layer: Layer) -> Answer<Vec<Value>> {
        let dir = std::path::PathBuf::from(&layer.dir);
        if !dir.is_dir() {
            self.unstack(&c.name, layer.worker).await?;
            return Ok(Vec::new());
        }
        let here = self.host.git(&c.path).at(&dir);
        let then = Then::Land { landing: Landing { layer: layer.clone() } };
        let moving = Box::new(Moving { checkout: c.name.clone(), dir: dir.clone(), then });
        let at = At::Beside { rev: &layer.merged, bound_at: None, moving };
        let Some(gated) = self.gate(&c.name, &here, at, Gating::Fast).await? else { return Ok(Vec::new()) };
        let did = self.layer_gated(c, &here, layer, Ok(gated)).await;
        let _ = self.work_dir_done(LineDir::Landing, &c.name, &dir);
        Ok(did?.into_iter().collect())
    }

    /// Finishes the gate of a stacked landing, whose steps `ended` so beside the ticks.
    pub(super) async fn landing_ran(
        &self,
        c: &Checkout,
        here: &Git,
        landing: Landing,
        ended: Result<Vec<Ran>, Ends>,
    ) -> Answer<Option<Value>> {
        let layer = landing.layer;
        let gated = match ended {
            Ok(rans) => {
                let ran = rans.into_iter().next().expect("one step");
                Ok(self.gate_ran(&c.name, here, &layer.merged, false, Kind::Fast, ran).await?)
            }
            Err(e) => Err(e),
        };
        self.layer_gated(c, here, layer, gated).await
    }

    /// Records how the gate of `layer`, in the work dir `here`, ended: green, the layer is
    /// green, its regenerated records amended into its merge; red, the layer is red, its worker
    /// told once every gate below is green ([`App::land_stack`]). Ended before its steps did, or
    /// no longer to land (its worker no longer asks, or its branch moved), it leaves the stack
    /// with every layer above it, and lands again later. A layer that left the stack while its
    /// gate ran is let be.
    async fn layer_gated(
        &self,
        c: &Checkout,
        here: &Git,
        layer: Layer,
        gated: Result<Gated, Ends>,
    ) -> Answer<Option<Value>> {
        let stack = self.state::<Stacks>(stack_id(&c.name)).await?;
        if stack.of(layer.worker).is_none_or(|l| l.merged != layer.merged) {
            return Ok(None);
        }
        let worker = layer.worker;
        let wk = self.state::<Workers>(worker).await?;
        let branch = wk.started.as_ref().map(|st| st.branch.as_str()).unwrap_or_default();
        let moved = self.host.git(&c.path).sha(branch).ok().as_deref() != Some(layer.head.as_str());
        let gated = match gated {
            Ok(gated) if wk.status == w::Status::Landing && !moved => gated,
            _ => {
                self.unstack(&c.name, worker).await?;
                return Ok(None);
            }
        };
        if !gated.green {
            let why = format!("the fast gate is red on your branch merged onto the proving tip:\n{}", gated.why());
            let kind = match &gated.ran {
                Some(ran) if ran.timed_out => w::RedKind::PastLimit,
                _ if !layer.regenerated.is_empty() => w::RedKind::Regenerated,
                _ => w::RedKind::GateRed,
            };
            self.submit::<RedLayer>(Red { checkout: c.name.clone(), worker, why, kind }).await?;
            return Ok(None);
        }
        let commit = if layer.regenerated.is_empty() {
            layer.merged
        } else {
            let message = format!(
                "{}\n\nRegenerated by the fast gate on the merge, from the copy below it: {}",
                layer.message,
                layer.regenerated.join(", ")
            );
            here.amend(&layer.regenerated, &message).map_err(fail("git"))?
        };
        self.submit::<GreenLayer>(Green { checkout: c.name.clone(), worker, commit }).await?;
        Ok(None)
    }

    /// Lands what of `c`'s stack is landable, then tells the red above it: proving pushed, in one
    /// push requiring the tip the stack's bottom merged onto, to the deepest landable layer's
    /// commit, each of them recorded and its worker ended. A layer whose worker no longer asks to
    /// land, or whose branch moved, leaves the stack with those above it first. The stack no
    /// longer on the tip leaves whole; layers merged onto a commit that is not the one pushed (it
    /// amended regenerated records) are stacked again.
    async fn land_stack(&self, c: &Checkout, git: &Git) -> Answer<Vec<Value>> {
        let mut did = Box::pin(self.push_stack(c, git)).await?;
        if let Some(layer) = self.state::<Stacks>(stack_id(&c.name)).await?.red().cloned() {
            self.unstack(&c.name, layer.worker).await?;
            let (worker, (why, kind), files) = (layer.worker, layer.red.expect("red"), layer.regenerated);
            if self.state::<Workers>(worker).await?.status == w::Status::Landing {
                let red = w::Red { worker, why, kind, files, tip: None, moved: false, moved_max: 0 };
                did.push(self.red(red).await?);
            }
        }
        Ok(did)
    }

    /// Pushes what of `c`'s stack is landable ([`App::land_stack`]).
    async fn push_stack(&self, c: &Checkout, git: &Git) -> Answer<Vec<Value>> {
        let stack = self.state::<Stacks>(stack_id(&c.name)).await?;
        let mut landing = Vec::new();
        for layer in stack.landable() {
            let wk = self.state::<Workers>(layer.worker).await?;
            let branch = wk.started.as_ref().map(|st| st.branch.as_str()).unwrap_or_default();
            if wk.status != w::Status::Landing || git.sha(branch).ok().as_deref() != Some(layer.head.as_str()) {
                self.unstack(&c.name, layer.worker).await?;
                break;
            }
            landing.push(layer.clone());
        }
        let (Some(first), Some(last)) = (landing.first(), landing.last()) else { return Ok(Vec::new()) };
        let pushed = last.green.clone().expect("landable");
        if let Err(e) = git.push_requiring(&pushed, &c.proving, Some(&first.on)) {
            // The tip moved since the stack's bottom merged onto it: every layer lands again on the new one.
            self.unstack(&c.name, first.worker).await?;
            return Ok(vec![json!({ "stack": c.name, "retry": e })]);
        }
        let mut did = Vec::new();
        for layer in &landing {
            let commit = if layer.worker == last.worker { pushed.clone() } else { layer.merged.clone() };
            let urgent = self.went_ahead(c, layer).await?;
            let (checkout, number) = (c.name.clone(), layer.number);
            self.submit::<line::RecordLanding>(line::Landed { checkout, number, commit: commit.clone() }).await?;
            self.end_worker(layer.worker, End::Landed(commit.clone())).await?;
            let mut landed = json!({ "landed": format!("{}#{number}", c.name), "commit": commit });
            if let Some(went) = urgent {
                landed["urgent"] = went;
            }
            did.push(landed);
        }
        self.submit::<LandStack>(OfLayer { checkout: c.name.clone(), worker: last.worker }).await?;
        let rest = self.state::<Stacks>(stack_id(&c.name)).await?;
        if let Some(next) = rest.layers.first().filter(|l| l.on != pushed) {
            self.unstack(&c.name, next.worker).await?;
        }
        Ok(did)
    }

    /// What the urgent `layer` of `c` went ahead of, if it did: each landing not urgent that
    /// asked before it and has not landed.
    async fn went_ahead(&self, c: &Checkout, layer: &Layer) -> Answer<Option<Value>> {
        if !self.issue(&c.name, layer.number).await?.urgent {
            return Ok(None);
        }
        let mut ahead_of = Vec::new();
        for o in self.live_workers().await?.into_iter().filter(|o| o.checkout == c.name) {
            if o.worker == layer.worker {
                break;
            }
            if o.state.status == w::Status::Landing && !self.issue(&c.name, o.number).await?.urgent {
                ahead_of.push(format!("{}#{}", c.name, o.number));
            }
        }
        Ok((!ahead_of.is_empty()).then(|| json!({ "ahead_of": ahead_of })))
    }

    /// Takes the layer of `worker` and every layer above it off `checkout`'s stack: the gates of
    /// those that run end, the work dirs of those that wait go.
    pub(super) async fn unstack(&self, checkout: &str, worker: Uuid) -> Answer<()> {
        let gone = self.state::<Stacks>(stack_id(checkout)).await?.from(worker);
        self.submit::<Unstack>(OfLayer { checkout: checkout.into(), worker }).await?;
        for layer in gone.iter().filter(|l| !l.gated()) {
            if self.gate_runs(checkout, layer) {
                self.unqueue_beside(|f| of_layer(f, checkout, layer)).await?;
            } else {
                let _ = self.work_dir_done(LineDir::Landing, checkout, std::path::Path::new(&layer.dir));
            }
        }
        Ok(())
    }

    /// As the daemon starts: every checkout's stack is let go, the gates that ran it gone with
    /// the daemon that ran them; each landing is stacked again.
    pub(crate) async fn clear_stacks(&self) -> Answer<()> {
        for c in self.listed().await? {
            self.submit::<s::ClearStack>(s::Clear { checkout: c.name }).await?;
        }
        Ok(())
    }

    /// What `c`'s line holds and what the app sees of it now, for the line to judge.
    pub(super) async fn line_look(&self, c: &Checkout, by_hand: bool) -> Answer<(line::Line, factorium_domain::neutral::landing::Look)> {
        let l = self.state::<Lines>(line_id(&c.name)).await?;
        let settings = self.settings().await?;
        let mut workers = Vec::new();
        for w in self.live_workers().await?.into_iter().filter(|w| w.checkout == c.name) {
            let urgent = w.state.status == w::Status::Landing && self.issue(&w.checkout, w.number).await?.urgent;
            workers.push(line::Seen { worker: w.worker, number: w.number, status: w.state.status, urgent });
        }
        Ok((l, line::look(self.now(), &settings, &workers, by_hand)))
    }
}

/// Whether `f` is the gate of `layer` of `checkout`.
fn of_layer(f: &For, checkout: &str, layer: &Layer) -> bool {
    matches!(f, For::Line(m) if m.checkout == checkout
        && matches!(&m.then, Then::Land { landing } if landing.layer.worker == layer.worker && landing.layer.merged == layer.merged))
}
