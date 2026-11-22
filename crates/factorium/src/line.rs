//! The proving line's three moves: a landing merges a worker's branch onto
//! the proving tip and pushes it once the fast gate is green; a promotion
//! fast-forwards the base branch to the proving tip once the slow gate is
//! green; a cut makes a version from the base tip ([`cut`]). Each push
//! requires the tip it read. Each runs in its own clone of the checkout
//! under the data directory. The daemon runs every heavy step of a move — a
//! landing's fast gate, a promotion's slow gate, a cut's steps — beside its
//! ticks ([`Moving`]), so no tick waits for one (factorium#231): the next
//! tick finds how they ended and finishes the move. A checkout's moves go one at a time: none
//! of its moves is carried while one of them runs beside the ticks.

mod cut;

use std::path::Path;

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::Kind;
use factorium_domain::doctor::Stuck;
use factorium_domain::heavy::Ends;
use factorium_domain::line::{self, Due, Lines, Move, NotDue};
use factorium_domain::line_id;
use factorium_domain::outbox::Sender;
use factorium_domain::settings::Setting;
use factorium_domain::worker::{self as w, Workers};
use factorium_host::{Git, Lag, Ran};
use factum::Uuid;
use serde_json::{Value, json};

use crate::gate::{At, Gated};
use crate::heavy::For;
use crate::life::End;
use crate::lock::{self, Taken};
use crate::{Answer, App, LineClone, Want, fail};

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

/// A line's move whose heavy command runs beside the daemon's ticks, of `checkout`: what is
/// left to do once its steps end.
pub(crate) struct Moving {
    pub(crate) checkout: String,
    then: Then,
}

enum Then {
    /// The landing of a worker's branch, merged in the clone `dir`.
    Land { landing: Landing, dir: std::path::PathBuf },
    /// The promotion of the proving `tip` over `base`, its slow gate run in the clone `dir`,
    /// holding the checkout's promotion lock.
    Promote { tip: String, base: String, dir: std::path::PathBuf, _lock: std::sync::Arc<std::fs::File> },
    /// The cut of `version` from `base`, its `steps` run in the clone `dir`.
    Cut { version: String, base: String, dir: std::path::PathBuf, steps: Vec<String> },
}

/// A worker's branch merged onto the proving tip, its fast gate to run.
#[derive(Clone)]
struct Landing {
    worker: Uuid,
    number: u64,
    /// The proving tip it merged onto, which its push requires.
    tip: String,
    /// The worker's branch head it merged: moved since, it lands again.
    head: String,
    merged: String,
    message: String,
    /// Records taken from proving's copy, rewritten by the gate.
    regenerated: Vec<String>,
}

impl App {
    /// The tick's pass over the lines: each move that ran beside the ticks
    /// and ended, finished; each checkout's landing, then each one's
    /// promotion and cut as its line says each is due, but none of a
    /// checkout whose move runs on; then its local base following origin's
    /// as last fetched, whatever left it behind. Answers what landed, and
    /// what else moved.
    pub(crate) async fn lines(&self, checkouts: &[Checkout]) -> Answer<(Vec<Value>, Vec<Value>)> {
        let (mut landed, mut lines) = (Vec::new(), Vec::new());
        // Checkouts whose landing ended this tick: their next lands on a later one, one a tick.
        let mut landing_ended = Vec::new();
        for (of, ended) in self.beside(|f| matches!(f, For::Line(_)), false, false).await? {
            let For::Line(moving) = of else { continue };
            let into = if matches!(moving.then, Then::Land { .. }) {
                landing_ended.push(moving.checkout.clone());
                &mut landed
            } else {
                &mut lines
            };
            into.extend(self.moved(*moving, ended).await?.ok());
        }
        let free = |c: &&Checkout| !self.beside_runs(|f| f.moves(&c.name));
        for c in checkouts.iter().filter(free).filter(|c| !landing_ended.contains(&c.name)) {
            landed.extend(self.carry(c, Move::Land, false).await?.ok());
        }
        for c in checkouts {
            for mv in [Move::Promote, Move::Cut] {
                if free(&c) {
                    lines.extend(self.carry(c, mv, false).await?.ok());
                }
            }
            lines.extend(follow_base(c, &self.host.git(&c.path), &c.fetched_base()));
        }
        Ok((landed, lines))
    }

    /// Finishes `moving`, whose steps ran beside the ticks and `ended` so: unqueued while they
    /// ran, it is due again.
    async fn moved(&self, moving: Moving, ended: Result<Vec<Ran>, Ends>) -> Answer<Did> {
        let c = self.checkout(&moving.checkout).await?;
        let Ok(rans) = ended else { return Ok(Err(format!("a move of {} was unqueued while it ran", c.name))) };
        match moving.then {
            Then::Land { landing, dir } => {
                let (ran, here) = (rans.into_iter().next().expect("one step"), self.host.git(&dir));
                let gated = self.gate_ran(&c.name, &here, &landing.merged, false, Kind::Fast, ran).await?;
                self.land_gated(&c, &here, landing, gated).await
            }
            Then::Promote { tip, base, dir, _lock } => {
                let ran = rans.into_iter().next().expect("one step");
                let gated = self.gate_ran(&c.name, &self.host.git(&dir), &tip, false, Kind::Slow, ran).await?;
                self.promote_gated(&c, &self.host.git(&c.path), tip, base, gated).await
            }
            Then::Cut { version, base, dir, steps } => {
                self.cut_ran(&c, &self.host.git(&dir), &version, &base, &steps, rans).await
            }
        }
    }

    /// Carries out `mv` on `c` if its line says it is due now.
    async fn carry(&self, c: &Checkout, mv: Move, by_hand: bool) -> Answer<Did> {
        let l = self.state::<Lines>(line_id(&c.name)).await?;
        let settings = self.settings().await?;
        let workers: Vec<_> = self
            .live_workers()
            .await?
            .into_iter()
            .filter(|w| w.checkout == c.name)
            .map(|w| (w.worker, w.state.status))
            .collect();
        let look = line::look(self.now(), &settings, &workers, by_hand);
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
            Ok(Due::Land(worker)) => self.land(c, worker).await,
            Ok(Due::Promote(tip)) => self.promote_tip(c, &git, tip, by_hand, bound_at).await,
            Ok(Due::Cut) => self.cut_base(c, by_hand, bound_at).await,
        }
    }

    /// Lands `worker` onto the proving tip of `c`.
    async fn land(&self, c: &Checkout, worker: Uuid) -> Answer<Did> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.clone() else { return Ok(Err("the worker never started".into())) };
        let git = self.host.git(&c.path);
        let tip = git.fetch(&c.proving).map_err(fail("git"))?;
        let here = git.fresh(&self.data().clone_of(LineClone::Landing, &c.name), &tip).map_err(fail("git"))?;
        let issue = self.issue(&c.name, st.number).await?;
        let message = format!("Land {}#{}: {}", c.name, st.number, issue.title);
        // The worker's branch lives in the checkout, not in this clone. Conflicts only in the
        // records the gate regenerates take proving's copy, rewritten by the gate below (#225).
        let records = self.declared(&here, &tip)?.gate.regenerated;
        // Read before the merge: a branch that moves after it is merged again, never pushed unseen.
        let head = git.sha(&st.branch).map_err(fail("git"))?;
        let (merged, regenerated) = match here.merge_from(Path::new(&c.path), &st.branch, &message, &records) {
            Ok(merged) => merged,
            Err(e) => {
                // Proving moved since the last red's tip, which the branch now holds (#171).
                let moved = wk
                    .red_tip
                    .as_deref()
                    .filter(|p1| *p1 != tip && git.is_ancestor(p1, &tip) && git.is_ancestor(p1, &st.branch));
                let since = match moved.map(|p1| (p1, git.landings(p1, &tip))) {
                    Some((p1, Ok(landed))) => format!(" Proving moved since {p1}: {}.", landed.join("; ")),
                    _ => String::new(),
                };
                let why = format!(
                    "your branch does not merge onto the proving tip {tip}: {e}.{since} Merge or rebase onto it and resolve."
                );
                let red = w::Red {
                    worker,
                    why,
                    tip: Some(tip.clone()),
                    moved: moved.is_some(),
                    moved_max: self.settings().await?.get(Setting::MovedRedsMax) as u32,
                };
                return self.red(red).await.map(Ok);
            }
        };
        let landing = Landing { worker, number: st.number, tip, head, merged, message, regenerated };
        let then = Then::Land { landing: landing.clone(), dir: here.dir.clone() };
        let moving = Box::new(Moving { checkout: c.name.clone(), then });
        let at = At::Beside { rev: &landing.merged, bound_at: None, moving };
        let Some(gated) = self.gate(&c.name, &here, at, Kind::Fast).await? else { return Ok(Err(QUEUED.into())) };
        self.land_gated(c, &here, landing, gated).await
    }

    /// Lands `landing` in the clone `here` as its fast gate says: red, the worker is told; green,
    /// proving is pushed to the merge. Not once the worker no longer asks to land, or its branch
    /// moved since the merge: it lands again on a later tick.
    async fn land_gated(&self, c: &Checkout, here: &Git, landing: Landing, gated: Gated) -> Answer<Did> {
        let Landing { worker, number, tip, head, merged, message, regenerated } = landing;
        let wk = self.state::<Workers>(worker).await?;
        if wk.status != w::Status::Landing {
            return Ok(Err(format!("worker {worker} no longer asks to land")));
        }
        let branch = wk.started.as_ref().map(|st| st.branch.as_str()).unwrap_or_default();
        if self.host.git(&c.path).sha(branch).ok().as_deref() != Some(head.as_str()) {
            return Ok(Err(format!("the branch of worker {worker} moved while its gate ran: it lands again")));
        }
        if !gated.green {
            let why = format!("the fast gate is red on your branch merged onto the proving tip:\n{}", gated.why());
            return self.red(w::Red { worker, why, tip: None, moved: false, moved_max: 0 }).await.map(Ok);
        }
        let merged = if regenerated.is_empty() {
            merged
        } else {
            let message = format!(
                "{message}\n\nRegenerated by the fast gate on the merge, from proving's copy: {}",
                regenerated.join(", ")
            );
            here.amend(&regenerated, &message).map_err(fail("git"))?
        };
        if let Err(e) = here.push_requiring(&merged, &c.proving, Some(&tip)) {
            // The tip moved since it was read: land again on the new tip.
            return Ok(Ok(json!({ "worker": worker, "retry": e })));
        }
        self.submit::<line::RecordLanding>(line::Landed { checkout: c.name.clone(), number, commit: merged.clone() })
            .await?;
        self.end_worker(worker, End::Landed(merged.clone())).await?;
        Ok(Ok(json!({ "landed": format!("{}#{}", c.name, number), "commit": merged })))
    }

    async fn red(&self, red: w::Red) -> Answer<Value> {
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

    /// Promotes the proving `tip` of `c`: its slow gate, then the base
    /// fast-forwarded to it. `bound_at`: when the promotion falls due however busy.
    async fn promote_tip(
        &self,
        c: &Checkout,
        git: &Git,
        tip: String,
        by_hand: bool,
        bound_at: Option<i64>,
    ) -> Answer<Did> {
        // One promotion of a checkout at a time: a second would reset the
        // clone under the running one's slow gate.
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
        let here = git.fresh(&self.data().clone_of(LineClone::Promotion, &c.name), &tip).map_err(fail("git"))?;
        let at = if by_hand {
            At::Commit { rev: &tip, bound_at }
        } else {
            let (dir, _lock) = (here.dir.clone(), running.clone());
            let then = Then::Promote { tip: tip.clone(), base: base.clone(), dir, _lock };
            let moving = Moving { checkout: c.name.clone(), then };
            At::Beside { rev: &tip, bound_at, moving: Box::new(moving) }
        };
        let Some(gated) = self.gate(&c.name, &here, at, Kind::Slow).await? else { return Ok(Err(QUEUED.into())) };
        self.promote_gated(c, git, tip, base, gated).await
    }

    /// Promotes the proving `tip` of `c` over `base` as its slow gate says: red, it is recorded
    /// red; green, the base is fast-forwarded to it.
    async fn promote_gated(&self, c: &Checkout, git: &Git, tip: String, base: String, gated: Gated) -> Answer<Did> {
        let behind = git.is_ancestor(&base, &tip);
        let red = (!gated.green).then(|| gated.why());
        let promotion = line::Promotion { checkout: c.name.clone(), tip: tip.clone(), base: base.clone(), red, behind };
        if let Some(why) = promotion.red() {
            self.submit::<line::Promote>(promotion).await?;
            return Ok(Ok(json!({ "promotion": c.name, "red": why })));
        }
        if let Err(e) = git.push_requiring(&tip, &c.base, Some(&base)) {
            return Ok(Ok(json!({ "promotion": c.name, "retry": e })));
        }
        self.submit::<line::Promote>(promotion).await?;
        let local = follow_base(c, git, &tip);
        Ok(Ok(json!({ "promoted": c.name, "tip": tip, "local": local })))
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
