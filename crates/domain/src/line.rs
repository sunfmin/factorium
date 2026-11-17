//! A checkout's proving line: what landed on the proving branch, what was
//! promoted to the base branch, and the versions cut from it.
//!
//! The line decides what is due on its checkout now, and why not
//! ([`Line::due`], neutral: [`crate::neutral::landing`]); its commands
//! record what the app did and refuse what the line does not hold. The app
//! does the git and gate work it is told.

use factum_core::{Aggregate, Uuid};
use serde::{Deserialize, Serialize};

pub use crate::neutral::landing::{Due, Landing, Line, LineEvent, Move, NotDue};
use crate::settings::{Settings, Time};
use crate::worker::Status;
use crate::{command, line_id, no};

pub struct Lines;

impl Aggregate for Lines {
    const NAME: &'static str = "line";
    type State = Line;
    type Event = LineEvent;

    fn evolve(s: &mut Line, event: LineEvent) {
        s.apply(event)
    }
}

/// What the app sees of the checkout now, for the line to judge: its live
/// workers and their status, oldest status first. Those waiting for the
/// watch do not keep it busy; of those asking to land, the first lands.
pub fn look(now: i64, settings: &Settings, workers: &[(Uuid, Status)], by_hand: bool) -> crate::neutral::landing::Look {
    crate::neutral::landing::Look {
        now,
        idle: workers.iter().all(|(_, s)| *s == Status::Waiting),
        asks: workers.iter().find(|(_, s)| *s == Status::Landing).map(|(w, _)| *w),
        promotion_bound: settings.ms(Time::PromotionBound),
        cut_bound: settings.ms(Time::CutBound),
        by_hand,
    }
}

/// Why a move is not due, in the words the CLI answers.
pub fn why(not: NotDue) -> String {
    match not {
        NotDue::Held(why) => format!("landings are held: {why}"),
        NotDue::NoAsk => "no worker asks to land".into(),
        NotDue::NothingToPromote => "nothing to promote".into(),
        NotDue::Busy => "the checkout is busy and its oldest landing waits within the promotion bound".into(),
        NotDue::Red { tip, why } => format!("the slow gate is red on the proving tip {tip}: {why}"),
        NotDue::NothingToCut => "nothing promoted since the last cut".into(),
        NotDue::CutFailed { version, why } => format!("cutting {version} failed; a cut by hand tries again: {why}"),
        NotDue::Unsettled => {
            "the checkout is busy or unpromoted, and its oldest promotion waits within the cut bound".into()
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
