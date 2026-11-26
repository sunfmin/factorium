//! A checkout: a repo on the host's list, with the branches factorium works
//! between. Every checkout has a proving branch (SPEC.md §9).

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{checkout_id, command, no, only};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Checkout {
    pub name: String,
    pub path: String,
    pub base: String,
    pub proving: String,
    /// On the list now: added, and not removed since.
    pub listed: bool,
    /// The role directory a worker of an issue that names none starts with
    /// (docs/adr/0007-roles-and-waiting-issues.md); an issue's own role wins.
    pub role: Option<String>,
    /// Its idle work is off on this host (docs/adr/0013-idle-work.md): no idle run
    /// starts for it here and none files its tasks; a run by hand still does.
    #[serde(default)]
    pub idle_off: bool,
}

impl Checkout {
    /// The role a worker of an issue whose own role is `own` starts with,
    /// and whose it is, `"issue"` or `"checkout"`: the issue's wins.
    pub fn role_for<'a>(&'a self, own: Option<&'a str>) -> Option<(&'a str, &'static str)> {
        own.map(|r| (r, "issue")).or_else(|| self.role.as_deref().map(|r| (r, "checkout")))
    }

    /// Origin's base branch as last fetched.
    pub fn fetched_base(&self) -> String {
        format!("refs/remotes/origin/{}", self.base)
    }

    /// Origin's proving branch as last fetched: what a worker's change lands on.
    pub fn fetched_proving(&self) -> String {
        format!("refs/remotes/origin/{}", self.proving)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum CheckoutEvent {
    CheckoutAdded(Add),
    CheckoutRemoved,
    /// Its default role from now: a directory, or, `None`, none.
    RoleNamed {
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Its idle work on this host is off from now, or, `false`, on.
    IdleTurned {
        off: bool,
    },
}

pub struct Checkouts;

impl Aggregate for Checkouts {
    const NAME: &'static str = "checkout";
    type State = Checkout;
    type Event = CheckoutEvent;

    fn evolve(state: &mut Checkout, event: CheckoutEvent) {
        match event {
            CheckoutEvent::CheckoutAdded(Add { name, path, base, proving, role }) => {
                *state = Checkout { name, path, base, proving, listed: true, role, idle_off: false };
            }
            CheckoutEvent::CheckoutRemoved => state.listed = false,
            CheckoutEvent::RoleNamed { role } => state.role = role,
            CheckoutEvent::IdleTurned { off } => state.idle_off = off,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Add {
    pub name: String,
    pub path: String,
    pub base: String,
    pub proving: String,
    /// Its default role, a directory by its absolute path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
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
    Ok(vec![CheckoutEvent::CheckoutAdded(p.clone())])
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

/// Checkout `name` names `role` as its default role from now, or, `None`, none.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NameRole {
    pub name: String,
    pub role: Option<String>,
}

command!(NameCheckoutRole, "checkout.name_role", Checkouts, NameRole, |p| checkout_id(&p.name), |s, p, now| {
    if !s.listed {
        return no("not_listed", format!("{} is not on the list", p.name));
    }
    only(s.role != p.role, CheckoutEvent::RoleNamed { role: p.role.clone() })
});

/// Checkout `name`'s idle work on this host is off from now, or, `false`, on.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnIdle {
    pub name: String,
    pub off: bool,
}

command!(TurnCheckoutIdle, "checkout.turn_idle", Checkouts, TurnIdle, |p| checkout_id(&p.name), |s, p, now| {
    if !s.listed {
        return no("not_listed", format!("{} is not on the list", p.name));
    }
    only(s.idle_off != p.off, CheckoutEvent::IdleTurned { off: p.off })
});

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
