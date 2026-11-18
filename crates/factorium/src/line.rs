//! The proving line's three moves: a landing merges a worker's branch onto
//! the proving tip and pushes it once the fast gate is green; a promotion
//! fast-forwards the base branch to the proving tip once the slow gate is
//! green; a cut makes a version from the base tip ([`cut`]). Each push
//! requires the tip it read. Each runs in its own clone of the checkout
//! under the data directory.

mod cut;

use std::path::Path;

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::Kind;
use factorium_domain::doctor::Stuck;
use factorium_domain::line::{self, Due, Lines, Move};
use factorium_domain::line_id;
use factorium_domain::outbox::Sender;
use factorium_domain::worker::{self as w, Status, Workers};
use factorium_host::{Git, Lag};
use factum::Uuid;
use serde_json::{Value, json};

use crate::gate::At;
use crate::life::End;
use crate::{Answer, App, LineClone, fail};

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

/// The lock one promotion holds while it runs, released when dropped or
/// when its process ends, however it ends.
struct Running {
    _held: std::fs::File,
}

impl Running {
    /// Takes the lock at `path` as `what`, unless it is held: then answers
    /// what holds it.
    fn take(path: &Path, what: &str) -> Result<Running, String> {
        use std::io::{Read, Seek, Write};
        let opened = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path));
        let mut f = opened.map_err(|e| format!("its lock {} does not open: {e}", path.display()))?;
        match f.try_lock() {
            Ok(()) => {
                let _ = f.set_len(0).and_then(|()| f.rewind());
                let _ = write!(f, "{what}, pid {}", std::process::id());
                Ok(Running { _held: f })
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let mut held = String::new();
                let _ = f.read_to_string(&mut held);
                Err(if held.is_empty() { "another promotion".into() } else { held })
            }
            Err(std::fs::TryLockError::Error(e)) => Err(format!("its lock {} does not lock: {e}", path.display())),
        }
    }
}

/// What one move did, or why it did not run now.
pub(crate) type Did = Result<Value, String>;

/// Why a move that is due did not run this tick.
const QUEUED: &str = "waits its turn in the heavy-command queue";

impl App {
    /// The tick's pass over the lines: each checkout's landing, then each
    /// one's promotion and cut as its line says each is due, then its local
    /// base following origin's as last fetched, whatever left it behind.
    /// Answers what landed, and what else moved.
    pub(crate) async fn lines(&self, checkouts: &[Checkout]) -> Answer<(Vec<Value>, Vec<Value>)> {
        let (mut landed, mut lines) = (Vec::new(), Vec::new());
        for c in checkouts {
            landed.extend(self.carry(c, Move::Land, false).await?.ok());
        }
        for c in checkouts {
            for mv in [Move::Promote, Move::Cut] {
                lines.extend(self.carry(c, mv, false).await?.ok());
            }
            lines.extend(follow_base(c, &self.host.git(&c.path), &c.fetched_base()));
        }
        Ok((landed, lines))
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
        // The worker's branch lives in the checkout, not in this clone.
        let merged = match here.merge_from(Path::new(&c.path), &st.branch, &message) {
            Ok(sha) => sha,
            Err(e) => {
                let why = format!(
                    "your branch does not merge onto the proving tip {tip}: {e}. Merge or rebase onto it and resolve."
                );
                return self.red(worker, why).await.map(Ok);
            }
        };
        let at = At::Commit { rev: &merged, by_hand: false, bound_at: None };
        let Some(gated) = self.gate(&c.name, &here, at, Kind::Fast).await? else { return Ok(Err(QUEUED.into())) };
        if !gated.green {
            let why = format!("the fast gate is red on your branch merged onto the proving tip:\n{}", gated.why());
            return self.red(worker, why).await.map(Ok);
        }
        if let Err(e) = here.push_requiring(&merged, &c.proving, Some(&tip)) {
            // The tip moved since it was read: land again on the new tip.
            return Ok(Ok(json!({ "worker": worker, "retry": e })));
        }
        self.submit::<line::RecordLanding>(line::Landed {
            checkout: c.name.clone(),
            number: st.number,
            commit: merged.clone(),
        })
        .await?;
        self.end_worker(worker, End::Landed(merged.clone())).await?;
        Ok(Ok(json!({ "landed": format!("{}#{}", c.name, st.number), "commit": merged })))
    }

    async fn red(&self, worker: Uuid, why: String) -> Answer<Value> {
        self.submit::<w::RedLanding>(w::Because { worker, why: why.clone() }).await?;
        let wk = self.state::<Workers>(worker).await?;
        // A red that does not reach it sends it to the watch at once, not after its silence.
        if let (Status::Working, Some(st)) = (wk.status, &wk.started) {
            let text = format!("your landing was red. {why}\nFix it, commit, and run `factorium worker done` again.");
            self.tell(&Self::worker_session(worker, st)?, Sender::Factorium, &text, false).await?;
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
        let _running = match Running::take(&self.data().promotion_lock(&c.name), &format!("{who} of {tip}")) {
            Ok(running) => running,
            Err(running) => return Ok(Err(format!("a promotion of {} runs: {running}", c.name))),
        };
        let base = git.fetch(&c.base).map_err(fail("git"))?;
        let here = git.fresh(&self.data().clone_of(LineClone::Promotion, &c.name), &tip).map_err(fail("git"))?;
        let at = At::Commit { rev: &tip, by_hand, bound_at };
        let Some(gated) = self.gate(&c.name, &here, at, Kind::Slow).await? else { return Ok(Err(QUEUED.into())) };
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
