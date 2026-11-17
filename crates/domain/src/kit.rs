//! The kit: the harnesses this host starts, in priority order, each with its
//! launch command and model; a checkout may pin one. And the accounts: each
//! harness's subscriptions, with their usage windows and the limits they
//! met. A worker starts on the first eligible harness, on its account with
//! the most left.

use std::collections::BTreeMap;

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{command, id, no};

pub fn kit_id() -> factum_core::Uuid {
    id("kit")
}

/// The harness keys factorium starts.
pub const HARNESSES: &[&str] = &["claude-code", "codex", "pi", "dsh", "kimi", "grok"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub harness: String,
    /// The line that launches it, e.g. `claude --dangerously-skip-permissions`.
    pub command: String,
    pub model: Option<String>,
}

/// One usage window an account's harness reported: how much is used, and
/// when the window resets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub used_percent: f64,
    /// When it resets, ms.
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub windows: Vec<Window>,
    /// Limited until then, ms: a quota screen, or a limit event.
    pub limited_until: Option<i64>,
}

impl Account {
    /// What is left, in percent: of the fullest window, nothing while limited.
    pub fn left(&self, now: i64) -> f64 {
        if self.limited_until.is_some_and(|t| t > now) {
            return 0.0;
        }
        let live = self.windows.iter().filter(|w| w.resets_at.is_none_or(|r| r > now));
        100.0 - live.map(|w| w.used_percent).fold(0.0, f64::max)
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

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KitEvent {
    KitSet { entries: Vec<Entry> },
    CheckoutPinned { checkout: String, harness: Option<String> },
    WindowsRead { name: String, windows: Vec<Window> },
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
            KitEvent::WindowsRead { name, windows } => s.accounts.entry(name).or_default().windows = windows,
            KitEvent::AccountLimited { name, until } => s.accounts.entry(name).or_default().limited_until = Some(until),
        }
    }
}

fn known(h: &str) -> Result<(), factum_core::Rejection> {
    if HARNESSES.contains(&h) { Ok(()) } else { no("no_harness", format!("{h} is none of {}", HARNESSES.join(", "))) }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetKit {
    pub entries: Vec<Entry>,
}

command!(SetTheKit, "kit.set", Kits, SetKit, |_p| kit_id(), |s, p, now| {
    for e in &p.entries {
        known(&e.harness)?;
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
    if let Some(h) = &p.harness {
        known(h)?;
    }
    Ok(vec![KitEvent::CheckoutPinned { checkout: p.checkout.clone(), harness: p.harness.clone() }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountNote {
    pub name: String,
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub until: i64,
}

command!(ReadWindows, "account.windows", Kits, AccountNote, |_p| kit_id(), |s, p, now| {
    Ok::<_, factum_core::Rejection>(match s.accounts.get(&p.name) {
        a if a.is_none_or(|a| a.windows != p.windows) => {
            vec![KitEvent::WindowsRead { name: p.name.clone(), windows: p.windows.clone() }]
        }
        _ => Vec::new(),
    })
});

command!(LimitAccount, "account.limited", Kits, AccountNote, |_p| kit_id(), |s, p, now| {
    Ok::<_, factum_core::Rejection>(match s.accounts.get(&p.name) {
        a if a.and_then(|a| a.limited_until).is_none_or(|u| u < p.until) => {
            vec![KitEvent::AccountLimited { name: p.name.clone(), until: p.until }]
        }
        _ => Vec::new(),
    })
});
