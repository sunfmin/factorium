//! The lines' fetch: once a pass, before any move, the base and proving of each checkout
//! whose line has work (a landing asked or stacked, a commit unpromoted or uncut) fetched
//! from origin in one fetch each, [`AT_ONCE`] checkouts at a time, each timed. The moves then
//! read those tips as this fetch left them ([`App::fetched`]), so a pass waits on the network
//! about as long as its slowest fetch, not as long as every fetch of every move one after
//! another; a checkout whose fetch failed refuses each move that reads a tip, as that move's own
//! fetch did.

use std::time::Instant;

use factorium_domain::checkout::Checkout;
use factorium_domain::line::Lines;
use factorium_domain::line_id;
use factorium_domain::stack::{Stacks, stack_id};
use factorium_domain::worker as w;
use factorium_host::Git;
use serde::Serialize;

use crate::{Answer, App, Refusal, fail};

/// How many checkouts' origins are fetched at once.
const AT_ONCE: usize = 8;

/// One checkout's fetch: how long it took, ms, and why it failed, if it did.
#[derive(Clone, Debug, Serialize)]
pub struct Fetched {
    pub checkout: String,
    pub ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl App {
    /// Fetches each of `checkouts` whose line has work, at once; answers each fetch.
    pub(super) async fn fetch_lines(&self, checkouts: &[Checkout]) -> Answer<Vec<Fetched>> {
        let landing: Vec<String> = (self.live_workers().await?.into_iter())
            .filter(|w| w.state.status == w::Status::Landing)
            .map(|w| w.checkout)
            .collect();
        let mut due = Vec::new();
        for c in checkouts {
            let l = self.state::<Lines>(line_id(&c.name)).await?;
            let stacked = !self.state::<Stacks>(stack_id(&c.name)).await?.layers.is_empty();
            if stacked || landing.contains(&c.name) || !l.unpromoted.is_empty() || !l.uncut.is_empty() {
                due.push(c.clone());
            }
        }
        Ok(self.fetch_now(&due))
    }

    /// Fetches the base and proving of each of `checkouts`, [`AT_ONCE`] at a time; answers
    /// each, in their order, and keeps which failed for [`App::fetched`].
    pub(super) fn fetch_now(&self, checkouts: &[Checkout]) -> Vec<Fetched> {
        let host = &*self.host;
        let fetch = move |c: &Checkout| {
            let start = Instant::now();
            let error = host.git(&c.path).fetch_all(&[&c.base, &c.proving]).err();
            Fetched { checkout: c.name.clone(), ms: start.elapsed().as_millis() as u64, error }
        };
        let fetched: Vec<Fetched> = (checkouts.chunks(AT_ONCE))
            .flat_map(|at_once| {
                std::thread::scope(|s| {
                    let each: Vec<_> = at_once.iter().map(|c| s.spawn(move || fetch(c))).collect();
                    each.into_iter().map(|f| f.join().expect("a fetch does not panic")).collect::<Vec<_>>()
                })
            })
            .collect();
        self.memory.lock().unwrap().unfetched =
            fetched.iter().filter_map(|f| Some((f.checkout.clone(), f.error.clone()?))).collect();
        fetched
    }

    /// `branch` of `c`, in its `git`, as origin had it at the lines' fetch; refused as that
    /// fetch was, if it failed.
    pub(crate) fn fetched(&self, c: &Checkout, git: &Git, branch: &str) -> Answer<String> {
        if let Some(e) = self.memory.lock().unwrap().unfetched.get(&c.name) {
            return Err(Refusal::new("git", e.clone()));
        }
        git.sha(&format!("refs/remotes/origin/{branch}")).map_err(fail("git"))
    }
}
