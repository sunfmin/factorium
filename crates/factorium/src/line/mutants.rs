//! A promotion's mutants, where the checkout's `slimfit.toml` holds their
//! record at the base (`[check] mutants_target`): before its slow gate, the
//! promotion asks slimfit's listing of the range whether it owes one; if it
//! does, it files one issue for it, which starts ahead of the ordinary ones,
//! and waits for it within `mutants_bound` of its filing. The listing takes
//! most of a minute: it runs beside the ticks, in the promotion's work dir,
//! and a later tick reads what it said ([`Listing`]). The rules are
//! [`factorium_domain::mutants`]'s.

use std::path::PathBuf;

use factorium_domain::checkout::Checkout;
use factorium_domain::issue::{Open, Status};
use factorium_domain::line::{self, Lines};
use factorium_domain::line_id;
use factorium_domain::mutants;
use factorium_domain::settings::Time;
use factorium_domain::when::utc;
use factorium_host::{Git, Said};
use serde_json::{Value, json};

use super::Did;
use crate::{Answer, App, LineDir};

/// Why a promotion does not go on while its mutants are listed.
const LISTS: &str = "lists its mutants beside the ticks";

/// slimfit's listing of a promotion's mutants, running beside the ticks: of which proving tip,
/// in which work dir, and what it says once it ends.
pub(crate) struct Listing {
    tip: String,
    pub(crate) dir: PathBuf,
    said: tokio::task::JoinHandle<Said>,
}

impl Listing {
    /// Whether it runs on.
    pub(crate) fn runs(&self) -> bool {
        !self.said.is_finished()
    }
}

impl App {
    /// What the promotion of `c`'s proving `tip`, in the checkout `git`, does about its mutants
    /// before its slow gate: `None`, it goes on to it; else what it did instead: filed the issue
    /// that owes them, waits for it, or lists them beside the ticks. A listing slimfit refuses
    /// holds nothing back: the slow gate runs slimfit too.
    pub(super) async fn mutants_first(&self, c: &Checkout, git: &Git, tip: &str) -> Answer<Option<Did>> {
        let Some(target) = git.shown(tip, "slimfit.toml").and_then(|t| mutants::held_at(&t)) else {
            return Ok(None);
        };
        // One mutants issue open per checkout: while it is, the promotion waits for it or goes on.
        if let Some(filed) = self.state::<Lines>(line_id(&c.name)).await?.awaited {
            let i = self.issue(&c.name, filed.number).await?;
            if i.status != Status::Closed {
                let startable = i.worker.is_some() || self.choice(&c.name, &i.on).await.is_ok();
                let bound = self.settings().await?.ms(Time::MutantsBound);
                return Ok(mutants::waits(filed, startable, self.now(), bound).map(|until| {
                    Err(format!(
                        "waits for its mutants issue {}#{}, at most until {}",
                        c.name,
                        filed.number,
                        utc(until)
                    ))
                }));
            }
        }
        if self.memory.lock().unwrap().owes_none.get(&c.name).is_some_and(|t| t == tip) {
            return Ok(None);
        }
        let Some((said, dir)) = self.mutants_listed(c, tip, &target).await? else { return Ok(Some(Err(LISTS.into()))) };
        let Some(listing) = serde_json::from_str::<Value>(&said.out).ok().filter(|_| said.ok) else {
            return Ok(None);
        };
        if !mutants::owes(&listing) {
            self.memory.lock().unwrap().owes_none.insert(c.name.clone(), tip.to_string());
            return Ok(None);
        }
        let _ = self.work_dir_done(LineDir::Promotion, &c.name, &dir);
        let from = listing["landing"]["from"].as_str().unwrap_or(&target).to_string();
        let (title, body, task) = mutants::issue(&c.proving, &target, &from, tip);
        let open = Open { checkout: c.name.clone(), title, body, task: Some(task), ..Open::default() };
        let number = self.open_issue(open).await?;
        self.submit::<line::AwaitMutants>(line::Await { checkout: c.name.clone(), number }).await?;
        let filed = format!("{}#{number}", c.name);
        Ok(Some(Ok(json!({ "promotion": c.name, "mutants": filed, "range": format!("{from}..{tip}") }))))
    }

    /// The checkouts whose mutants' listing had ended as the pass over the lines began.
    pub(super) fn mutants_ended(&self) -> Vec<String> {
        let m = self.memory.lock().unwrap();
        m.listings.iter().filter(|(_, l)| !l.runs()).map(|(c, _)| c.clone()).collect()
    }

    /// Lets go each listing of `ended` that the pass did not read: its promotion is no longer
    /// due. The sweep removes its work dir.
    pub(super) fn mutants_unread(&self, ended: &[String]) {
        self.memory.lock().unwrap().listings.retain(|c, _| !ended.contains(c));
    }

    /// What slimfit's listing of `c`'s mutants at `tip` against `target` said once it ended, and
    /// the work dir it ran in, which the promotion's gate takes (it has its name); `None` while it
    /// runs beside the ticks, started now if none was. One of another tip is let go once it ends.
    async fn mutants_listed(&self, c: &Checkout, tip: &str, target: &str) -> Answer<Option<(Said, PathBuf)>> {
        let ended = {
            let mut m = self.memory.lock().unwrap();
            match m.listings.get(&c.name) {
                Some(l) if l.runs() => return Ok(None),
                _ => m.listings.remove(&c.name),
            }
        };
        if let Some(l) = ended {
            if l.tip == tip {
                let said = l.said.await.unwrap_or_else(|e| Said { ok: false, out: String::new(), err: e.to_string() });
                return Ok(Some((said, l.dir)));
            }
            let _ = self.work_dir_done(LineDir::Promotion, &c.name, &l.dir);
        }
        let here = self.promotion_dir(c, tip)?;
        let (host, dir, target) = (self.host.clone(), here.dir.to_string_lossy().into_owned(), target.to_string());
        let said = tokio::task::spawn_blocking(move || {
            host.programs().run("slimfit", &["check", "--repo", &dir, "--mutants", "--target", &target])
        });
        self.memory.lock().unwrap().listings.insert(c.name.clone(), Listing { tip: tip.into(), dir: here.dir, said });
        Ok(None)
    }
}
