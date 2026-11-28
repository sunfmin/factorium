//! The lines' fetch: once a pass, before any move, the base and proving of each checkout
//! whose line has work (a landing asked or stacked, a commit unpromoted or uncut) fetched
//! from origin in one fetch each, [`AT_ONCE`] checkouts at a time, each timed. The moves then
//! read those tips as this fetch left them ([`App::fetched`]), so a pass waits on the network
//! about as long as its slowest fetch, not as long as every fetch of every move one after
//! another; a checkout whose fetch failed refuses each move that reads a tip, as that move's own
//! fetch did.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
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
        let (host, next) = (&*self.host, AtomicUsize::new(0));
        let each: Mutex<Vec<(usize, Fetched)>> = Mutex::default();
        std::thread::scope(|s| {
            for _ in 0..AT_ONCE.min(checkouts.len()) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(c) = checkouts.get(i) else { break };
                        let start = Instant::now();
                        let error = host.git(&c.path).fetch_all(&[&c.base, &c.proving]).err();
                        let ms = start.elapsed().as_millis() as u64;
                        each.lock().unwrap().push((i, Fetched { checkout: c.name.clone(), ms, error }));
                    }
                });
            }
        });
        let mut each = each.into_inner().unwrap();
        each.sort_by_key(|(i, _)| *i);
        let fetched: Vec<Fetched> = each.into_iter().map(|(_, f)| f).collect();
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
