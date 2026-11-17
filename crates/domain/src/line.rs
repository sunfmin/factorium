//! A checkout's proving line: what landed on the proving branch, what was
//! promoted to the base branch, and the versions cut from it.
//!
//! The line decides what is due on its checkout now, and why not
//! ([`Line::due`]); its commands record what the app did and refuse what
//! the line does not hold. The app does the git and gate work it is told.

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use factum_core::Uuid;

use crate::settings::{Settings, Time};
use crate::worker::Status;
use crate::{command, line_id, no};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Landing {
    pub number: u64,
    pub commit: String,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Landed, oldest first, and not yet promoted.
    pub unpromoted: Vec<Landing>,
    /// Promoted, and not yet in a cut: when each landed.
    pub uncut: Vec<i64>,
    pub promoted: Option<String>,
    /// The proving tip whose slow gate went red, while it is the tip.
    pub red: Option<(String, String)>,
    pub version: Option<String>,
    /// The version whose cut failed, and why, until a cut succeeds.
    pub cut_failed: Option<(String, String)>,
    /// Landings held by the owner or the watch.
    pub held: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum LineEvent {
    Landed { number: u64, commit: String, at: i64 },
    PromotionRed { tip: String, why: String },
    Promoted { tip: String, at: i64 },
    CutMade { version: String, commit: String },
    CutFailed { version: String, why: String },
    LandingsHeld { why: String },
    LandingsReleased,
}

pub struct Lines;

impl Aggregate for Lines {
    const NAME: &'static str = "line";
    type State = Line;
    type Event = LineEvent;

    fn evolve(s: &mut Line, event: LineEvent) {
        match event {
            LineEvent::Landed { number, commit, at } => s.unpromoted.push(Landing { number, commit, at }),
            LineEvent::PromotionRed { tip, why } => s.red = Some((tip, why)),
            LineEvent::Promoted { tip, .. } => {
                // Everything landed up to the tip is promoted with it.
                let n = s.unpromoted.iter().position(|l| l.commit == tip).map_or(s.unpromoted.len(), |i| i + 1);
                s.uncut.extend(s.unpromoted.drain(..n).map(|l| l.at));
                (s.promoted, s.red) = (Some(tip), None);
            }
            LineEvent::CutMade { version, .. } => {
                s.uncut.clear();
                (s.version, s.cut_failed) = (Some(version), None);
            }
            LineEvent::CutFailed { version, why } => s.cut_failed = Some((version, why)),
            LineEvent::LandingsHeld { why } => s.held = Some(why),
            LineEvent::LandingsReleased => s.held = None,
        }
    }
}

/// One of the line's three moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Land,
    Promote,
    Cut,
}

/// What is due: the move, and for a landing the worker it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    Land(Uuid),
    Promote,
    Cut,
}

/// What the app sees of the checkout now, for the line to judge.
pub struct Look<'a> {
    pub now: i64,
    pub settings: &'a Settings,
    /// The checkout's live workers and their status, oldest status first.
    pub workers: &'a [(Uuid, Status)],
    /// Asked for by hand: the bounds and a red tip or failed cut do not hold it.
    pub by_hand: bool,
    /// The proving tip, once read. A promotion is asked without it first,
    /// so the tip is read only when everything else is due.
    pub tip: Option<&'a str>,
}

impl Look<'_> {
    /// No worker of the checkout works or lands; those waiting for the watch do not count.
    fn idle(&self) -> bool {
        self.workers.iter().all(|(_, s)| *s == Status::Waiting)
    }

    /// Whether what began at `since` has waited `bound`.
    fn waited(&self, since: i64, bound: Time) -> bool {
        self.now - since >= self.settings.ms(bound)
    }
}

impl Line {
    /// Whether `mv` is due on this checkout now, or why not.
    pub fn due(&self, mv: Move, look: &Look) -> Result<Due, String> {
        match mv {
            Move::Land => {
                if let Some(why) = &self.held {
                    return Err(format!("landings are held: {why}"));
                }
                // One landing per checkout at a time: the one that asked first.
                let first = look.workers.iter().find(|(_, s)| *s == Status::Landing);
                first.map(|(w, _)| Due::Land(*w)).ok_or_else(|| "no worker asks to land".into())
            }
            Move::Promote => {
                let Some(oldest) = self.unpromoted.first() else { return Err("nothing to promote".into()) };
                if !(look.by_hand || look.idle() || look.waited(oldest.at, Time::PromotionBound)) {
                    return Err("the checkout is busy and its oldest landing waits within the promotion bound".into());
                }
                match (&self.red, look.tip) {
                    (Some((red, why)), Some(tip)) if red == tip && !look.by_hand => {
                        Err(format!("the slow gate is red on the proving tip {tip}: {why}"))
                    }
                    _ => Ok(Due::Promote),
                }
            }
            Move::Cut => {
                let Some(&oldest) = self.uncut.first() else {
                    return Err("nothing promoted since the last cut".into());
                };
                if let Some((version, why)) = self.cut_failed.as_ref().filter(|_| !look.by_hand) {
                    return Err(format!("cutting {version} failed; a cut by hand tries again: {why}"));
                }
                let settled = look.idle() && self.unpromoted.is_empty();
                if !(look.by_hand || settled || look.waited(oldest, Time::CutBound)) {
                    return Err(
                        "the checkout is busy or unpromoted, and its oldest promotion waits within the cut bound"
                            .into(),
                    );
                }
                Ok(Due::Cut)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Landed {
    pub checkout: String,
    pub number: u64,
    pub commit: String,
}

command!(RecordLanding, "line.landed", Lines, Landed, |p| line_id(&p.checkout), |s, p, now| {
    if s.unpromoted.iter().any(|l| l.commit == p.commit) {
        return no("landed_already", format!("{} landed already", p.commit));
    }
    Ok(vec![LineEvent::Landed { number: p.number, commit: p.commit.clone(), at: now.0 }])
});

/// What a promotion found: the slow gate on `tip`, and whether the base
/// it read is an ancestor of the tip.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Promotion {
    pub checkout: String,
    pub tip: String,
    pub base: String,
    /// The slow gate's tail when it was red.
    pub red: Option<String>,
    /// The base it read is an ancestor of the tip.
    pub behind: bool,
}

command!(Promote, "line.promote", Lines, Promotion, |p| line_id(&p.checkout), |s, p, now| {
    if s.unpromoted.is_empty() {
        return no("nothing_to_promote", "nothing landed since the last promotion");
    }
    Ok(vec![match p.red() {
        Some(why) => LineEvent::PromotionRed { tip: p.tip.clone(), why },
        None => LineEvent::Promoted { tip: p.tip.clone(), at: now.0 },
    }])
});

impl Promotion {
    /// Why the base may not be moved to the tip, if it may not: its slow
    /// gate is red, or the base moved outside factorium, its one writer.
    pub fn red(&self) -> Option<String> {
        self.red.clone().or_else(|| {
            (!self.behind).then(|| {
                format!("the base {} is not an ancestor of the proving tip; it moved outside factorium", self.base)
            })
        })
    }
}

/// What a cut did: the version, the commit it tagged, or why it failed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cut {
    pub checkout: String,
    pub version: String,
    pub commit: String,
    pub failed: Option<String>,
}

command!(RecordCut, "line.cut", Lines, Cut, |p| line_id(&p.checkout), |s, p, now| {
    if s.uncut.is_empty() {
        return no("nothing_to_cut", "nothing promoted since the last cut");
    }
    Ok(vec![match &p.failed {
        Some(why) => LineEvent::CutFailed { version: p.version.clone(), why: why.clone() },
        None => LineEvent::CutMade { version: p.version.clone(), commit: p.commit.clone() },
    }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hold {
    pub checkout: String,
    /// `None` releases the hold.
    pub why: Option<String>,
}

command!(HoldLandings, "line.hold", Lines, Hold, |p| line_id(&p.checkout), |s, p, now| match (&p.why, &s.held) {
    (Some(w), _) if w.trim().is_empty() => no("no_reason", "a hold needs a reason"),
    (Some(w), _) => Ok(vec![LineEvent::LandingsHeld { why: w.clone() }]),
    (None, Some(_)) => Ok(vec![LineEvent::LandingsReleased]),
    (None, None) => Ok(Vec::new()),
});

/// The next version after `last`: its patch number plus one; `v0.1.0` first.
pub fn next_version(last: Option<&str>) -> String {
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        let mut it = v.trim_start_matches('v').split('.').map(|n| n.parse::<u64>().ok());
        Some((it.next()??, it.next()??, it.next()??))
    };
    match last.and_then(parse) {
        Some((a, b, c)) => format!("v{a}.{b}.{}", c + 1),
        None => "v0.1.0".into(),
    }
}
