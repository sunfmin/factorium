//! The kit: the harnesses this host starts, in priority order, each with its
//! launch command and model; a checkout may pin one. And the accounts: each
//! harness's subscriptions, with the limits they met. A worker starts on the first eligible harness, on its account with
//! the most left.

use std::collections::BTreeMap;

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{command, id, no};

pub fn kit_id() -> factum_core::Uuid {
    id("kit")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub harness: String,
    /// The line that launches it, e.g. `claude --dangerously-skip-permissions`.
    pub command: String,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Limited until then, ms: a quota screen, or a limit event.
    pub limited_until: Option<i64>,
}

impl Account {
    /// What is left, in percent: nothing while limited, else all of it.
    pub fn left(&self, now: i64) -> f64 {
        if self.limited_until.is_some_and(|t| t > now) { 0.0 } else { 100.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Kit {
    pub entries: Vec<Entry>,
    /// Checkout name → the harness it is pinned to.
    pub pins: BTreeMap<String, String>,
    pub accounts: BTreeMap<String, Account>,
}

/// What a worker starts on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub entry: Entry,
    pub account: Option<String>,
}

impl Kit {
    /// The harness and account a worker of `checkout` starts on now: the
    /// pinned harness, or the first in order with an account left (a harness
    /// with no account on the list runs on the host user's own login).
    pub fn choose(&self, checkout: &str, now: i64, here: &[(String, String)]) -> Result<Choice, String> {
        let pinned = self.pins.get(checkout);
        let entries = self.entries.iter().filter(|e| pinned.is_none_or(|p| *p == e.harness));
        let mut why = Vec::new();
        for entry in entries {
            let none = Account::default();
            let mut accounts: Vec<(&String, &Account)> = here
                .iter()
                .filter(|(_, h)| *h == entry.harness)
                .map(|(n, _)| (n, self.accounts.get(n).unwrap_or(&none)))
                .collect();
            if accounts.is_empty() {
                return Ok(Choice { entry: entry.clone(), account: None });
            }
            accounts.sort_by(|a, b| b.1.left(now).total_cmp(&a.1.left(now)).then(a.0.cmp(b.0)));
            let (name, best) = accounts[0];
            if best.left(now) > 0.0 {
                return Ok(Choice { entry: entry.clone(), account: Some(name.clone()) });
            }
            why.push(format!("{}: every account is used up", entry.harness));
        }
        if why.is_empty() {
            why.push(match pinned {
                Some(p) => format!("{checkout} is pinned to {p}, which is not in the kit"),
                None => "the kit is empty".into(),
            });
        }
        Err(why.join("; "))
    }
}

impl Kit {
    /// The harness and account a worker on `harness` has its mutants picked
    /// on: the first other harness of the kit `asks` says can be asked, else
    /// `harness` again (a new session), whatever a pin says, on its account
    /// with the most left.
    pub fn choose_picker(
        &self,
        harness: &str,
        asks: impl Fn(&str) -> bool,
        now: i64,
        here: &[(String, String)],
    ) -> Result<Choice, String> {
        let (others, same): (Vec<Entry>, Vec<Entry>) =
            self.entries.iter().filter(|e| asks(&e.harness)).cloned().partition(|e| e.harness != harness);
        let kit = Kit { entries: [others, same].concat(), pins: BTreeMap::new(), accounts: self.accounts.clone() };
        kit.choose("", now, here).map_err(|why| format!("no harness of the kit that can pick mutants: {why}"))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KitEvent {
    KitSet { entries: Vec<Entry> },
    CheckoutPinned { checkout: String, harness: Option<String> },
    AccountLimited { name: String, until: i64 },
}

pub struct Kits;

impl Aggregate for Kits {
    const NAME: &'static str = "kit";
    type State = Kit;
    type Event = KitEvent;

    fn evolve(s: &mut Kit, event: KitEvent) {
        match event {
            KitEvent::KitSet { entries } => s.entries = entries,
            KitEvent::CheckoutPinned { checkout, harness: Some(h) } => _ = s.pins.insert(checkout, h),
            KitEvent::CheckoutPinned { checkout, harness: None } => _ = s.pins.remove(&checkout),
            KitEvent::AccountLimited { name, until } => s.accounts.entry(name).or_default().limited_until = Some(until),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetKit {
    pub entries: Vec<Entry>,
}

command!(SetTheKit, "kit.set", Kits, SetKit, |_p| kit_id(), |s, p, now| {
    for e in &p.entries {
        if e.command.trim().is_empty() {
            return no("no_command", format!("{} needs a launch command", e.harness));
        }
    }
    Ok(vec![KitEvent::KitSet { entries: p.entries.clone() }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pin {
    pub checkout: String,
    pub harness: Option<String>,
}

command!(PinCheckout, "kit.pin", Kits, Pin, |_p| kit_id(), |s, p, now| {
    Ok(vec![KitEvent::CheckoutPinned { checkout: p.checkout.clone(), harness: p.harness.clone() }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountNote {
    pub name: String,
    #[serde(default)]
    pub until: i64,
}

command!(LimitAccount, "account.limited", Kits, AccountNote, |_p| kit_id(), |s, p, now| {
    Ok::<_, factum_core::Rejection>(match s.accounts.get(&p.name) {
        a if a.and_then(|a| a.limited_until).is_none_or(|u| u < p.until) => {
            vec![KitEvent::AccountLimited { name: p.name.clone(), until: p.until }]
        }
        _ => Vec::new(),
    })
});

#[cfg(test)]
mod tests {
    use super::*;

    /// Which other harness is first, and that the pin does not choose it.
    #[test]
    fn a_picker_runs_on_the_kit_s_other_harness_that_asks_else_on_the_worker_s() {
        let entry = |h: &str| Entry { harness: h.into(), command: h.into(), model: None };
        let pins = BTreeMap::from([("web".to_string(), "a".to_string())]);
        let kit = Kit { entries: vec![entry("a"), entry("b"), entry("c")], pins, accounts: BTreeMap::new() };
        let picked = |worker: &str, asks: &[&str]| {
            let asks = |h: &str| asks.contains(&h);
            kit.choose_picker(worker, asks, 0, &[]).map(|c| c.entry.harness)
        };
        assert_eq!(picked("a", &["a", "b"]), Ok("b".into()));
        assert_eq!(picked("b", &["a", "b"]), Ok("a".into()));
        assert_eq!(picked("a", &["a", "c"]), Ok("c".into()), "the pin is the worker's, not the picker's");
        assert_eq!(picked("a", &["a"]), Ok("a".into()));
        assert!(picked("a", &[]).unwrap_err().starts_with("no harness of the kit that can pick mutants"));
    }
}
