//! The proving line's three moves: a landing merges a worker's branch onto
//! the proving tip and pushes it once the fast gate is green; a promotion
//! fast-forwards the base branch to the proving tip once the slow gate is
//! green; a cut makes a version from the base tip. Each push requires the
//! tip it read. Each runs in its own clone of the checkout under the data directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use factorium_domain::checkout::Checkout;
use factorium_domain::declared::Kind;
use factorium_domain::gate::{Gates, Ran as GateRan, RecordGate, record_id};
use factorium_domain::issue::{Because, CloseIssue};
use factorium_domain::line::{self, Lines, next_version};
use factorium_domain::settings::Time;
use factorium_domain::worker::{self as w, Status, Workers};
use factorium_domain::{id, line_id};
use factorium_host::Git;
use factum::Uuid;
use serde_json::{Value, json};

use crate::{Answer, App, Refusal, fail};

/// What one move did, for the tick's answer.
pub(crate) type Did = Option<Value>;

/// Puts a clone of the checkout at `path` on `rev`, keeping its ignored
/// files (builds). A work dir made as a worktree of the checkout before is
/// removed and made again as a clone.
fn fresh(git: &Git, path: &Path, rev: &str) -> Result<Git, String> {
    let here = Git::at(path);
    if path.join(".git").is_file() {
        git.worktree_remove(path)?;
    }
    if path.join(".git").is_dir() {
        here.run(&["checkout", "--quiet", "--detach", "--force", rev])?;
        here.run(&["clean", "-fdq"])?;
    } else {
        std::fs::create_dir_all(path.parent().unwrap_or(path)).map_err(|e| e.to_string())?;
        git.clone_shared(path, rev)?;
    }
    Ok(here)
}

impl App {
    /// Runs the `kind` gate on `rev` in `here`, unless that tree is green
    /// already; answers whether it is green and the tail of a red run.
    /// `Ok(None)`: the queue has no room yet; ask again next tick.
    pub(crate) async fn gate_tree(
        &self,
        checkout: &Checkout,
        here: &Git,
        rev: &str,
        kind: Kind,
        queued: Uuid,
    ) -> Answer<Option<(bool, String)>> {
        let tree = here.tree(rev).map_err(fail("git"))?;
        if self.state::<Gates>(record_id(&checkout.name, &tree, kind)).await?.green {
            return Ok(Some((true, String::new())));
        }
        let declared = self.declared(here, rev)?;
        let Some((cmd, limit)) = declared.gate.of(kind) else { return Ok(Some((true, String::new()))) };
        let what = format!("{} gate of {}", kind.as_str(), checkout.name);
        if self.heavy_try(queued, &what, None).await?.is_err() {
            return Ok(None);
        }
        let (dir, cmd2) = (here.dir.clone(), cmd.to_string());
        let ran = tokio::task::spawn_blocking(move || {
            factorium_host::run(&dir, &cmd2, &[], Duration::from_secs(limit), &|| false)
        })
        .await
        .map_err(|e| Refusal::new("internal", e.to_string()))?;
        self.heavy_end(queued).await?;
        let record = GateRan {
            checkout: checkout.name.clone(),
            tree,
            kind,
            green: ran.ok,
            took_ms: ran.took_ms,
            tail: ran.tail.clone(),
        };
        self.submit::<RecordGate>(record).await?;
        let why = if ran.timed_out { format!("past its limit of {limit}s\n{}", ran.tail) } else { ran.tail };
        Ok(Some((ran.ok, why)))
    }

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
        let git = Git::at(&c.path);
        let tip = git.fetch(&c.proving).map_err(fail("git"))?;
        let here = fresh(&git, &self.dir(&["landing", &c.name]), &tip).map_err(fail("git"))?;
        let issue = self.issue(&c.name, st.number).await?;
        let message = format!("Land {}#{}: {}", c.name, st.number, issue.title);
        // The worker's branch lives in the checkout, not in this clone.
        let branch = format!("+refs/heads/{0}:refs/heads/{0}", st.branch);
        let merged = here
            .run(&["fetch", "--quiet", &c.path, &branch])
            .and_then(|_| here.run(&["merge", "--no-ff", "--no-edit", "-m", &message, &st.branch]));
        if let Err(e) = merged {
            let _ = here.run(&["merge", "--abort"]);
            let why = format!(
                "your branch does not merge onto the proving tip {tip}: {e}. Merge or rebase onto it and resolve."
            );
            return self.red(worker, &st.terminal, why).await.map(Some);
        }
        let merged = here.sha("HEAD").map_err(fail("git"))?;
        let queued = id(&format!("landing:{worker}:{merged}"));
        let Some((green, tail)) = self.gate_tree(&c, &here, &merged, Kind::Fast, queued).await? else {
            return Ok(None);
        };
        if !green {
            let why = format!("the fast gate is red on your branch merged onto the proving tip:\n{tail}");
            return self.red(worker, &st.terminal, why).await.map(Some);
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

    async fn red(&self, worker: Uuid, terminal: &str, why: String) -> Answer<Value> {
        self.submit::<w::RedLanding>(w::Because { worker, why: why.clone() }).await?;
        let wk = self.state::<Workers>(worker).await?;
        if wk.status == Status::Working {
            let text = format!(
                "factorium: your landing was red. {why}\nFix it, commit, and run `factorium worker done` again."
            );
            let _ = self.host.orca().send(terminal, &one_line(&text));
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
        let git = Git::at(&c.path);
        let kept = git.fetch(&c.proving).ok().is_some_and(|tip| git.ahead(&tip, &st.branch).unwrap_or(1) > 0);
        if remove && !kept {
            let path = PathBuf::from(&st.worktree);
            self.host.orca().remove(&path).map_err(fail("orca"))?;
            let _ = git.worktree_remove(&path);
            let _ = git.run(&["branch", "-D", &st.branch]);
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
        let git = Git::at(&c.path);
        let tip = git.fetch(&c.proving).map_err(fail("git"))?;
        if l.red.as_ref().is_some_and(|(t, _)| *t == tip) && !force {
            return Ok(None);
        }
        let base = git.fetch(&c.base).map_err(fail("git"))?;
        let here = fresh(&git, &self.dir(&["promotion", &c.name]), &tip).map_err(fail("git"))?;
        let queued = id(&format!("promotion:{}:{tip}", c.name));
        let Some((green, tail)) = self.gate_tree(c, &here, &tip, Kind::Slow, queued).await? else { return Ok(None) };
        let red = |why: String| line::Tip { checkout: c.name.clone(), tip: tip.clone(), why };
        if !green {
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
        let git = Git::at(&c.path);
        let base = git.fetch(&c.base).map_err(fail("git"))?;
        let _ = git.run(&["fetch", "--quiet", "--tags", "origin"]);
        let declared = self.declared(&git, &base)?;
        let Some(cut) = declared.cut else { return Ok(None) };
        let version = next_version(git.version_tags().first().map(String::as_str));
        let here = fresh(&git, &self.dir(&["cut", &c.name]), &base).map_err(fail("git"))?;
        let env = vec![("FACTORIUM_VERSION".to_string(), version.clone())];
        let fail_cut =
            |why: String| line::Cut { checkout: c.name.clone(), version: version.clone(), commit: base.clone(), why };
        for step in &cut.steps {
            let queued = id(&format!("cut:{}:{version}:{step}", c.name));
            while self.heavy_try(queued, &format!("cut {version} of {}", c.name), None).await?.is_err() {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            let (dir, s, e) = (here.dir.clone(), step.clone(), env.clone());
            let limit = Duration::from_secs(cut.limit);
            let ran = tokio::task::spawn_blocking(move || factorium_host::run(&dir, &s, &e, limit, &|| false))
                .await
                .map_err(|e| Refusal::new("internal", e.to_string()))?;
            self.heavy_end(queued).await?;
            if !ran.ok {
                let why = format!("step `{step}` failed:\n{}", ran.tail);
                self.submit::<line::FailCut>(fail_cut(why.clone())).await?;
                return Ok(Some(json!({ "cut": c.name, "version": version, "failed": why })));
            }
        }
        let tagged = here.sha("HEAD").map_err(fail("git"))?;
        let tag = here
            .run(&["tag", "-a", &version, "-m", &version, &tagged])
            .and_then(|_| here.run(&["push", "--quiet", "origin", &format!("refs/tags/{version}")]));
        if let Err(e) = tag {
            self.submit::<line::FailCut>(fail_cut(format!("tagging failed: {e}"))).await?;
            return Ok(Some(json!({ "cut": c.name, "version": version, "failed": e })));
        }
        let (mut installed, mut restart) = (None, None);
        if let Some(formula) = &cut.formula {
            // The daemon's own formula: it runs the new binary once the tick ends.
            restart = self.host.binary_of(formula);
            if let Err(e) = self.host.brew_upgrade(formula, &version) {
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

    /// `factorium promote`: promotes now, whatever the bound says.
    pub async fn promote(&self, checkout: &str) -> Answer {
        let c = self.checkout(checkout).await?;
        Ok(self
            .promote_if_due(&c, true, true)
            .await?
            .unwrap_or(json!({ "promoted": null, "why": "nothing to promote" })))
    }

    /// `factorium cut`: cuts now, whatever the bound says.
    pub async fn cut(&self, checkout: &str) -> Answer {
        let c = self.checkout(checkout).await?;
        Ok(self
            .cut_if_due(&c, true, true)
            .await?
            .unwrap_or(json!({ "cut": null, "why": "nothing promoted since the last cut, or no cut declared" })))
    }
}

/// Text typed at a terminal: one line.
pub(crate) fn one_line(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" | ")
}
