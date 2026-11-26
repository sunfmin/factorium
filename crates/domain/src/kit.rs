//! The kit: the checkouts pinned to one engine, and the host's own login's
//! limits. Which engine, model and account a session starts on, and the
//! command each engine starts with, are stablehand's (`stablehand pick`,
//! `stablehand command show`); an engine it keeps none for starts on its
//! bare program. Without stablehand a session starts on [`FALLBACK`] alone,
//! by [`FALLBACK_COMMAND`], on the host user's own login, which rests on its
//! own limits here.

use std::collections::BTreeMap;

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{command, id};

pub fn kit_id() -> factum_core::Uuid {
    id("kit")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Kit {
    /// Engine (a harness's key) → the command the kit kept for it before
    /// stablehand kept them (factorium#306): moved to stablehand once
    /// ([`KitEvent::CommandsMoved`]), then empty. No start reads it.
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
    /// Engine → the command the move found and stablehand refused, and why:
    /// a finding of doctor until stablehand keeps one for that engine.
    #[serde(default)]
    pub refused: BTreeMap<String, Refused>,
    /// Checkout name → the engine it is pinned to.
    pub pins: BTreeMap<String, String>,
    /// Engine → until when (ms) the host's own login of it is at its
    /// engine's limit: what a session without stablehand starts on.
    #[serde(default)]
    pub limits: BTreeMap<String, i64>,
}

/// The engine a worker starts on when stablehand cannot be asked: on the
/// host user's own login, and no other, since without stablehand no other
/// engine's accounts or windows are known.
pub const FALLBACK: &str = "claude-code";

/// The command [`FALLBACK`] starts with when stablehand cannot be asked,
/// for that case alone: stablehand keeps every other start's command.
pub const FALLBACK_COMMAND: &str = "claude --dangerously-skip-permissions";

/// A command of the kit's that stablehand refused to keep, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub command: String,
    pub why: String,
}

/// What a session starts on: an engine, the command it starts with, a model
/// (`None`: the engine's own) and an account (`None`: the host user's own
/// login).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub harness: String,
    pub command: String,
    pub model: Option<String>,
    pub account: Option<String>,
}

/// What a start asks stablehand's pick (`stablehand pick`): the walk
/// narrowed to these engines (`--engine`; none: the whole order) and this
/// model (`--model`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ask {
    pub engines: Vec<String>,
    pub model: Option<String>,
}

/// What stablehand's pick answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picked {
    /// The engine, model (`None`: the engine's own) and account one more
    /// session starts on.
    On { engine: String, model: Option<String>, account: String },
    /// None now: until when (RFC 3339; `None`: no time is known), and each
    /// step passed over, and why.
    None { until: Option<String>, why: Vec<Unpicked> },
}

/// A step stablehand's pick passed over, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unpicked {
    /// `None`: its engine was skipped whole (`no_account`).
    pub account: Option<String>,
    pub why: String,
}

impl Picked {
    /// Why none starts now, in words: until when, and each step passed over.
    fn none_why(until: &Option<String>, why: &[Unpicked]) -> String {
        let each: Vec<&str> = why.iter().map(|u| u.why.as_str()).collect();
        let until = until.as_deref().map_or_else(String::new, |t| format!(" until {t}"));
        format!("stablehand picks none{until} ({})", each.join("; "))
    }
}

impl Kit {
    /// What a start of `checkout` asks stablehand's pick: its pin's engine
    /// alone, if it has one, and `model`, the issue's own, if it names one.
    pub fn ask(&self, checkout: &str, model: Option<&str>) -> Ask {
        Ask { engines: self.pins.get(checkout).cloned().into_iter().collect(), model: model.map(Into::into) }
    }

    /// What a start of `checkout` starts on, from what stablehand's pick
    /// answered to [`Kit::ask`]: its engine, model and account, with
    /// `commands`' command for that engine (each engine's: the one
    /// stablehand keeps, else its bare program). A pick that failed
    /// (stablehand not found, or a fault) leaves [`FALLBACK`] alone, by
    /// [`FALLBACK_COMMAND`], on the host user's own login, where the pin and
    /// that login's limit allow. `Err`: why none can.
    pub fn choose(
        &self,
        checkout: &str,
        model: Option<&str>,
        now: i64,
        commands: &BTreeMap<String, String>,
        picked: Result<Picked, String>,
    ) -> Result<Choice, String> {
        match picked {
            Ok(Picked::On { engine, model, account }) => on(commands, engine, model, Some(account)),
            Ok(Picked::None { until, why }) => Err(Picked::none_why(&until, &why)),
            Err(fault) => self.fallback(self.pins.get(checkout).is_none_or(|p| p == FALLBACK), model, now, fault),
        }
    }

    /// What a worker of `checkout` whose issue names `account`, of the
    /// engine `engine`, starts on, from what stablehand's pick of that
    /// engine answered: that account, on the model the pick answered (else
    /// `model`, the issue's own), by `commands`' command for that engine.
    /// Refused when `checkout` is pinned to another engine, or the pick
    /// passed the account over.
    pub fn choose_on(
        &self,
        checkout: &str,
        account: &str,
        engine: &str,
        model: Option<&str>,
        commands: &BTreeMap<String, String>,
        picked: &Picked,
    ) -> Result<Choice, String> {
        if let Some(p) = self.pins.get(checkout).filter(|p| *p != engine) {
            return Err(format!("account {account} is of {engine}, and {checkout} is pinned to {p}"));
        }
        let model = match picked {
            Picked::On { model: m, .. } => m.clone().or(model.map(Into::into)),
            Picked::None { why, .. } => {
                if let Some(u) = why.iter().find(|u| u.account.as_deref() == Some(account)) {
                    return Err(format!("account {account} takes no session now: {}", u.why));
                }
                model.map(Into::into)
            }
        };
        on(commands, engine.into(), model, Some(account.into()))
    }

    /// What a worker on `harness` has its mutants picked on, from
    /// stablehand's pick of each of `asks` in turn (the engines of
    /// `commands` that can be asked headless other than `harness`, then
    /// `harness` again, a new session), whatever a pin says. `Err`: why
    /// none can.
    pub fn choose_picker(
        &self,
        harness: &str,
        asks: impl Fn(&str) -> bool,
        now: i64,
        commands: &BTreeMap<String, String>,
        mut pick: impl FnMut(&Ask) -> Result<Picked, String>,
    ) -> Result<Choice, String> {
        let engines = commands.keys().filter(|e| asks(e));
        let (others, same): (Vec<String>, Vec<String>) = engines.cloned().partition(|e| e != harness);
        let mut why = Vec::new();
        for engines in [others, same].into_iter().filter(|e| !e.is_empty()) {
            let fallback = engines.iter().any(|e| e == FALLBACK);
            match pick(&Ask { engines, model: None }) {
                Ok(Picked::On { engine, model, account }) => return on(commands, engine, model, Some(account)),
                Ok(Picked::None { until, why: passed }) => why.push(Picked::none_why(&until, &passed)),
                Err(fault) => return self.fallback(fallback, None, now, fault),
            }
        }
        if why.is_empty() {
            why.push("no engine can be asked headless".into());
        }
        Err(format!("no harness that can pick mutants: {}", why.join("; ")))
    }

    /// What a session starts on when stablehand could not be asked
    /// (`fault`): [`FALLBACK`] by [`FALLBACK_COMMAND`], where `allowed`, on
    /// the host user's own login unless that is at its limit.
    fn fallback(&self, allowed: bool, model: Option<&str>, now: i64, fault: String) -> Result<Choice, String> {
        if !allowed {
            return Err(format!("{fault}; without stablehand only {FALLBACK} starts, and it is not to be had here"));
        }
        match self.limits.get(FALLBACK).copied().filter(|t| *t > now) {
            None => Ok(Choice {
                harness: FALLBACK.into(),
                command: FALLBACK_COMMAND.into(),
                model: model.map(Into::into),
                account: None,
            }),
            Some(t) => Err(format!("{fault}; {FALLBACK} on the host's own login is at its limit until {t} (ms)")),
        }
    }
}

/// A session of `engine` on `model` and `account`, by `commands`' command
/// for it; refused when factorium has no harness of that engine.
fn on(
    commands: &BTreeMap<String, String>,
    engine: String,
    model: Option<String>,
    account: Option<String>,
) -> Result<Choice, String> {
    match commands.get(&engine) {
        Some(command) => Ok(Choice { command: command.clone(), harness: engine, model, account }),
        None => Err(format!("stablehand picks {engine}, which factorium has no harness of")),
    }
}

/// A kit entry as factorium once kept its order: read from the events
/// written then, its command kept, its order and model (not read) left to
/// stablehand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub harness: String,
    pub command: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KitEvent {
    /// The kit as factorium once kept it, in order with models: read as
    /// its commands alone, the first of each engine's.
    KitSet {
        entries: Vec<Entry>,
    },
    /// The kit's commands, each engine's, as factorium once kept them.
    CommandsSet {
        commands: BTreeMap<String, String>,
    },
    /// The kit's commands moved to stablehand (factorium#306): dropped
    /// here, those it refused kept for doctor.
    CommandsMoved {
        refused: BTreeMap<String, Refused>,
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
            KitEvent::KitSet { entries } => {
                s.commands = entries.into_iter().rev().map(|e| (e.harness, e.command)).collect()
            }
            KitEvent::CommandsSet { commands } => s.commands = commands,
            KitEvent::CommandsMoved { refused } => (s.commands, s.refused) = (BTreeMap::new(), refused),
            KitEvent::CheckoutPinned { checkout, harness: Some(h) } => _ = s.pins.insert(checkout, h),
            KitEvent::CheckoutPinned { checkout, harness: None } => _ = s.pins.remove(&checkout),
            KitEvent::AccountLimited { .. } => {}
            KitEvent::HarnessLimited(n) => _ = s.limits.insert(n.harness, n.until),
        }
    }

    /// "3": the kit's commands move to stablehand, those it refused kept.
    fn code() -> &'static str {
        "3"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Moved {
    pub refused: BTreeMap<String, Refused>,
}

// The kit's commands were set in stablehand, those it refused named:
// once, while the kit holds any.
command!(MoveCommands, "kit.moved", Kits, Moved, |_p| kit_id(), |s, p, now| {
    crate::only(!s.commands.is_empty(), KitEvent::CommandsMoved { refused: p.refused.clone() })
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
command!(LimitHarness, "harness.limited", Kits, HarnessNote, |_p| kit_id(), |s, p, now| {
    crate::only(s.limits.get(&p.harness).is_none_or(|u| *u < p.until), KitEvent::HarnessLimited(p.clone()))
});
