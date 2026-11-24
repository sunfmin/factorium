//! The kit: how this host starts each engine (its launch command), and the
//! checkouts pinned to one. Which engine, model and account a session
//! starts on is stablehand's (`stablehand pick`, walking its order, the one
//! home of the order and the models: stablehand#24); factorium starts the
//! answer with the kit's command for its engine. Without stablehand a
//! session starts on [`FALLBACK`] alone, on the host user's own login,
//! which rests on its own limits here.

use std::collections::BTreeMap;

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{command, id, no};

pub fn kit_id() -> factum_core::Uuid {
    id("kit")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Kit {
    /// Engine (a harness's key) → the line that launches it, e.g.
    /// `claude --dangerously-skip-permissions`.
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
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

/// What a session starts on: an engine, the kit's command for it, a model
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
    /// answered to [`Kit::ask`]: its engine, model and account, with the
    /// kit's command for that engine. A pick that failed (stablehand not
    /// found, or a fault) leaves [`FALLBACK`] alone, on the host user's own
    /// login, where the pin and that login's limit allow. `Err`: why none
    /// can.
    pub fn choose(
        &self,
        checkout: &str,
        model: Option<&str>,
        now: i64,
        picked: Result<Picked, String>,
    ) -> Result<Choice, String> {
        match picked {
            Ok(Picked::On { engine, model, account }) => self.on(engine, model, Some(account)),
            Ok(Picked::None { until, why }) => Err(Picked::none_why(&until, &why)),
            Err(fault) => self.fallback(self.pins.get(checkout).is_none_or(|p| p == FALLBACK), model, now, fault),
        }
    }

    /// What a worker of `checkout` whose issue names `account`, of the
    /// engine `engine`, starts on, from what stablehand's pick of that
    /// engine answered: that account, on the model the pick answered (else
    /// `model`, the issue's own). Refused when `checkout` is pinned to
    /// another engine, or the pick passed the account over.
    pub fn choose_on(
        &self,
        checkout: &str,
        account: &str,
        engine: &str,
        model: Option<&str>,
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
        self.on(engine.into(), model, Some(account.into()))
    }

    /// What a worker on `harness` has its mutants picked on, from
    /// stablehand's pick of each of `asks` in turn (the engines of the kit
    /// that can be asked headless other than `harness`, then `harness`
    /// again, a new session), whatever a pin says. `Err`: why none can.
    pub fn choose_picker(
        &self,
        harness: &str,
        asks: impl Fn(&str) -> bool,
        now: i64,
        mut pick: impl FnMut(&Ask) -> Result<Picked, String>,
    ) -> Result<Choice, String> {
        let engines = self.commands.keys().filter(|e| asks(e));
        let (others, same): (Vec<String>, Vec<String>) = engines.cloned().partition(|e| e != harness);
        let mut why = Vec::new();
        for engines in [others, same].into_iter().filter(|e| !e.is_empty()) {
            let fallback = engines.iter().any(|e| e == FALLBACK);
            match pick(&Ask { engines, model: None }) {
                Ok(Picked::On { engine, model, account }) => return self.on(engine, model, Some(account)),
                Ok(Picked::None { until, why: passed }) => why.push(Picked::none_why(&until, &passed)),
                Err(fault) => return self.fallback(fallback, None, now, fault),
            }
        }
        if why.is_empty() {
            why.push("the kit has no command of an engine that can be asked".into());
        }
        Err(format!("no harness of the kit that can pick mutants: {}", why.join("; ")))
    }

    /// A session of `engine` on `model` and `account`, by the kit's command
    /// for it; refused, naming the fix, when the kit has none.
    fn on(&self, engine: String, model: Option<String>, account: Option<String>) -> Result<Choice, String> {
        match self.commands.get(&engine) {
            Some(command) => Ok(Choice { command: command.clone(), harness: engine, model, account }),
            None => Err(format!(
                "stablehand picks {engine}, and the kit has no command for it: `factorium kit set {engine}=<command>` \
                 (with the kit's other commands), or take it out of `stablehand order`"
            )),
        }
    }

    /// What a session starts on when stablehand could not be asked
    /// (`fault`): [`FALLBACK`], where `allowed`, on the host user's own
    /// login unless that is at its limit.
    fn fallback(&self, allowed: bool, model: Option<&str>, now: i64, fault: String) -> Result<Choice, String> {
        let Some(command) = self.commands.get(FALLBACK).filter(|_| allowed) else {
            return Err(format!("{fault}; without stablehand only {FALLBACK} starts, and it is not to be had here"));
        };
        match self.limits.get(FALLBACK).copied().filter(|t| *t > now) {
            None => Ok(Choice {
                harness: FALLBACK.into(),
                command: command.clone(),
                model: model.map(Into::into),
                account: None,
            }),
            Some(t) => Err(format!("{fault}; {FALLBACK} on the host's own login is at its limit until {t} (ms)")),
        }
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
    /// The kit's commands, each engine's.
    CommandsSet {
        commands: BTreeMap<String, String>,
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
            KitEvent::CheckoutPinned { checkout, harness: Some(h) } => _ = s.pins.insert(checkout, h),
            KitEvent::CheckoutPinned { checkout, harness: None } => _ = s.pins.remove(&checkout),
            KitEvent::AccountLimited { .. } => {}
            KitEvent::HarnessLimited(n) => _ = s.limits.insert(n.harness, n.until),
        }
    }

    /// "2": the kit keeps commands by engine, its order and models gone.
    fn code() -> &'static str {
        "2"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetKit {
    pub commands: BTreeMap<String, String>,
}

command!(SetTheKit, "kit.set", Kits, SetKit, |_p| kit_id(), |s, p, now| {
    for (engine, command) in &p.commands {
        if command.trim().is_empty() {
            return no("no_command", format!("{engine} needs a launch command"));
        }
    }
    Ok(vec![KitEvent::CommandsSet { commands: p.commands.clone() }])
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

    fn kit(engines: &[&str]) -> Kit {
        Kit { commands: engines.iter().map(|e| (e.to_string(), format!("{e}-cmd"))).collect(), ..Kit::default() }
    }

    fn on(engine: &str, model: Option<&str>, account: &str) -> Picked {
        Picked::On { engine: engine.into(), model: model.map(Into::into), account: account.into() }
    }

    fn unpicked(engine: &str, account: Option<&str>, reason: &str) -> Unpicked {
        Unpicked { account: account.map(Into::into), why: format!("{}: {reason}", account.unwrap_or(engine)) }
    }

    /// A start asks its pin's engine and its issue's model, and starts what
    /// the pick answers whole with the kit's command for it.
    #[test]
    fn a_start_is_the_pick_s_engine_model_and_account_on_the_kit_s_command() {
        let k = Kit { pins: BTreeMap::from([("web".into(), "codex".into())]), ..kit(&["codex", "claude-code"]) };
        assert_eq!(k.ask("web", Some("o3")), Ask { engines: vec!["codex".into()], model: Some("o3".into()) });
        assert_eq!(k.ask("app", None), Ask::default());
        let c = k.choose("app", None, 0, Ok(on("codex", Some("o3"), "x"))).unwrap();
        assert_eq!(
            c,
            Choice {
                harness: "codex".into(),
                command: "codex-cmd".into(),
                model: Some("o3".into()),
                account: Some("x".into())
            }
        );
        let none = Picked::None {
            until: Some("2030-01-01T00:00:00Z".into()),
            why: vec![unpicked("codex", Some("x"), "limit")],
        };
        assert_eq!(
            k.choose("app", None, 0, Ok(none)).unwrap_err(),
            "stablehand picks none until 2030-01-01T00:00:00Z (x: limit)"
        );
        let why = k.choose("app", None, 0, Ok(on("grok", None, "g"))).unwrap_err();
        assert!(why.contains("`factorium kit set grok=<command>`"), "{why}");
    }

    /// Without stablehand, claude-code alone on the host's own login, where
    /// the pin and that login's limit allow.
    #[test]
    fn without_stablehand_claude_code_alone_starts_on_the_host_s_login() {
        let k = kit(&["codex", "claude-code"]);
        let fell = k.choose("web", Some("opus"), 0, Err("down".into())).unwrap();
        assert_eq!((fell.harness.as_str(), fell.model.as_deref(), fell.account), ("claude-code", Some("opus"), None));
        let spent = Kit { limits: BTreeMap::from([("claude-code".into(), 10)]), ..k.clone() };
        assert!(spent.choose("web", None, 0, Err("down".into())).unwrap_err().contains("at its limit"));
        let pinned = Kit { pins: BTreeMap::from([("web".into(), "codex".into())]), ..k.clone() };
        assert!(pinned.choose("web", None, 0, Err("down".into())).unwrap_err().starts_with("down; without stablehand"));
        assert!(kit(&["codex"]).choose("web", None, 0, Err("down".into())).is_err());
    }

    /// A named account starts unless its pick passed it over.
    #[test]
    fn a_named_account_starts_unless_stablehand_passed_it_over() {
        let k = Kit { pins: BTreeMap::from([("web".into(), "codex".into())]), ..kit(&["codex", "claude-code"]) };
        let other = on("claude-code", Some("opus"), "y");
        let c = k.choose_on("app", "x", "claude-code", None, &other).unwrap();
        assert_eq!((c.account.as_deref(), c.model.as_deref()), (Some("x"), Some("opus")));
        let passed = Picked::None { until: None, why: vec![unpicked("claude-code", Some("x"), "limit")] };
        assert_eq!(
            k.choose_on("app", "x", "claude-code", None, &passed).unwrap_err(),
            "account x takes no session now: x: limit"
        );
        assert!(k.choose_on("web", "x", "claude-code", None, &other).unwrap_err().contains("pinned to codex"));
        assert!(k.choose_on("app", "x", "grok", None, &other).unwrap_err().contains("kit set grok"));
    }

    /// The picker asks the other engines that ask first, then the worker's.
    #[test]
    fn a_picker_runs_on_another_engine_that_asks_else_on_the_worker_s() {
        let k = kit(&["a", "b", "c"]);
        let none = || Picked::None { until: None, why: Vec::new() };
        let asked = |worker: &str, asks: &[&str], first: Picked| {
            let mut seen = Vec::new();
            let mut answers = vec![first, on(worker, None, "w")].into_iter();
            let asks = |h: &str| asks.contains(&h);
            let c = k.choose_picker(worker, asks, 0, |a| {
                seen.push(a.engines.clone());
                Ok(answers.next().unwrap())
            });
            (c.map(|c| c.harness), seen)
        };
        let (c, seen) = asked("a", &["a", "b"], on("b", None, "x"));
        assert_eq!((c, seen), (Ok("b".into()), vec![vec!["b".to_string()]]));
        let (c, seen) = asked("a", &["a", "b", "c"], none());
        assert_eq!((c, seen), (Ok("a".into()), vec![vec!["b".to_string(), "c".into()], vec!["a".into()]]));
        let (c, _) = asked("a", &[], none());
        assert!(c.unwrap_err().starts_with("no harness of the kit that can pick mutants"));
    }

    /// A snapshot of the kit as code "1" folded it holds no commands, so
    /// the fold names other code and such a snapshot is not read.
    #[test]
    fn a_snapshot_of_the_kit_s_old_shape_is_not_read() {
        let old: Kit =
            serde_json::from_str(r#"{"entries": [{"harness": "codex", "command": "x"}], "pins": {}}"#).unwrap();
        assert!(old.commands.is_empty(), "read as this kit, it would have lost its commands");
        assert!(!["", "1"].contains(&Kits::code()), "{}", Kits::code());
    }

    /// The kit set as factorium once kept it reads as its commands.
    #[test]
    fn an_old_kit_reads_as_its_commands() {
        let mut k = Kit::default();
        let old = r#"{"type": "KitSet", "data": {"entries": [{"harness": "codex", "command": "first", "model": "m"},
            {"harness": "codex", "command": "second", "model": null}]}}"#;
        Kits::evolve(&mut k, serde_json::from_str(old).unwrap());
        assert_eq!(k.commands, BTreeMap::from([("codex".to_string(), "first".to_string())]));
    }
}
