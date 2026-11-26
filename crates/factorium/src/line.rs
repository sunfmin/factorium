//! The proving line's three moves: a landing merges a worker's branch onto
//! the proving tip and pushes it once the fast gate is green; a promotion
//! fast-forwards the base branch to the proving tip once its gate is green —
//! the slow steps alone on a tip a landing gated green, else the fast gate
//! then the slow steps; a cut makes a version from the base tip ([`cut`]). Each push
//! requires the tip it read. Each runs in its own clone of the checkout
//! under the data directory. The daemon runs every heavy step of a move — a
//! landing's fast gate, a promotion's slow gate, a cut's steps — beside its
//! ticks ([`Moving`]), so no tick waits for one (factorium#231): the next
//! tick finds how they ended and finishes the move. A checkout's moves go one at a time: none
//! of its moves is carried while one of them runs beside the ticks. A landing waiting its turn
//! is checked for its merge onto the proving tip on the tick after `done`, and again each time
//! its branch or the tip moves: one that does not merge is red then, not at its turn
//! (factorium#307).

mod cut;
mod mutants;

pub(crate) use cut::Proving;

use std::path::Path;

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::{Gating, Kind};
use factorium_domain::doctor::Stuck;
use factorium_domain::heavy::{Ends, Heavy, queue_id};
use factorium_domain::line::{self, Due, Lines, Move, NotDue};
use factorium_domain::line_id;
use factorium_domain::outbox::Sender;
use factorium_domain::settings::Setting;
use factorium_domain::worker::{self as w, Workers};
use factorium_host::{Git, Lag, NoMerge, Ran};
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

impl Moving {
    /// The worker it lands, if it is a landing.
    pub(crate) fn lands(&self) -> Option<Uuid> {
        match &self.then {
            Then::Land { landing, .. } => Some(landing.worker),
            _ => None,
        }
    }
}

enum Then {
    /// The landing of a worker's branch, merged in the clone `dir`.
    Land { landing: Landing, dir: std::path::PathBuf },
    /// The promotion of the proving `tip` over `base`, its gate run in the clone `dir` as
    /// `gating` says, holding the checkout's promotion lock.
    Promote { tip: String, base: String, gating: Gating, dir: std::path::PathBuf, _lock: std::sync::Arc<std::fs::File> },
    /// The cut of `version` from `base`, its `steps` run in the clone `dir`.
    Cut { version: String, base: String, dir: std::path::PathBuf, steps: Vec<String> },
}

/// A worker's branch merged onto the proving tip, its fast gate to run.
#[derive(Clone)]
struct Landing {
    worker: Uuid,
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
        lines.extend(self.cuts_proven().await?);
        // After a landing pushed, before the next lands: one that no longer merges leaves the line.
        landed.extend(self.check_waiting(checkouts).await?);
        let free = |c: &&Checkout| !self.beside_runs(|f| f.moves(&c.name)) && !self.cut_proving(&c.name);
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
        let Ok(rans) = ended else {
            return Ok(Err(format!("a move of {} ended while it ran: {}", c.name, ended.unwrap_err())));
        };
        match moving.then {
            Then::Land { landing, dir } => {
                let (ran, here) = (rans.into_iter().next().expect("one step"), self.host.git(&dir));
                let gated = self.gate_ran(&c.name, &here, &landing.merged, false, Kind::Fast, ran).await?;
                self.land_gated(&c, &here, landing, gated).await
            }
            Then::Promote { tip, base, gating, dir, _lock } => {
                let ran = rans.into_iter().next().expect("one step");
                let gated = self.gate_ran(&c.name, &self.host.git(&dir), &tip, false, Kind::Slow, ran).await?;
                self.promote_gated(&c, &self.host.git(&c.path), tip, base, &gating, gated).await
            }
            Then::Cut { version, base, dir, steps } => {
                self.cut_ran(&c, &self.host.git(&dir), &version, &base, &steps, rans, true).await
            }
        }
    }

    /// Carries out `mv` on `c` if its line says it is due now.
    async fn carry(&self, c: &Checkout, mv: Move, by_hand: bool) -> Answer<Did> {
        let l = self.state::<Lines>(line_id(&c.name)).await?;
        let settings = self.settings().await?;
        let mut workers = Vec::new();
        for w in self.live_workers().await?.into_iter().filter(|w| w.checkout == c.name) {
            let urgent = w.state.status == w::Status::Landing && self.issue(&w.checkout, w.number).await?.urgent;
            workers.push(line::Seen { worker: w.worker, number: w.number, status: w.state.status, urgent });
        }
        // An urgent issue's halt (docs/adr/0015-an-urgent-issue-halts-the-host.md) holds no line move: its gate waits in the queue as usual.
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
            Ok(Due::Land(worker)) => {
                let did = self.land(c, worker).await?;
                // Urgent, it went ahead of the landings that asked before it: said so.
                let first = workers.iter().find(|w| w.status == w::Status::Landing).map(|w| w.worker);
                if first == Some(worker) {
                    return Ok(did);
                }
                let named = |n: u64| format!("{}#{n}", c.name);
                let ahead = workers.iter().take_while(|w| w.worker != worker);
                let ahead_of: Vec<_> =
                    ahead.filter(|w| w.status == w::Status::Landing && !w.urgent).map(|w| named(w.number)).collect();
                let went = json!({ "ahead_of": ahead_of });
                Ok(Ok(match did {
                    Ok(mut v) => {
                        v["urgent"] = went;
                        v
                    }
                    Err(why) => {
                        json!({ "worker": worker, "lands": workers.iter().find(|w| w.worker == worker).map(|w| named(w.number)), "urgent": went, "then": why })
                    }
                }))
            }
            Ok(Due::Promote(tip)) => self.promote_tip(c, &git, tip, by_hand, bound_at).await,
            Ok(Due::Cut) => self.cut_base(c, by_hand, bound_at).await,
        }
    }

    /// Checks each landing that waits its turn (asked, its merge not running beside the ticks)
    /// for its merge onto its checkout's proving tip, unless its branch and the tip are as when
    /// it last merged: red now, each that does not merge (factorium#307).
    async fn check_waiting(&self, checkouts: &[Checkout]) -> Answer<Vec<Value>> {
        let live = self.live_workers().await?;
        let asks: Vec<_> = live.iter().filter(|w| w.state.status == w::Status::Landing).collect();
        let lands = |f: &For, w: Uuid| matches!(f, For::Line(m) if matches!(&m.then, Then::Land { landing, .. } if landing.worker == w));
        let running = |w: Uuid| self.beside_runs(|f| lands(f, w));
        let mut reds = Vec::new();
        for c in checkouts {
            let mut waiting = asks.iter().filter(|a| a.checkout == c.name && !running(a.worker)).peekable();
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
    async fn not_merging(&self, git: &Git, worker: Uuid, wk: &w::Worker, tip: &str, e: NoMerge) -> Answer<Value> {
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
            Err(e) => return self.not_merging(&git, worker, &wk, &tip, e).await.map(Ok),
        };
        let landing = Landing { worker, tip, head, merged, message, regenerated };
        let then = Then::Land { landing: landing.clone(), dir: here.dir.clone() };
        let moving = Box::new(Moving { checkout: c.name.clone(), then });
        let at = At::Beside { rev: &landing.merged, bound_at: None, moving };
        let Some(gated) = self.gate(&c.name, &here, at, Gating::Fast).await? else { return Ok(Err(QUEUED.into())) };
        self.land_gated(c, &here, landing, gated).await
    }

    /// Lands `landing` in the clone `here` as its fast gate says: red, the worker is told; green,
    /// proving is pushed to the merge. Not once the worker no longer asks to land, or its branch
    /// moved since the merge: it lands again on a later tick.
    async fn land_gated(&self, c: &Checkout, here: &Git, landing: Landing, gated: Gated) -> Answer<Did> {
        let Landing { worker, tip, head, merged, message, regenerated } = landing;
        let wk = self.state::<Workers>(worker).await?;
        if wk.status != w::Status::Landing {
            return Ok(Err(format!("worker {worker} no longer asks to land")));
        }
        let (branch, number) = wk.started.as_ref().map(|st| (st.branch.as_str(), st.number)).unwrap_or_default();
        if self.host.git(&c.path).sha(branch).ok().as_deref() != Some(head.as_str()) {
            return Ok(Err(format!("the branch of worker {worker} moved while its gate ran: it lands again")));
        }
        if !gated.green {
            let why = format!("the fast gate is red on your branch merged onto the proving tip:\n{}", gated.why());
            let kind = match &gated.ran {
                Some(ran) if ran.timed_out => w::RedKind::PastLimit,
                _ if !regenerated.is_empty() => w::RedKind::Regenerated,
                _ => w::RedKind::GateRed,
            };
            let red = w::Red { worker, why, kind, files: regenerated, tip: None, moved: false, moved_max: 0 };
            return self.red(red).await.map(Ok);
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
        let line = self.state::<Lines>(line_id(&c.name)).await?;
        let gating = match line.unpromoted.iter().find(|l| l.commit == tip) {
            Some(l) => Gating::SlowOnly { by: format!("{}#{}", c.name, l.number) },
            None => Gating::Full,
        };
        let here = git.fresh(&self.data().clone_of(LineClone::Promotion, &c.name), &tip).map_err(fail("git"))?;
        // Its mutants first, where slimfit holds them at the base; by hand, whatever they owe.
        if !by_hand && let Some(did) = self.mutants_first(c, &here, &tip).await? {
            return Ok(did);
        }
        let at = if by_hand {
            At::Commit { rev: &tip, bound_at }
        } else {
            let (dir, _lock) = (here.dir.clone(), running.clone());
            let then = Then::Promote { tip: tip.clone(), base: base.clone(), gating: gating.clone(), dir, _lock };
            let moving = Moving { checkout: c.name.clone(), then };
            At::Beside { rev: &tip, bound_at, moving: Box::new(moving) }
        };
        let Some(gated) = self.gate(&c.name, &here, at, gating.clone()).await? else { return Ok(Err(QUEUED.into())) };
        self.promote_gated(c, git, tip, base, &gating, gated).await
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
