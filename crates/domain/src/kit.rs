//! The kit: the harnesses this host starts, in priority order, each with its
//! launch command and model; a checkout may pin one. A worker starts on the
//! first eligible harness, on the account stablehand picks of its engine
//! (stablehand, the accounts' one home, keeps their limits and windows);
//! a harness of which this host has no account runs on the host user's own
//! login, which rests on its own limits here.

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
pub struct Kit {
    pub entries: Vec<Entry>,
    /// Checkout name → the harness it is pinned to.
    pub pins: BTreeMap<String, String>,
    /// Harness → until when (ms) the host's own login of it is at its
    /// engine's limit: a harness no account here holds.
    #[serde(default)]
    pub limits: BTreeMap<String, i64>,
}

/// The harness a worker starts on when stablehand cannot be asked: on the
/// host user's own login, and no other, since without stablehand no other
/// engine's accounts or windows are known.
pub const FALLBACK: &str = "claude-code";

/// What a worker starts on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub entry: Entry,
    pub account: Option<String>,
}

/// What stablehand's pick answered of one engine (`stablehand pick`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picked {
    /// The account one more session starts on.
    On(String),
    /// None now: each account of the engine that cannot take one, and why.
    None(Vec<Unpicked>),
}

/// An account stablehand's pick passed over, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unpicked {
    pub account: String,
    /// Its `reason`: `off`, `elsewhere`, `not_logged_in`, `limit`, …
    pub reason: String,
    pub why: String,
}

impl Picked {
    /// Whether this host holds no account of the engine: none, or each
    /// one's login bound to another machine. Its sessions run on the host
    /// user's own login.
    fn none_here(&self) -> bool {
        matches!(self, Picked::None(why) if why.iter().all(|u| u.reason == "elsewhere"))
    }

    /// Why `account` can take no session now, as the pick said; `None`: it
    /// can.
    fn passed_over(&self, account: &str) -> Option<&Unpicked> {
        match self {
            Picked::On(_) => None,
            Picked::None(why) => why.iter().find(|u| u.account == account),
        }
    }
}

impl Kit {
    /// The harness and account a worker of `checkout` starts on now: the
    /// pinned harness, or the first in order with an account `pick` (of its
    /// harness, by key) picks; a harness this host holds no account of
    /// runs on the host user's own login unless that is at its limit.
    /// A pick that fails (stablehand not found, or a fault) leaves the
    /// [`FALLBACK`] harness alone, on the host user's own login: without
    /// stablehand no other engine's accounts or windows are known. `Err`:
    /// why none can.
    pub fn choose(
        &self,
        checkout: &str,
        now: i64,
        mut pick: impl FnMut(&str) -> Result<Picked, String>,
    ) -> Result<Choice, String> {
        let pinned = self.pins.get(checkout);
        let entries = self.entries.iter().filter(|e| pinned.is_none_or(|p| *p == e.harness));
        let limited = |h: &str| self.limits.get(h).copied().filter(|t| *t > now);
        let mut why = Vec::new();
        for entry in entries.clone() {
            let picked = match pick(&entry.harness) {
                Ok(picked) => picked,
                Err(fault) => return self.fallback(entries, fault, limited),
            };
            if picked.none_here() {
                match limited(&entry.harness) {
                    None => return Ok(Choice { entry: entry.clone(), account: None }),
                    Some(t) => {
                        why.push(format!("{}: at its limit until it resets at {t} (ms)", entry.harness));
                        continue;
                    }
                }
            }
            match picked {
                Picked::On(name) => return Ok(Choice { entry: entry.clone(), account: Some(name) }),
                Picked::None(passed) => {
                    let each: Vec<String> = passed.iter().map(|u| format!("{}: {}", u.account, u.why)).collect();
                    why.push(format!("{}: stablehand picks no account ({})", entry.harness, each.join("; ")));
                }
            }
        }
        if why.is_empty() {
            why.push(match pinned {
                Some(p) => format!("{checkout} is pinned to {p}, which is not in the kit"),
                None => "the kit is empty".into(),
            });
        }
        Err(why.join("; "))
    }

    /// What a worker starts on when stablehand could not be asked
    /// (`fault`): the [`FALLBACK`] entry among `entries`, on the host
    /// user's own login, unless `limited` says that login is at its limit.
    fn fallback<'a>(
        &self,
        mut entries: impl Iterator<Item = &'a Entry>,
        fault: String,
        limited: impl Fn(&str) -> Option<i64>,
    ) -> Result<Choice, String> {
        let Some(entry) = entries.find(|e| e.harness == FALLBACK) else {
            return Err(format!("{fault}; without stablehand only {FALLBACK} starts, and it is not to be had here"));
        };
        match limited(FALLBACK) {
            None => Ok(Choice { entry: entry.clone(), account: None }),
            Some(t) => Err(format!("{fault}; {FALLBACK} on the host's own login is at its limit until {t} (ms)")),
        }
    }

    /// What a worker of `checkout` whose issue names `account`, of the
    /// harness `harness`, starts on now: that account, on the first entry
    /// of its harness, whatever the order of the kit. Refused when its
    /// harness is not in the kit or not the one `checkout` is pinned to,
    /// or `picked` (stablehand's pick of that harness) passed it over.
    pub fn choose_on(&self, checkout: &str, account: &str, harness: &str, picked: &Picked) -> Result<Choice, String> {
        if let Some(p) = self.pins.get(checkout).filter(|p| *p != harness) {
            return Err(format!("account {account} is of {harness}, and {checkout} is pinned to {p}"));
        }
        let Some(entry) = self.entries.iter().find(|e| e.harness == *harness) else {
            return Err(format!("account {account} is of {harness}, which is not in the kit"));
        };
        if let Some(u) = picked.passed_over(account) {
            return Err(format!("account {account} takes no session now: {}", u.why));
        }
        Ok(Choice { entry: entry.clone(), account: Some(account.into()) })
    }

    /// The harness and account a worker on `harness` has its mutants picked
    /// on: the first other harness of the kit `asks` says can be asked, else
    /// `harness` again (a new session), whatever a pin says, on the account
    /// `pick` picks.
    pub fn choose_picker(
        &self,
        harness: &str,
        asks: impl Fn(&str) -> bool,
        now: i64,
        pick: impl FnMut(&str) -> Result<Picked, String>,
    ) -> Result<Choice, String> {
        let (others, same): (Vec<Entry>, Vec<Entry>) =
            self.entries.iter().filter(|e| asks(&e.harness)).cloned().partition(|e| e.harness != harness);
        let kit = Kit { entries: [others, same].concat(), pins: BTreeMap::new(), ..self.clone() };
        kit.choose("", now, pick).map_err(|why| format!("no harness of the kit that can pick mutants: {why}"))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KitEvent {
    KitSet {
        entries: Vec<Entry>,
    },
    CheckoutPinned {
        checkout: String,
        harness: Option<String>,
    },
    /// An account was used up until then (ms), as factorium once kept its
    /// accounts' limits: read, and changes nothing. stablehand keeps them now.
    AccountLimited {
        name: String,
        until: i64,
    },
    /// The host's own login of a harness is at its limit until then (ms).
    HarnessLimited(HarnessNote),
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
            KitEvent::AccountLimited { .. } => {}
            KitEvent::HarnessLimited(n) => _ = s.limits.insert(n.harness, n.until),
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
pub struct HarnessNote {
    pub harness: String,
    pub until: i64,
}

// The host's own login of a harness reached its engine's limit: no
// worker starts on it before then; a later reset is kept.
command!(
    LimitHarness,
    "harness.limited",
    Kits,
    HarnessNote,
    |_p| kit_id(),
    |s, p, now| Ok::<_, factum_core::Rejection>(if s.limits.get(&p.harness).is_none_or(|u| *u < p.until) {
        vec![KitEvent::HarnessLimited(p.clone())]
    } else {
        Vec::new()
    })
);

#[cfg(test)]
mod tests {
    use super::*;

    /// Which other harness is first, and that the pin does not choose it.
    #[test]
    fn a_picker_runs_on_the_kit_s_other_harness_that_asks_else_on_the_worker_s() {
        let entry = |h: &str| Entry { harness: h.into(), command: h.into(), model: None };
        let pins = BTreeMap::from([("web".to_string(), "a".to_string())]);
        let kit = Kit { entries: vec![entry("a"), entry("b"), entry("c")], pins, ..Kit::default() };
        let picked = |worker: &str, asks: &[&str]| {
            let asks = |h: &str| asks.contains(&h);
            kit.choose_picker(worker, asks, 0, |_| Ok(Picked::None(Vec::new()))).map(|c| c.entry.harness)
        };
        assert_eq!(picked("a", &["a", "b"]), Ok("b".into()));
        assert_eq!(picked("b", &["a", "b"]), Ok("a".into()));
        assert_eq!(picked("a", &["a", "c"]), Ok("c".into()), "the pin is the worker's, not the picker's");
        assert_eq!(picked("a", &["a"]), Ok("a".into()));
        assert!(picked("a", &[]).unwrap_err().starts_with("no harness of the kit that can pick mutants"));
    }

    fn unpicked(account: &str, reason: &str) -> Unpicked {
        Unpicked { account: account.into(), reason: reason.into(), why: format!("{account} is {reason}") }
    }

    /// The first harness whose pick is an account; one with no account here
    /// (or only elsewhere) runs on the host's own login, unless at its limit.
    #[test]
    fn a_worker_starts_on_the_first_harness_stablehand_picks_an_account_of() {
        let entry = |h: &str| Entry { harness: h.into(), command: h.into(), model: None };
        let kit = Kit { entries: vec![entry("a"), entry("b")], ..Kit::default() };
        let chosen = |kit: &Kit, a: Picked, b: Picked| {
            kit.choose("web", 0, |h| Ok(if h == "a" { a.clone() } else { b.clone() }))
                .map(|c| (c.entry.harness, c.account))
        };
        let full = Picked::None(vec![unpicked("x", "full")]);
        let on = |n: &str| Picked::On(n.into());
        assert_eq!(chosen(&kit, on("x"), on("y")), Ok(("a".into(), Some("x".into()))));
        assert_eq!(chosen(&kit, full.clone(), on("y")), Ok(("b".into(), Some("y".into()))));
        let away = Picked::None(vec![unpicked("x", "elsewhere")]);
        assert_eq!(chosen(&kit, away.clone(), on("y")), Ok(("a".into(), None)), "the host's own login");
        let why = chosen(&kit, full.clone(), full.clone()).unwrap_err();
        assert!(why.contains("a: stablehand picks no account (x: x is full)"), "{why}");
        let limited = Kit { limits: BTreeMap::from([("a".to_string(), 10)]), ..kit.clone() };
        assert_eq!(chosen(&limited, away, on("y")), Ok(("b".into(), Some("y".into()))));
        let failed = kit.choose("web", 0, |_| Err("stablehand: not found".to_string()));
        assert!(failed.unwrap_err().starts_with("stablehand: not found; without stablehand only claude-code starts"));
        let with_claude = Kit { entries: vec![entry("a"), entry("claude-code")], ..Kit::default() };
        let fell = with_claude.choose("web", 0, |_| Err("stablehand: not found".to_string())).unwrap();
        assert_eq!((fell.entry.harness.as_str(), fell.account), ("claude-code", None), "its own login, alone");
        let spent = Kit { limits: BTreeMap::from([("claude-code".to_string(), 10)]), ..with_claude.clone() };
        assert!(spent.choose("web", 0, |_| Err("down".into())).unwrap_err().contains("at its limit"));
        let pins = BTreeMap::from([("web".to_string(), "a".to_string())]);
        let pinned = Kit { pins, ..with_claude };
        assert!(pinned.choose("web", 0, |_| Err("down".into())).is_err(), "a pin to another harness starts none");
    }

    /// A named account starts on its harness's entry unless its pick passed it over.
    #[test]
    fn a_named_account_starts_unless_stablehand_passed_it_over() {
        let entry = |h: &str| Entry { harness: h.into(), command: h.into(), model: None };
        let pins = BTreeMap::from([("web".to_string(), "b".to_string())]);
        let kit = Kit { entries: vec![entry("a"), entry("b")], pins, ..Kit::default() };
        let other = Picked::On("y".into());
        assert_eq!(kit.choose_on("app", "x", "b", &other).unwrap().entry.harness, "b", "not picked, not passed over");
        let passed = Picked::None(vec![unpicked("x", "limit")]);
        assert_eq!(kit.choose_on("app", "x", "b", &passed).unwrap_err(), "account x takes no session now: x is limit");
        assert!(kit.choose_on("web", "x", "a", &other).unwrap_err().contains("pinned to b"));
        assert!(kit.choose_on("app", "x", "c", &other).unwrap_err().contains("not in the kit"));
    }
}
