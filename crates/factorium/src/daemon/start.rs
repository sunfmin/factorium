//! The tick's one start ([`App::start_one`]): the order of the ready
//! issues, the per-checkout cap, claims, blocks, accounts, roles and
//! admission, urgent or not, are all read here.

use factorium_domain::admission;
use factorium_domain::issue::urgent_first;
use factorium_domain::next;
use factorium_domain::settings::{Setting, Settings};
use factorium_domain::tracker::{self, IssueRow};
use factorium_domain::watch::{Held, NoteAdmission};
use factum::Value;
use serde_json::json;

use crate::load::Load;
use crate::{Answer, App};

impl App {
    /// One start, if admission allows: the worker it started, or why the
    /// first ready issue was held (noted for doctor); neither when none is.
    pub(crate) async fn start_one(&self, load: &Load, settings: &Settings) -> Answer<(Option<Value>, Option<String>)> {
        if let Some(IssueRow { checkout, number, urgent, .. }) = self.next_issue().await? {
            let mut started = None;
            let reading = load.admission();
            // Urgent, only the memory and disk floors hold it; what else held it, it went past.
            // A halt holds no start: a start not urgent is admission's as at any other time.
            let held = match urgent {
                true => admission::admit_urgent(&reading, settings),
                false => admission::admit(&reading, &load.queue, self.now(), settings),
            };
            // What it goes ahead of, read while it is still among the ready ones.
            let ahead_of = match (urgent, &held) {
                (true, Ok(())) => self.ahead_of(&checkout, number).await?,
                _ => Vec::new(),
            };
            let held = match held {
                Ok(()) => match self.start_worker(&checkout, number).await {
                    Ok(mut s) => {
                        if urgent {
                            let past = admission::holds(&reading, &load.queue, self.now(), settings);
                            s["urgent"] = json!({ "ahead_of": ahead_of, "past": past });
                        }
                        started = Some(s);
                        // An urgent start halts the host from now, not from the next tick.
                        self.note_halt().await?;
                        None
                    }
                    Err(r) if r.code == "no_harness" => Some(format!("kit: {}", r.message)),
                    // Claimed since it was picked: it starts on a later tick.
                    Err(r) if r.code == "claimed" => Some(format!("{checkout}#{number}: {}", r.message)),
                    Err(r) => return Err(r),
                },
                Err(why) => Some(why),
            };
            self.submit::<NoteAdmission>(Held { why: held.clone() }).await?;
            return Ok((started, held));
        }
        self.submit::<NoteAdmission>(Held { why: None }).await?;
        Ok((None, None))
    }

    /// The ready issues no worker holds, in the ordinary order they start
    /// ([`next::order`]): the checkout with the fewest live workers first.
    async fn ready(&self) -> Answer<Vec<IssueRow>> {
        let live = self.live_workers().await?;
        Ok(next::order(self.rows(tracker::untaken()).await?, live.iter().map(|w| w.checkout.as_str())))
    }

    /// The ready issues no worker holds and nothing open blocks, ahead of
    /// which the urgent `checkout`#`number` started: those not urgent before
    /// it in the ordinary order ([`App::ready`]).
    async fn ahead_of(&self, checkout: &str, number: u64) -> Answer<Vec<String>> {
        let mut ahead = Vec::new();
        for r in self.ready().await? {
            if (r.checkout.as_str(), r.number) == (checkout, number) {
                break;
            }
            let issue = self.issue(&r.checkout, r.number).await?;
            if !r.urgent && self.blockers(&issue).await?.iter().all(|w| w.met.is_some()) {
                ahead.push(format!("{}#{}", r.checkout, r.number));
            }
        }
        Ok(ahead)
    }

    /// The first ready issue no worker holds and nothing open blocks, the
    /// urgent ones first, then those that start ahead (a promotion's mutants
    /// issue), then in the ordinary order ([`App::ready`]);
    /// whose checkout is under its cap (`workers_per_checkout`), unless it is
    /// urgent or starts ahead; whose account, if it names one, can start now, and whose role
    /// has what it needs of this host (each role checked once a tick).
    async fn next_issue(&self) -> Answer<Option<IssueRow>> {
        let workers = self.live_workers().await?;
        let live = next::live(workers.iter().map(|w| w.checkout.as_str()));
        let cap = self.settings().await?.get(Setting::WorkersPerCheckout);
        let mut checked = std::collections::BTreeSet::new();
        // Roles found lacking this tick: one that passes starts its issue at once.
        let mut lacking = std::collections::BTreeSet::new();
        let mut held = None;
        let mut rows = self.ready().await?;
        // Those that start ahead (a promotion's mutants issue), then the urgent before all.
        urgent_first(&mut rows, |r| r.ahead);
        urgent_first(&mut rows, |r| r.urgent);
        for r in rows {
            // Its checkout at its cap, it waits for one of its workers to end: doctor's `capped`.
            if next::capped(&live, &r, cap) {
                continue;
            }
            let (checkout, number) = (r.checkout.clone(), r.number);
            if !checked.contains(&checkout) && self.checkout(&checkout).await.is_err() {
                continue;
            }
            checked.insert(checkout.clone());
            let issue = self.issue(&checkout, number).await?;
            // Claimed, a command acts on its worktrees: it starts once that
            // is done. A claim whose process is gone is cleared.
            self.clear_dead_claim(&issue).await?;
            if self.issue(&checkout, number).await?.claim.is_some() {
                continue;
            }
            if !self.blockers(&issue).await?.iter().all(|w| w.met.is_some()) {
                continue;
            }
            // One that cannot start now (its account used up, or what its
            // checkout is pinned to at its limit) waits behind those that
            // can; with none, its start answers why it is held.
            if self.choice(&checkout, &issue.on).await.is_err() {
                held.get_or_insert(r);
                continue;
            }
            // Its role lacking what it needs of this host, it waits; doctor
            // hands the watch what it lacks.
            let role = self.checkout(&checkout).await?.role_for(issue.on.role.as_deref()).map(|(r, _)| r.to_string());
            if let Some(role) = role
                && (lacking.contains(&role) || !self.role_ready(&role).await?)
            {
                lacking.insert(role);
                continue;
            }
            return Ok(Some(r));
        }
        Ok(held)
    }
}
