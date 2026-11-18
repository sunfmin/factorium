//! A proving line: what landed on the proving branch, what was promoted to
//! the base branch, and the versions cut from it; and what is due on it now,
//! and why not ([`Line::due`]). The caller does the git and gate work, and
//! records what it did as [`LineEvent`]s.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// Landings held, and why.
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

impl Line {
    /// The line once `event` happened.
    pub fn apply(&mut self, event: LineEvent) {
        let s = self;
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

/// What is due: the move, for a landing the one it lands, and for a
/// promotion the proving tip it was judged on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Due {
    Land(Uuid),
    Promote(String),
    Cut,
}

/// Why a move is not due now. The caller says it in its own words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotDue {
    /// Landings are held, and why.
    Held(String),
    /// Nothing asks to land.
    NoAsk,
    NothingToPromote,
    /// Not idle, and the oldest landing waits within the promotion bound less the lead.
    Busy,
    /// The slow gate is red on this proving tip, and why.
    Red {
        tip: String,
        why: String,
    },
    NothingToCut,
    /// Cutting this version failed, and why; a cut by hand tries again.
    CutFailed {
        version: String,
        why: String,
    },
    /// Not idle or not all promoted, and the oldest promotion waits within
    /// the cut bound less the lead.
    Unsettled,
}

/// What the caller sees of the line's repo now, for the line to judge.
pub struct Look {
    pub now: i64,
    /// Nothing works on the repo or lands on it.
    pub idle: bool,
    /// Of those asking to land, the one that asked first.
    pub asks: Option<Uuid>,
    pub bounds: Bounds,
    /// Asked for by hand: the bounds and a red tip or failed cut do not hold it.
    pub by_hand: bool,
}

/// How long what the line owes waits at most, ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    /// The oldest unpromoted landing.
    pub promotion: i64,
    /// The oldest uncut promotion.
    pub cut: i64,
    /// How long before its bound a move owed falls due however busy the
    /// repo is: the longest a running heavy command may hold the slot its
    /// gate or cut waits for, so it runs within the bound.
    pub lead: i64,
}

/// What the line owes, a promotion or a cut: since when, its bound, and
/// when it falls due. A move owed is due once it has waited its bound less
/// the lead, and overdue past its bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owed {
    pub since: i64,
    /// The bound, ms.
    pub bound: i64,
    /// When it falls due however busy the repo is: the bound less the lead,
    /// never before `since`.
    pub due_at: i64,
    pub now: i64,
}

impl Owed {
    /// It has waited its bound less the lead: due however busy the repo is.
    pub fn due(&self) -> bool {
        self.now >= self.due_at
    }

    /// It has waited past its bound.
    pub fn overdue(&self) -> bool {
        self.now - self.since > self.bound
    }
}

impl Line {
    /// What the line owes of `mv` at `now`: a promotion of its oldest
    /// unpromoted landing, a cut of its oldest uncut promotion. A landing
    /// is asked for, not owed.
    pub fn owed(&self, mv: Move, now: i64, bounds: &Bounds) -> Option<Owed> {
        let (since, bound) = match mv {
            Move::Land => return None,
            Move::Promote => (self.unpromoted.first()?.at, bounds.promotion),
            Move::Cut => (*self.uncut.first()?, bounds.cut),
        };
        let due_at = since + (bound - bounds.lead).max(0);
        Some(Owed { since, bound, due_at, now })
    }

    /// Whether `mv` is due now, or why not, in one call. `tip` reads the
    /// proving tip: it is called only for a promotion due on everything
    /// else, which is then judged on that tip, and its error is passed up
    /// as it is.
    pub fn due<E>(
        &self,
        mv: Move,
        look: &Look,
        tip: impl FnOnce() -> Result<String, E>,
    ) -> Result<Result<Due, NotDue>, E> {
        match mv {
            Move::Land => Ok(self.landing(look)),
            Move::Promote => match self.promotable(look) {
                Ok(()) => Ok(self.on_tip(look, tip()?)),
                Err(why) => Ok(Err(why)),
            },
            Move::Cut => Ok(self.cut(look)),
        }
    }

    fn landing(&self, look: &Look) -> Result<Due, NotDue> {
        if let Some(why) = &self.held {
            return Err(NotDue::Held(why.clone()));
        }
        // One landing at a time: the one that asked first.
        look.asks.map(Due::Land).ok_or(NotDue::NoAsk)
    }

    /// Whether a promotion is due on everything but the proving tip.
    fn promotable(&self, look: &Look) -> Result<(), NotDue> {
        let Some(owed) = self.owed(Move::Promote, look.now, &look.bounds) else {
            return Err(NotDue::NothingToPromote);
        };
        if !(look.by_hand || look.idle || owed.due()) {
            return Err(NotDue::Busy);
        }
        Ok(())
    }

    /// A promotion onto `tip`: not while it is the tip the slow gate found red.
    fn on_tip(&self, look: &Look, tip: String) -> Result<Due, NotDue> {
        match &self.red {
            Some((red, why)) if *red == tip && !look.by_hand => Err(NotDue::Red { tip, why: why.clone() }),
            _ => Ok(Due::Promote(tip)),
        }
    }

    fn cut(&self, look: &Look) -> Result<Due, NotDue> {
        let Some(owed) = self.owed(Move::Cut, look.now, &look.bounds) else {
            return Err(NotDue::NothingToCut);
        };
        if let Some((version, why)) = self.cut_failed.as_ref().filter(|_| !look.by_hand) {
            return Err(NotDue::CutFailed { version: version.clone(), why: why.clone() });
        }
        let settled = look.idle && self.unpromoted.is_empty();
        if !(look.by_hand || settled || owed.due()) {
            return Err(NotDue::Unsettled);
        }
        Ok(Due::Cut)
    }
}
