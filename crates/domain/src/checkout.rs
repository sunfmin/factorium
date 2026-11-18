//! A checkout: a repo on the host's list, with the branches factorium works
//! between. Every checkout has a proving branch (SPEC.md §9).

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{checkout_id, command, no};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Checkout {
    pub name: String,
    pub path: String,
    pub base: String,
    pub proving: String,
    /// On the list now: added, and not removed since.
    pub listed: bool,
}

impl Checkout {
    /// Origin's base branch as last fetched.
    pub fn fetched_base(&self) -> String {
        format!("refs/remotes/origin/{}", self.base)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum CheckoutEvent {
    CheckoutAdded { name: String, path: String, base: String, proving: String },
    CheckoutRemoved,
}

pub struct Checkouts;

impl Aggregate for Checkouts {
    const NAME: &'static str = "checkout";
    type State = Checkout;
    type Event = CheckoutEvent;

    fn evolve(state: &mut Checkout, event: CheckoutEvent) {
        match event {
            CheckoutEvent::CheckoutAdded { name, path, base, proving } => {
                *state = Checkout { name, path, base, proving, listed: true };
            }
            CheckoutEvent::CheckoutRemoved => state.listed = false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Add {
    pub name: String,
    pub path: String,
    pub base: String,
    pub proving: String,
}

command!(AddCheckout, "checkout.add", Checkouts, Add, |p| checkout_id(&p.name), |s, p, now| {
    if s.listed {
        return no("listed", format!("{} is on the list already, at {}", s.name, s.path));
    }
    if !valid_name(&p.name) {
        return no("invalid_name", "a checkout's name is letters, digits, '-', '_' and '.'");
    }
    if p.base == p.proving {
        return no("proving_is_base", "the proving branch must be another branch than the base");
    }
    let Add { name, path, base, proving } = p.clone();
    Ok(vec![CheckoutEvent::CheckoutAdded { name, path, base, proving }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Named {
    pub name: String,
}

command!(RemoveCheckout, "checkout.remove", Checkouts, Named, |p| checkout_id(&p.name), |s, p, now| {
    if !s.listed {
        return no("not_listed", format!("{} is not on the list", p.name));
    }
    Ok(vec![CheckoutEvent::CheckoutRemoved])
});

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
