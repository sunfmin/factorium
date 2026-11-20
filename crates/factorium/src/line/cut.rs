//! The cut: a version from the base tip, its steps in one heavy turn, then
//! the tag and the formula's install; recorded once, done or failed.

use factorium_domain::checkout::Checkout;
use factorium_domain::heavy::LineJob;
use factorium_domain::line::{self, next_version};
use serde_json::json;

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
            // Its factorium runs in the daemon's place only once it opens this
            // host's database (factorium#141): else the cut failed, and the old one runs on.
            if let Some(bin) = self.host.brew().bin(formula, "factorium") {
                self.proves(&bin).await.map_err(|e| format!("installed {formula}, but {e}"))?;
            }
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
}
