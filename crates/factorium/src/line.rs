//! The proving line's three moves: a landing merges a worker's branch onto
//! the proving tip and pushes it once the fast gate is green; a promotion
//! fast-forwards the base branch to the proving tip once the slow gate is
//! green; a cut makes a version from the base tip. Each push requires the
//! tip it read. Each runs in its own clone of the checkout under the data directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::Kind;
use factorium_domain::issue::{Because, CloseIssue};
use factorium_domain::line::{self, Lines, next_version};
use factorium_domain::line_id;
use factorium_domain::settings::Time;
use factorium_domain::worker::{self as w, Status, Workers};
use factum::Uuid;
use serde_json::{Value, json};

use crate::gate::At;
use crate::heavy::{Job, Turn};
use crate::{Answer, App, fail};

/// What one move did, for the tick's answer.
pub(crate) type Did = Option<Value>;

impl App {
    /// Lands the worker `worker`: answers what happened, or `None` when it
    /// must wait for another tick.
    pub(crate) async fn land(&self, worker: Uuid) -> Answer<Did> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.clone() else { return Ok(None) };
        if wk.status != Status::Landing {
            return Ok(None);
        }
        let c = self.checkout(&st.checkout).await?;
        if self.state::<Lines>(line_id(&c.name)).await?.held.is_some() {
            return Ok(None);
        }
        let git = self.host.git(&c.path);
        let tip = git.fetch(&c.proving).map_err(fail("git"))?;
        let here = git.fresh(&self.dir(&["landing", &c.name]), &tip).map_err(fail("git"))?;
        let issue = self.issue(&c.name, st.number).await?;
        let message = format!("Land {}#{}: {}", c.name, st.number, issue.title);
        // The worker's branch lives in the checkout, not in this clone.
        let merged = match here.merge_from(Path::new(&c.path), &st.branch, &message) {
            Ok(sha) => sha,
            Err(e) => {
                let why = format!(
                    "your branch does not merge onto the proving tip {tip}: {e}. Merge or rebase onto it and resolve."
                );
                return self.red(worker, why).await.map(Some);
            }
        };
        let at = At::Commit { rev: &merged, by_hand: false };
        let Some(gated) = self.gate(&c.name, &here, at, Kind::Fast).await? else { return Ok(None) };
        if !gated.green {
            let why = format!("the fast gate is red on your branch merged onto the proving tip:\n{}", gated.why());
            return self.red(worker, why).await.map(Some);
        }
        if let Err(e) = here.push_requiring(&merged, &c.proving, Some(&tip)) {
            // The tip moved since it was read: land again on the new tip.
            return Ok(Some(json!({ "worker": worker, "retry": e })));
        }
        self.submit::<w::GreenLanding>(w::Land { worker, commit: merged.clone() }).await?;
        self.submit::<line::RecordLanding>(line::Landed {
            checkout: c.name.clone(),
            number: st.number,
            commit: merged.clone(),
        })
        .await?;
        let reason = format!("landed as {merged}");
        self.submit::<CloseIssue>(Because { checkout: c.name.clone(), number: st.number, reason }).await?;
        self.release_worker(worker, true).await?;
        Ok(Some(json!({ "landed": format!("{}#{}", c.name, st.number), "commit": merged })))
    }

    async fn red(&self, worker: Uuid, why: String) -> Answer<Value> {
        self.submit::<w::RedLanding>(w::Because { worker, why: why.clone() }).await?;
        let wk = self.state::<Workers>(worker).await?;
        if wk.status == Status::Working {
            let text = format!(
                "factorium: your landing was red. {why}\nFix it, commit, and run `factorium worker done` again."
            );
            if let Some(st) = &wk.started {
                let _ = self.type_at(&Self::worker_session(st), &one_line(&text));
            }
        }
        Ok(json!({ "worker": worker, "red": why, "status": wk.status.as_str() }))
    }

    /// Ends a worker's session; its worktree goes unless it holds commits
    /// that are nowhere else (or `keep`).
    pub(crate) async fn release_worker(&self, worker: Uuid, remove: bool) -> Answer<()> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started else { return Ok(()) };
        self.host.orca().close(&st.terminal);
        let c = self.checkout(&st.checkout).await?;
        let git = self.host.git(&c.path);
        let kept = git.fetch(&c.proving).ok().is_some_and(|tip| git.ahead(&tip, &st.branch).unwrap_or(1) > 0);
        if remove && !kept {
            let path = PathBuf::from(&st.worktree);
            self.host.orca().remove(&path).map_err(fail("orca"))?;
            git.forget(&path, &st.branch);
        }
        Ok(())
    }

    /// Promotes `checkout` if it is due: the slow gate on the proving tip,
    /// then the base fast-forwarded to it.
    pub(crate) async fn promote_if_due(&self, c: &Checkout, idle: bool, force: bool) -> Answer<Did> {
        let l = self.state::<Lines>(line_id(&c.name)).await?;
        let Some(oldest) = l.unpromoted.first() else { return Ok(None) };
        let bound = self.settings().await?.ms(Time::PromotionBound);
        if !(force || idle || self.now() - oldest.at >= bound) {
            return Ok(None);
        }
        let git = self.host.git(&c.path);
        let tip = git.fetch(&c.proving).map_err(fail("git"))?;
        if l.red.as_ref().is_some_and(|(t, _)| *t == tip) && !force {
            return Ok(None);
        }
        let base = git.fetch(&c.base).map_err(fail("git"))?;
        let here = git.fresh(&self.dir(&["promotion", &c.name]), &tip).map_err(fail("git"))?;
        let at = At::Commit { rev: &tip, by_hand: force };
        let Some(gated) = self.gate(&c.name, &here, at, Kind::Slow).await? else { return Ok(None) };
        let red = |why: String| line::Tip { checkout: c.name.clone(), tip: tip.clone(), why };
        if !gated.green {
            let tail = gated.why();
            self.submit::<line::RedPromotion>(red(tail.clone())).await?;
            return Ok(Some(json!({ "promotion": c.name, "red": tail })));
        }
        if !git.is_ancestor(&base, &tip) {
            let why = format!("{} is not an ancestor of the proving tip; the base moved outside factorium", c.base);
            self.submit::<line::RedPromotion>(red(why.clone())).await?;
            return Ok(Some(json!({ "promotion": c.name, "red": why })));
        }
        if let Err(e) = git.push_requiring(&tip, &c.base, Some(&base)) {
            return Ok(Some(json!({ "promotion": c.name, "retry": e })));
        }
        self.submit::<line::RecordPromotion>(red(String::new())).await?;
        Ok(Some(json!({ "promoted": c.name, "tip": tip })))
    }

    /// Cuts a version of `checkout` if it is due and it declares a cut.
    pub(crate) async fn cut_if_due(&self, c: &Checkout, idle: bool, force: bool) -> Answer<Did> {
        let l = self.state::<Lines>(line_id(&c.name)).await?;
        let Some(oldest) = l.uncut.first() else { return Ok(None) };
        if l.cut_failed.is_some() && !force {
            return Ok(None);
        }
        let bound = self.settings().await?.ms(Time::CutBound);
        if !(force || (idle && l.unpromoted.is_empty()) || self.now() - oldest >= bound) {
            return Ok(None);
        }
        let git = self.host.git(&c.path);
        let base = git.fetch(&c.base).map_err(fail("git"))?;
        let declared = self.declared(&git, &base)?;
        let Some(cut) = declared.cut else { return Ok(None) };
        let version = next_version(git.version_tags().first().map(String::as_str));
        let here = git.fresh(&self.dir(&["cut", &c.name]), &base).map_err(fail("git"))?;
        let env = vec![("FACTORIUM_VERSION".to_string(), version.clone())];
        let fail_cut =
            |why: String| line::Cut { checkout: c.name.clone(), version: version.clone(), commit: base.clone(), why };
        // The steps share the clone, so they take one turn of the queue.
        let what = format!("cut {version} of {}", c.name);
        let job = Job {
            what,
            worker: None,
            dir: &here.dir,
            steps: &cut.steps,
            env: &env,
            limit: Duration::from_secs(cut.limit),
        };
        let turn = if force { Turn::Wait } else { Turn::Once(format!("cut:{}:{version}:{base}", c.name)) };
        let Some(rans) = self.heavy(job, turn).await? else { return Ok(None) };
        if let Some(ran) = rans.last().filter(|r| !r.ok) {
            let why = format!("step `{}` failed:\n{}", cut.steps[rans.len() - 1], ran.tail);
            self.submit::<line::FailCut>(fail_cut(why.clone())).await?;
            return Ok(Some(json!({ "cut": c.name, "version": version, "failed": why })));
        }
        let tagged = here.sha("HEAD").map_err(fail("git"))?;
        if let Err(e) = here.tag_pushed(&version, &tagged) {
            self.submit::<line::FailCut>(fail_cut(format!("tagging failed: {e}"))).await?;
            return Ok(Some(json!({ "cut": c.name, "version": version, "failed": e })));
        }
        let (mut installed, mut restart) = (None, None);
        if let Some(formula) = &cut.formula {
            // The daemon's own formula: it runs the new binary once the tick ends.
            restart = self.host.brew().binary_of(formula);
            if let Err(e) = self.host.brew().upgrade(formula, &version) {
                self.submit::<line::FailCut>(fail_cut(format!("installing {formula} failed: {e}"))).await?;
                return Ok(Some(json!({ "cut": c.name, "version": version, "failed": e })));
            }
            installed = Some(formula.clone());
        }
        let made = line::Cut {
            checkout: c.name.clone(),
            version: version.clone(),
            commit: tagged.clone(),
            why: String::new(),
        };
        self.submit::<line::RecordCut>(made).await?;
        Ok(Some(
            json!({ "cut": c.name, "version": version, "commit": tagged, "installed": installed, "restart": restart }),
        ))
    }

    /// `factorium promote`: promotes now, whatever the bound says. Forced, it
    /// waits its turn in the heavy queue, so `None` means only that nothing
    /// is unpromoted: "not yet" is never answered as "nothing to do".
    pub async fn promote(&self, checkout: &str) -> Answer {
        let c = self.checkout(checkout).await?;
        Ok(self
            .promote_if_due(&c, true, true)
            .await?
            .unwrap_or(json!({ "promoted": null, "why": "nothing to promote" })))
    }

    /// `factorium cut`: cuts now, whatever the bound says, waiting its turn
    /// in the heavy queue like `promote`.
    pub async fn cut(&self, checkout: &str) -> Answer {
        let c = self.checkout(checkout).await?;
        if let Some(did) = self.cut_if_due(&c, true, true).await? {
            return Ok(did);
        }
        let uncut = !self.state::<Lines>(line_id(&c.name)).await?.uncut.is_empty();
        let why = if uncut { "no cut declared" } else { "nothing promoted since the last cut" };
        Ok(json!({ "cut": null, "why": why }))
    }
}

/// Text typed at a terminal: one line.
pub(crate) fn one_line(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" | ")
}
