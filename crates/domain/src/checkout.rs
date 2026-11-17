//! A checkout: a repo on the host's list, with the branches factorium works
//! between. Every checkout has a proving branch (SPEC.md §9).

use factum_core::{Aggregate, By, Command, Decision, Instant, Rejection, Uuid};
use serde::{Deserialize, Serialize};

use crate::checkout_id;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Checkout {
    pub name: String,
    pub path: String,
    pub base: String,
    pub proving: String,
    /// On the list now: added, and not removed since.
    pub listed: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum CheckoutEvent {
    CheckoutAdded {
        name: String,
        path: String,
        base: String,
        proving: String,
    },
    CheckoutRemoved,
}

pub struct Checkouts;

impl Aggregate for Checkouts {
    const NAME: &'static str = "checkout";
    type State = Checkout;
    type Event = CheckoutEvent;

    fn evolve(state: &mut Checkout, event: CheckoutEvent) {
        match event {
            CheckoutEvent::CheckoutAdded {
                name,
                path,
                base,
                proving,
            } => {
                *state = Checkout {
                    name,
                    path,
                    base,
                    proving,
                    listed: true,
                };
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

pub struct AddCheckout;

impl Command for AddCheckout {
    const NAME: &'static str = "checkout.add";
    type Aggregate = Checkouts;
    type Params = Add;

    fn stream(p: &Add) -> Uuid {
        checkout_id(&p.name)
    }

    fn decide(
        s: &Checkout,
        p: &Add,
        _: &By,
        _: Instant,
    ) -> Result<Decision<CheckoutEvent>, Rejection> {
        if s.listed {
            return Err(Rejection::new(
                "listed",
                format!("{} is on the list already, at {}", s.name, s.path),
            ));
        }
        if !valid_name(&p.name) {
            return Err(Rejection::new(
                "invalid_name",
                "a checkout's name is letters, digits, '-', '_' and '.'",
            ));
        }
        if p.base == p.proving {
            return Err(Rejection::new(
                "proving_is_base",
                "the proving branch must be another branch than the base",
            ));
        }
        Ok(vec![CheckoutEvent::CheckoutAdded {
            name: p.name.clone(),
            path: p.path.clone(),
            base: p.base.clone(),
            proving: p.proving.clone(),
        }]
        .into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Named {
    pub name: String,
}

pub struct RemoveCheckout;

impl Command for RemoveCheckout {
    const NAME: &'static str = "checkout.remove";
    type Aggregate = Checkouts;
    type Params = Named;

    fn stream(p: &Named) -> Uuid {
        checkout_id(&p.name)
    }

    fn decide(
        s: &Checkout,
        p: &Named,
        _: &By,
        _: Instant,
    ) -> Result<Decision<CheckoutEvent>, Rejection> {
        if !s.listed {
            return Err(Rejection::new(
                "not_listed",
                format!("{} is not on the list", p.name),
            ));
        }
        Ok(vec![CheckoutEvent::CheckoutRemoved].into())
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
