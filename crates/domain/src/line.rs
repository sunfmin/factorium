//! A checkout's proving line: what landed on the proving branch, what was
//! promoted to the base branch, and the versions cut from it.

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Landed {
    pub checkout: String,
    pub number: u64,
    pub commit: String,
}

command!(RecordLanding, "line.landed", Lines, Landed, |p| line_id(&p.checkout), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![LineEvent::Landed { number: p.number, commit: p.commit.clone(), at: now.0 }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tip {
    pub checkout: String,
    pub tip: String,
    #[serde(default)]
    pub why: String,
}

command!(RecordPromotion, "line.promoted", Lines, Tip, |p| line_id(&p.checkout), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![LineEvent::Promoted { tip: p.tip.clone(), at: now.0 }])
});

command!(RedPromotion, "line.promotion_red", Lines, Tip, |p| line_id(&p.checkout), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![LineEvent::PromotionRed { tip: p.tip.clone(), why: p.why.clone() }])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cut {
    pub checkout: String,
    pub version: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub why: String,
}

command!(RecordCut, "line.cut", Lines, Cut, |p| line_id(&p.checkout), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![LineEvent::CutMade { version: p.version.clone(), commit: p.commit.clone() }])
});

command!(FailCut, "line.cut_failed", Lines, Cut, |p| line_id(&p.checkout), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![LineEvent::CutFailed { version: p.version.clone(), why: p.why.clone() }])
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
