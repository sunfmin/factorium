//! The cut: a version from the base tip, its steps in one heavy turn (the
//! daemon's beside its ticks), then the tag and the formula's install;
//! recorded once, done or failed: the daemon's once its binary's proof,
//! beside the ticks too, ended.

use factorium_domain::checkout::Checkout;
use factorium_domain::heavy::LineJob;
use factorium_domain::line::{self, next_version};
use serde_json::{Value, json};

use factorium_host::{Git, Ran};

use super::{Did, Moving, QUEUED, Then};
use crate::heavy::{For, Job, Turn};
use crate::{Answer, App, LineClone, fail};

/// Why a cut the daemon finished is not recorded yet.
const PROVING: &str = "its installed binary is being proven beside the ticks";

/// A cut of `version` of `checkout` from `base`.
#[derive(Clone)]
struct Cut {
    checkout: String,
    version: String,
    base: String,
}

/// A cut tagged at `tagged` and its `formula` installed, whose binary's
/// proof (factorium#141) runs beside the daemon's ticks (factorium#242): a
/// `doctor` of a slow binary has up to 120 s, past half the hang bound.
pub(crate) struct Proving {
    cut: Cut,
    tagged: String,
    formula: String,
    proven: tokio::sync::watch::Receiver<Option<Result<(), String>>>,
}

impl Proving {
    /// How its proof ended, once it did.
    fn ended(&self) -> Option<Result<(), String>> {
        match &*self.proven.borrow() {
            Some(proven) => Some(proven.clone()),
            None if self.proven.has_changed().is_err() => Some(Err("its proof ended without an answer".into())),
            None => None,
        }
    }
}

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
        // The steps share the clone, so they take one turn of the queue.
        let job = Job::cut(LineJob::Cut.of(&c.name), &here.dir, &cut.steps, &version, cut.limit).of(None, bound_at);
        let turn = if by_hand {
            Turn::Wait
        } else {
            let key = Some(format!("cut:{}:{version}:{base}", c.name));
            let (version, base, dir, steps) = (version.clone(), base.clone(), here.dir.clone(), cut.steps.clone());
            let then = Then::Cut { version, base, dir, steps };
            Turn::Beside { key, yields: false, of: For::Line(Box::new(Moving { checkout: c.name.clone(), then })) }
        };
        let Some(rans) = self.heavy(job, turn).await?.filter(|_| by_hand) else { return Ok(Err(QUEUED.into())) };
        self.cut_ran(c, &here, &version, &base, &cut.steps, rans, false).await
    }

    /// Finishes the cut of `version` of `c` from `base`, whose `steps` ran in the clone `here`
    /// so: tagged and installed once they all passed, recorded made or failed. `beside`: the
    /// daemon's, whose installed binary's proof runs beside the ticks ([`Proving`]), and a later
    /// tick records the cut once it ends ([`App::cuts_proven`]); else it is waited for.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn cut_ran(
        &self,
        c: &Checkout,
        here: &Git,
        version: &str,
        base: &str,
        steps: &[String],
        rans: Vec<Ran>,
        beside: bool,
    ) -> Answer<Did> {
        let declared = self.declared(here, base)?;
        let formula = declared.cut.and_then(|cut| cut.formula);
        // Each step after the queue, in turn; the first that fails is why the
        // cut failed, and every way it fails is recorded the one way.
        let done = async {
            if let Some(ran) = rans.last().filter(|r| !r.ok) {
                return Err(format!("step `{}` failed:\n{}", steps[rans.len() - 1], ran.tail));
            }
            let tagged = here.sha("HEAD").and_then(|t| here.tag_pushed(version, &t).map(|_| t));
            let tagged = tagged.map_err(|e| format!("tagging failed: {e}"))?;
            let Some(formula) = &formula else { return Ok((tagged, None)) };
            // The daemon's own formula: it sees the new link once the tick ends.
            // It waits, up to 90s, for a tap pushed seconds ago: off the runtime's threads.
            let (host, f, v) = (self.host.clone(), formula.clone(), version.to_string());
            let up = tokio::task::spawn_blocking(move || host.brew().upgrade(&f, &v));
            let up = up.await.unwrap_or_else(|e| Err(format!("brew panicked: {e}")));
            up.map_err(|e| format!("installing {formula} failed: {e}"))?;
            Ok((tagged, Some((formula.clone(), self.proves_formula(formula)))))
        };
        let cut = Cut { checkout: c.name.clone(), version: version.into(), base: base.into() };
        let (tagged, installed) = match done.await {
            Ok((tagged, Some((formula, Some(proof))))) if beside => {
                let (tx, proven) = tokio::sync::watch::channel(None);
                tokio::spawn(async move { tx.send_replace(Some(proof.await)) });
                self.memory.lock().unwrap().proving.push(Proving { cut, tagged, formula, proven });
                return Ok(Err(PROVING.into()));
            }
            Ok((tagged, Some((formula, Some(proof))))) => (tagged, proof.await.map(|()| Some(formula))),
            Ok((tagged, installed)) => (tagged, Ok(installed.map(|(f, _)| f))),
            Err(why) => (String::new(), Err(why)),
        };
        self.cut_recorded(cut, &tagged, installed).await
    }

    /// Records `cut` made at `tagged`, its formula `installed` (if it declares one), or failed why.
    async fn cut_recorded(&self, cut: Cut, tagged: &str, installed: Result<Option<String>, String>) -> Answer<Did> {
        let Cut { checkout, version, base } = cut;
        let (commit, failed) = match &installed {
            Ok(_) => (tagged.to_string(), None),
            Err(why) => (base, Some(why.clone())),
        };
        let record = line::Cut { checkout: checkout.clone(), version: version.clone(), commit: commit.clone(), failed };
        self.submit::<line::RecordCut>(record).await?;
        Ok(Ok(match installed {
            Ok(installed) => json!({ "cut": checkout, "version": version, "commit": commit, "installed": installed }),
            Err(why) => json!({ "cut": checkout, "version": version, "failed": why }),
        }))
    }

    /// Each cut whose proof ran beside the ticks and ended, recorded as it
    /// ended: what each did. One whose record fails is kept for the next tick.
    pub(crate) async fn cuts_proven(&self) -> Answer<Vec<Value>> {
        let ended: Vec<Proving> = {
            let mut m = self.memory.lock().unwrap();
            let (ended, on) = std::mem::take(&mut m.proving).into_iter().partition(|p| p.ended().is_some());
            m.proving = on;
            ended
        };
        let (mut did, mut ended) = (Vec::new(), ended.into_iter());
        while let Some(p) = ended.next() {
            let proven = p.ended().expect("ended").map(|()| Some(p.formula.clone()));
            match self.cut_recorded(p.cut.clone(), &p.tagged, proven).await {
                Ok(d) => did.extend(d.ok()),
                Err(e) => {
                    let mut m = self.memory.lock().unwrap();
                    m.proving.push(p);
                    m.proving.extend(ended);
                    return Err(e);
                }
            }
        }
        Ok(did)
    }

    /// Whether a cut of `checkout` is being proven beside the ticks.
    pub(crate) fn cut_proving(&self, checkout: &str) -> bool {
        self.memory.lock().unwrap().proving.iter().any(|p| p.cut.checkout == checkout)
    }

    /// Until each cut being proven beside the ticks now has its proof ended.
    pub(crate) async fn cuts_proofs_ended(&self) {
        let proving: Vec<_> = self.memory.lock().unwrap().proving.iter().map(|p| p.proven.clone()).collect();
        for mut p in proving {
            // Its sender gone unanswered, it ended too ([`Proving::ended`]).
            let _ = p.wait_for(Option::is_some).await;
        }
    }

    /// Its factorium runs in the daemon's place only once it opens this
    /// host's database (factorium#141): else the cut failed, and the old one
    /// runs on. The proof of its binary, if it has one.
    fn proves_formula(&self, formula: &str) -> Option<impl Future<Output = Result<(), String>> + Send + 'static> {
        let bin = self.host.brew().bin(formula, "factorium")?;
        let (proof, formula) = (self.proves(&bin), formula.to_string());
        Some(async move { proof.await.map_err(|e| format!("installed {formula}, but {e}")) })
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
            let proven = match self.proves_formula(formula) {
                Some(proof) => proof.await.is_ok(),
                None => true,
            };
            if !has || !proven {
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
