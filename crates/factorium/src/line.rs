//! The proving line's three moves: a landing merges a worker's branch onto
//! the proving tip and pushes it once the fast gate is green; a promotion
//! fast-forwards the base branch to the proving tip once its gate is green —
//! the slow steps alone on a tip a landing gated green, else the fast gate
//! then the slow steps; a cut makes a version from the base tip ([`cut`]). Each push
//! requires the tip it read. Each runs in a work dir of its own, removed
//! when it ends ([`crate::workdir`]). The daemon runs every heavy step of a move — a
//! landing's fast gate, a promotion's slow gate, a cut's steps — beside its
//! ticks ([`Moving`]), so no tick waits for one (factorium#231): the next
//! tick finds how they ended and finishes the move. A checkout's moves run apart: its landings
//! stack, their gates at once as the queue has room ([`stack`],
//! docs/adr/0019-stacked-landing-gates.md); no promotion while a promotion runs, no cut while a
//! cut runs or its binary is proven; landings go on while a promotion's gate or a cut's steps
//! run. They write different refs
//! (proving, the base, a tag) from work dirs of their own, and each push requires the tip it
//! read, so none needs another held; a landing line held behind them was most of the wait
//! between landings (docs/adr/0018-a-checkout-s-moves-run-apart.md). A landing waiting its turn
//! is checked for its merge onto the proving tip on the tick after `done`, and again each time
//! its branch or the tip moves: one that does not merge is red then, not at its turn
//! (factorium#307).
mod cut;
mod mutants;
mod stack;

pub(crate) use cut::Proving;

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::{Gating, Kind};
use factorium_domain::doctor::Stuck;
use factorium_domain::heavy::Ends;
use factorium_domain::line::{self, Due, Lines, Move, NotDue};
use factorium_domain::line_id;
use factorium_domain::outbox::Sender;
use factorium_domain::settings::Setting;
use factorium_domain::stack::{Stacks, stack_id};
use factorium_domain::worker::{self as w, Workers};
use factorium_host::{Git, Lag, NoMerge, Ran};
use factum::Uuid;
use serde_json::{Value, json};

use crate::gate::{At, Gated};
use crate::heavy::For;
use crate::lock::{self, Taken};
use crate::{Answer, App, LineDir, Want, fail};

/// Fast-forwards the checkout's local base to `tip`, never further, where
/// nothing of it is lost; answers what it did, if it moved or failed.
fn follow_base(c: &Checkout, git: &Git, tip: &str) -> Option<Value> {
    match git.follow(&c.base, tip) {
        Ok(Lag::Behind(n)) => Some(json!({ "base": c.name, "fast_forwarded": n })),
        Ok(_) => None,
        Err(e) => Some(json!({ "base": c.name, "error": e })),
    }
}

/// The checkout's local base, where it cannot follow origin's as last fetched.
pub(crate) fn stuck_base(c: &Checkout, git: &Git) -> Option<Stuck> {
    let (behind, ours, dirty) = match git.lag(&c.base, &c.fetched_base()).ok()? {
        Lag::Diverged { behind, ours } => (behind, ours, false),
        Lag::Dirty { behind } => (behind, 0, true),
        Lag::Even | Lag::Behind(_) => return None,
    };
    Some(Stuck { base: c.base.clone(), path: c.path.clone(), behind, ours, dirty })
}

/// What one move did, or why it did not run now.
pub(crate) type Did = Result<Value, String>;

/// Why a move that is due did not run this tick.
const QUEUED: &str = "waits its turn in the heavy-command queue, or runs beside the ticks";

/// A line's move whose heavy command runs beside the daemon's ticks, of `checkout`, in the work
/// dir `dir`: what is left to do once its steps end.
pub(crate) struct Moving {
    pub(crate) checkout: String,
    pub(crate) dir: std::path::PathBuf,
    then: Then,
}

impl Moving {
    /// The kind of job it is.
    pub(crate) fn kind(&self) -> LineDir {
        match &self.then {
            Then::Land { .. } => LineDir::Landing,
            Then::Promote { .. } => LineDir::Promotion,
            Then::Cut { .. } => LineDir::Cut,
        }
    }

    /// The worker it lands, if it is a landing.
    pub(crate) fn lands(&self) -> Option<Uuid> {
        match &self.then {
            Then::Land { layer } => Some(layer.worker),
            _ => None,
        }
    }
}

enum Then {
    /// The stacked landing of a worker's branch, merged in the work dir.
    Land { layer: factorium_domain::stack::Layer },
    /// The promotion of the proving `tip` over `base`, its gate run as `gating` says, holding
    /// the checkout's promotion lock.
    Promote { tip: String, base: String, gating: Gating, _lock: std::sync::Arc<std::fs::File> },
    /// The cut of `version` from `base`, its `steps` run in the work dir.
    Cut { version: String, base: String, steps: Vec<String> },
}

impl App {
    /// The tick's pass over the lines: each move that ran beside the ticks
    /// and ended, finished; each checkout's landing stack, then each one's
    /// promotion and cut as its line says each is due, but none of a kind
    /// whose move of that checkout runs on ([`App::runs`]); then its local base following origin's
    /// as last fetched, whatever left it behind. Answers what landed, and
    /// what else moved.
    pub(crate) async fn lines(&self, checkouts: &[Checkout]) -> Answer<(Vec<Value>, Vec<Value>)> {
        let (mut landed, mut lines) = (Vec::new(), Vec::new());
        for (of, ended) in self.beside(|f| matches!(f, For::Line(_)), false, false).await? {
            let For::Line(moving) = of else { continue };
            let into = if matches!(moving.then, Then::Land { .. }) { &mut landed } else { &mut lines };
            into.extend(self.moved(*moving, ended).await?.ok());
        }
        lines.extend(self.cuts_proven().await?);
        for c in checkouts {
            landed.extend(self.landings(c).await?);
        }
        // Once proving moved: a landing not stacked that no longer merges onto the tip leaves the line.
        landed.extend(self.check_waiting(checkouts).await?);
        for c in checkouts {
            for mv in [Move::Promote, Move::Cut] {
                if !self.runs(c, mv) {
                    lines.extend(self.carry(c, mv, false).await?.ok());
                }
            }
            lines.extend(follow_base(c, &self.host.git(&c.path), &c.fetched_base()));
        }
        Ok((landed, lines))
    }

    /// Whether a promotion or a cut of `c`, `mv`, runs on: its steps beside the ticks, or for a
    /// cut, its binary's proof. Moves of other kinds go on beside it.
    fn runs(&self, c: &Checkout, mv: Move) -> bool {
        let kind = if mv == Move::Cut { LineDir::Cut } else { LineDir::Promotion };
        self.beside_runs(|f| f.moves(&c.name, kind)) || (mv == Move::Cut && self.cut_proving(&c.name))
    }

    /// Finishes `moving`, whose steps ran beside the ticks and `ended` so: unqueued while they
    /// ran, it is due again. Its work dir goes either way.
    async fn moved(&self, moving: Moving, ended: Result<Vec<Ran>, Ends>) -> Answer<Did> {
        let (kind, checkout, dir) = (moving.kind(), moving.checkout.clone(), moving.dir.clone());
        let did = self.finish_moved(moving, ended).await;
        let _ = self.work_dir_done(kind, &checkout, &dir);
        did
    }

    async fn finish_moved(&self, moving: Moving, ended: Result<Vec<Ran>, Ends>) -> Answer<Did> {
        let c = self.checkout(&moving.checkout).await?;
        if let Then::Land { layer } = moving.then {
            self.landing_ran(&c, &self.host.git(&c.path).at(&moving.dir), layer, ended).await?;
            return Ok(Err("a stacked landing's gate ended: it lands with the stack".to_string()));
        }
        let Ok(rans) = ended else {
            return Ok(Err(format!("a move of {} ended while it ran: {}", c.name, ended.unwrap_err())));
        };
        let dir = moving.dir;
        match moving.then {
            Then::Land { .. } => unreachable!("finished above"),
            Then::Promote { tip, base, gating, _lock } => {
                let ran = rans.into_iter().next().expect("one step");
                let gated = self.gate_ran(&c.name, &self.host.git(&dir), &tip, false, Kind::Slow, ran).await?;
                self.promote_gated(&c, &self.host.git(&c.path), tip, base, &gating, gated).await
            }
            Then::Cut { version, base, steps } => {
                self.cut_ran(&c, &self.host.git(&dir), &version, &base, &steps, rans, true).await
            }
        }
    }

    /// Carries out `mv` on `c` if its line says it is due now.
    async fn carry(&self, c: &Checkout, mv: Move, by_hand: bool) -> Answer<Did> {
        // An urgent issue's halt (docs/adr/0015-an-urgent-issue-halts-the-host.md) holds no line move: its gate waits in the queue as usual.
        let (l, look) = self.line_look(c, by_hand).await?;
        let git = self.host.git(&c.path);
        // Past when it falls due, the move goes ahead of workers' waiting heavy entries.
        let bound_at = l.owed(mv, look.now, &look.bounds).map(|o| o.due_at);
        match l.due(mv, &look, || git.fetch(&c.proving).map_err(fail("git")))? {
            // A failed cut whose steps hold now is made, without a new cut.
            Err(NotDue::CutFailed { version, why }) => match self.cut_read_back(c, &version).await? {
                Some(did) => Ok(Ok(did)),
                None => Ok(Err(line::why(NotDue::CutFailed { version, why }))),
            },
            Err(not) => Ok(Err(line::why(not))),
            // Landings stack in the tick's pass ([`App::landings`]), never one by one.
            Ok(Due::Land(_)) => Ok(Err("landings are stacked by the tick's pass".into())),
            Ok(Due::Promote(tip)) => self.promote_tip(c, &git, tip, by_hand, bound_at).await,
            Ok(Due::Cut) => self.cut_base(c, by_hand, bound_at).await,
        }
    }

    /// Checks each landing that waits its turn (asked, not in its checkout's stack)
    /// for its merge onto its checkout's proving tip, unless its branch and the tip are as when
    /// it last merged: red now, each that does not merge (factorium#307).
    async fn check_waiting(&self, checkouts: &[Checkout]) -> Answer<Vec<Value>> {
        let live = self.live_workers().await?;
        let asks: Vec<_> = live.iter().filter(|w| w.state.status == w::Status::Landing).collect();
        let mut reds = Vec::new();
        for c in checkouts {
            let stack = self.state::<Stacks>(stack_id(&c.name)).await?;
            let mut waiting = asks.iter().filter(|a| a.checkout == c.name && stack.of(a.worker).is_none()).peekable();
            if waiting.peek().is_none() {
                continue;
            }
            let git = self.host.git(&c.path);
            let tip = git.fetch(&c.proving).map_err(fail("git"))?;
            let records = self.declared(&git, &tip)?.gate.regenerated;
            for worker in waiting.map(|a| a.worker) {
                let wk = self.state::<Workers>(worker).await?;
                let Some(st) = wk.started.as_ref() else { continue };
                let head = git.sha(&st.branch).map_err(fail("git"))?;
                let at = (tip.clone(), head);
                if self.memory.lock().unwrap().merges.get(&worker) == Some(&at) {
                    continue;
                }
                match git.merges(&tip, &st.branch, &records) {
                    Ok(()) => {
                        self.submit::<w::NoteMergeChecked>(w::Checked { worker, tip: tip.clone() }).await?;
                        self.memory.lock().unwrap().merges.insert(worker, at);
                    }
                    Err(e) => reds.push(self.not_merging(&git, worker, &wk, &tip, e).await?),
                }
            }
        }
        Ok(reds)
    }

    /// The red of `worker`, `wk`, whose branch does not merge onto the proving `tip` of the
    /// checkout `git`, and `e` says why: told as for a first red when proving moved since the
    /// last red's tip, which the branch now holds (factorium#171).
    pub(super) async fn not_merging(
        &self,
        git: &Git,
        worker: Uuid,
        wk: &w::Worker,
        tip: &str,
        e: NoMerge,
    ) -> Answer<Value> {
        let branch = wk.started.as_ref().map(|st| st.branch.as_str()).unwrap_or_default();
        let moved =
            wk.red_tip.as_deref().filter(|p1| *p1 != tip && git.is_ancestor(p1, tip) && git.is_ancestor(p1, branch));
        let since = match moved.map(|p1| (p1, git.landings(p1, tip))) {
            Some((p1, Ok(landed))) => format!(" Proving moved since {p1}: {}.", landed.join("; ")),
            _ => String::new(),
        };
        let why = format!(
            "your branch does not merge onto the proving tip {tip}: {e}.{since} Merge or rebase onto it and resolve."
        );
        let moved_max = self.settings().await?.get(Setting::MovedRedsMax) as u32;
        let (kind, files, tip, moved) = (w::RedKind::NoMerge, e.files, Some(tip.to_string()), moved.is_some());
        self.red(w::Red { worker, why, kind, files, tip, moved, moved_max }).await
    }

    pub(super) async fn red(&self, red: w::Red) -> Answer<Value> {
        let (worker, why) = (red.worker, red.why.clone());
        self.submit::<w::RedLanding>(red).await?;
        let wk = self.state::<Workers>(worker).await?;
        // A red that does not reach it sends it to the watch at once, not after its silence.
        if let Some(live) = self.live_or_none(worker, Want::Working).await? {
            let text = format!("your landing was red. {why}\nFix it, commit, and run `factorium worker done` again.");
            self.tell(&live.session, Sender::Factorium, &text, false).await?;
        }
        Ok(json!({ "worker": worker, "red": why, "status": wk.status }))
    }

    /// Promotes the proving `tip` of `c`: its gate, then the base fast-forwarded to it. The tip
    /// a green landing pushed (its record names it) runs the slow steps alone, none when the
    /// checkout declares none; any other, the fast gate then the slow steps (docs/adr/0012-the-line-gate-landing-promotion-cut.md).
    /// `bound_at`: when the promotion falls due however busy.
    async fn promote_tip(
        &self,
        c: &Checkout,
        git: &Git,
        tip: String,
        by_hand: bool,
        bound_at: Option<i64>,
    ) -> Answer<Did> {
        // One promotion of a checkout at a time: a second would share the
        // build cache with the running one's slow gate.
        let who = if by_hand { "`factorium promote`" } else { "the daemon's promotion" };
        let running = match lock::take(
            &self.data().promotion_lock(&c.name),
            &format!("{who} of {tip}, pid {}", std::process::id()),
        ) {
            Ok(running) => std::sync::Arc::new(running),
            Err(taken) => {
                let held = match taken {
                    Taken::Held(held) if held.is_empty() => "another promotion".into(),
                    Taken::Held(e) | Taken::Fails(e) => e,
                };
                return Ok(Err(format!("a promotion of {} runs: {held}", c.name)));
            }
        };
        let base = git.fetch(&c.base).map_err(fail("git"))?;
        let line = self.state::<Lines>(line_id(&c.name)).await?;
        let gating = match line.unpromoted.iter().find(|l| l.commit == tip) {
            Some(l) => Gating::SlowOnly { by: format!("{}#{}", c.name, l.number) },
            None => Gating::Full,
        };
        let short = &tip[..tip.len().min(7)];
        let about = format!(
            "factorium's promotion of {}: its gate run on the proving tip {short}, the base {} moved to it once green.",
            c.name, c.base
        );
        let here = self.work_dir(c, LineDir::Promotion, 0, &format!("{} {short}", c.name), &about, &tip)?;
        // Its mutants first, where slimfit holds them at the base; by hand, whatever they owe.
        if !by_hand && let Some(did) = self.mutants_first(c, &here, &tip).await? {
            let _ = self.work_dir_done(LineDir::Promotion, &c.name, &here.dir);
            return Ok(did);
        }
        let at = if by_hand {
            At::Commit { rev: &tip, bound_at }
        } else {
            let _lock = running.clone();
            let then = Then::Promote { tip: tip.clone(), base: base.clone(), gating: gating.clone(), _lock };
            let moving = Moving { checkout: c.name.clone(), dir: here.dir.clone(), then };
            At::Beside { rev: &tip, bound_at, moving: Box::new(moving) }
        };
        let Some(gated) = self.gate(&c.name, &here, at, gating.clone()).await? else { return Ok(Err(QUEUED.into())) };
        let did = self.promote_gated(c, git, tip, base, &gating, gated).await;
        let _ = self.work_dir_done(LineDir::Promotion, &c.name, &here.dir);
        did
    }

    /// Promotes the proving `tip` of `c` over `base` as its gate, run as `gating`, says:
    /// red, it is recorded red; green, the base is fast-forwarded to it. Both say which case ran.
    async fn promote_gated(
        &self,
        c: &Checkout,
        git: &Git,
        tip: String,
        base: String,
        gating: &Gating,
        gated: Gated,
    ) -> Answer<Did> {
        let (behind, said) = (git.is_ancestor(&base, &tip), gating.said());
        let red = (!gated.green).then(|| format!("{said}: {}", gated.why()));
        let (checkout, gated) = (c.name.clone(), said);
        let promotion = line::Promotion { checkout, tip: tip.clone(), base: base.clone(), red, behind, gated };
        if let Some(why) = promotion.red() {
            self.submit::<line::Promote>(promotion).await?;
            return Ok(Ok(json!({ "promotion": c.name, "red": why })));
        }
        let gated = promotion.gated.clone();
        if let Err(e) = git.push_requiring(&tip, &c.base, Some(&base)) {
            return Ok(Ok(json!({ "promotion": c.name, "retry": e })));
        }
        self.submit::<line::Promote>(promotion).await?;
        let local = follow_base(c, git, &tip);
        Ok(Ok(json!({ "promoted": c.name, "tip": tip, "gated": gated, "local": local })))
    }

    /// `factorium promote`: promotes now, whatever the bound or a red tip says.
    pub async fn promote(&self, checkout: &str) -> Answer {
        let c = self.checkout(checkout).await?;
        Ok(self.carry(&c, Move::Promote, true).await?.unwrap_or_else(|why| json!({ "promoted": null, "why": why })))
    }

    /// `factorium cut`: cuts now, whatever the bound or a failed cut says.
    pub async fn cut(&self, checkout: &str) -> Answer {
        let c = self.checkout(checkout).await?;
        Ok(self.carry(&c, Move::Cut, true).await?.unwrap_or_else(|why| json!({ "cut": null, "why": why })))
    }
}
