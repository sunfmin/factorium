//! The cut: a version from the base tip, its steps in one heavy turn, then
//! the tag and the formula's install; recorded once, done or failed.

use factorium_domain::checkout::Checkout;
use factorium_domain::heavy::LineJob;
use factorium_domain::line::{self, next_version};
use serde_json::{Value, json};

use super::{Did, QUEUED};
use crate::heavy::{Job, Turn};
use crate::{Answer, App, LineClone, fail};

impl App {
    /// Cuts a version of `c` from its base tip, if it declares a cut.
    /// `bound_at`: when the cut falls due however busy.
    pub(super) async fn cut_base(&self, c: &Checkout, by_hand: bool, bound_at: Option<i64>) -> Answer<Did> {
        let git = self.host.git(&c.path);
        let base = git.fetch(&c.base).map_err(fail("git"))?;
        let declared = self.declared(&git, &base)?;
        let Some(cut) = declared.cut else { return Ok(Err("the repo declares no cut".into())) };
        let version = next_version(git.version_tags().first().map(String::as_str));
        let here = git.fresh(&self.data().clone_of(LineClone::Cut, &c.name), &base).map_err(fail("git"))?;
        let record = |commit: &str, failed: Option<String>| line::Cut {
            checkout: c.name.clone(),
            version: version.clone(),
            commit: commit.into(),
            failed,
        };
        // The steps share the clone, so they take one turn of the queue.
        let job = Job::cut(LineJob::Cut.of(&c.name), &here.dir, &cut.steps, &version, cut.limit).of(None, bound_at);
        let turn = if by_hand { Turn::Wait } else { Turn::Once(format!("cut:{}:{version}:{base}", c.name)) };
        let Some(rans) = self.heavy(job, turn).await? else { return Ok(Err(QUEUED.into())) };
        // Each step after the queue, in turn; the first that fails is why the
        // cut failed, and every way it fails is recorded the one way.
        let done = async {
            if let Some(ran) = rans.last().filter(|r| !r.ok) {
                return Err(format!("step `{}` failed:\n{}", cut.steps[rans.len() - 1], ran.tail));
            }
            let tagged = here.sha("HEAD").and_then(|t| here.tag_pushed(&version, &t).map(|_| t));
            let tagged = tagged.map_err(|e| format!("tagging failed: {e}"))?;
            let Some(formula) = &cut.formula else { return Ok((tagged, None)) };
            // The daemon's own formula: it sees the new link once the tick ends.
            // It waits, up to 90s, for a tap pushed seconds ago: off the runtime's threads.
            let (host, f, v) = (self.host.clone(), formula.clone(), version.clone());
            let up = tokio::task::spawn_blocking(move || host.brew().upgrade(&f, &v));
            let up = up.await.unwrap_or_else(|e| Err(format!("brew panicked: {e}")));
            up.map_err(|e| format!("installing {formula} failed: {e}"))?;
            self.proves_formula(formula).await?;
            Ok((tagged, Some(formula.clone())))
        };
        let (tagged, installed) = match done.await {
            Ok(done) => done,
            Err(why) => {
                self.submit::<line::RecordCut>(record(&base, Some(why.clone()))).await?;
                return Ok(Ok(json!({ "cut": c.name, "version": version, "failed": why })));
            }
        };
        self.submit::<line::RecordCut>(record(&tagged, None)).await?;
        Ok(Ok(json!({ "cut": c.name, "version": version, "commit": tagged, "installed": installed })))
    }

    /// Its factorium runs in the daemon's place only once it opens this
    /// host's database (factorium#141): else the cut failed, and the old one runs on.
    async fn proves_formula(&self, formula: &str) -> Result<(), String> {
        match self.host.brew().bin(formula, "factorium") {
            Some(bin) => self.proves(&bin).await.map_err(|e| format!("installed {formula}, but {e}")),
            None => Ok(()),
        }
    }

    /// The failed cut of `version` on `c`, read back (factorium#185): its tag
    /// is on origin and, where the repo declares a formula, Homebrew has that
    /// version, whoever installed it, and it proves. Then the cut is recorded
    /// made, and answers what it did; else it stays failed, and answers
    /// nothing. The steps before the tag leave nothing to read back: the tag
    /// is pushed only once they pass.
    pub(super) async fn cut_read_back(&self, c: &Checkout, version: &str) -> Answer<Option<Value>> {
        let git = self.host.git(&c.path);
        // The checkout's tags are origin's, as fetched with its base: a cut tags in its own clone.
        git.fetch(&c.base).map_err(fail("git"))?;
        let Ok(tagged) = git.sha(&format!("refs/tags/{version}")) else { return Ok(None) };
        let Some(cut) = self.declared(&git, &tagged).ok().and_then(|d| d.cut) else { return Ok(None) };
        if let Some(formula) = &cut.formula {
            let (host, f, v) = (self.host.clone(), formula.clone(), version.to_string());
            let has = tokio::task::spawn_blocking(move || host.brew().has(&f, &v)).await.unwrap_or(false);
            if !has || self.proves_formula(formula).await.is_err() {
                return Ok(None);
            }
        }
        let record =
            line::Cut { checkout: c.name.clone(), version: version.into(), commit: tagged.clone(), failed: None };
        self.submit::<line::RecordCut>(record).await?;
        Ok(Some(
            json!({ "cut": c.name, "version": version, "commit": tagged, "installed": cut.formula, "read_back": true }),
        ))
    }
}
