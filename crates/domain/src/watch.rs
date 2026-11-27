//! The watch: one resident agent session. A look reads doctor's findings;
//! each finding is an episode while it stands, and an episode starts a
//! round — a brief, a bound, and the report the watch files — and, still
//! standing `round_again` after one ended, another, at most `rounds_max`:
//! then it is unresolved. One round runs at a time.

use std::collections::BTreeMap;

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{command, id, no};

pub fn watch_id() -> factum_core::Uuid {
    id("watch")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub what: String,
    pub fix: String,
    pub opened_at: i64,
    /// The round it started, if any.
    pub round: Option<u64>,
    /// What was said about it since it opened: comments on a waiting
    /// worker's issue.
    #[serde(default)]
    pub heard: Vec<String>,
    /// How many rounds it started.
    #[serde(default)]
    pub rounds: u32,
    /// How its last round ended, if one did.
    pub ended: Option<RoundEnd>,
}

/// How an episode's round ended: when (ms), and what the watch reported;
/// `None`: it ran past its bound unreported.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoundEnd {
    pub at: i64,
    pub report: Option<String>,
}

impl Watch {
    /// The episodes whose rounds are spent, none running: still standing,
    /// they are the owner's.
    pub fn unresolved(&self, rounds_max: u32) -> impl Iterator<Item = (&String, &Episode)> {
        let running = self.round.as_ref().map(|r| &r.episode);
        self.episodes.iter().filter(move |(k, e)| e.ended.is_some() && e.rounds >= rounds_max && running != Some(*k))
    }
}

impl Episode {
    /// It is due a round now: none started yet, or its last ended `again`
    /// ago (ms) with rounds left.
    fn due(&self, now: i64, again: i64, rounds_max: u32) -> bool {
        match &self.ended {
            _ if self.round.is_none() => true,
            Some(e) => now - e.at >= again && self.rounds < rounds_max,
            None => false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Round {
    pub n: u64,
    pub episode: String,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub round: u64,
    pub episode: String,
    pub text: String,
    /// What only the owner can do: spend money, mostly.
    pub human: Option<String>,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub terminal: Option<String>,
    pub harness: Option<String>,
    /// The account its session started on, if its harness takes one.
    pub account: Option<String>,
    /// The hash of the rules its session was briefed with ([`Started::rules`]);
    /// `None`: briefed before rules were recorded.
    pub rules: Option<String>,
    pub episodes: BTreeMap<String, Episode>,
    /// The round running now.
    pub round: Option<Round>,
    pub rounds: u64,
    /// The latest reports, newest last.
    pub reports: Vec<Report>,
    pub last_look: i64,
    /// Admission held since then, for this reason.
    pub held: Option<(i64, String)>,
    /// The report whose owner's part the owner has seen.
    pub seen: u64,
    /// Until when (ms) the owner has read the Decided records (`decided --read`).
    #[serde(default)]
    pub decided_read: i64,
    /// The terminals its sessions were started in, the latest `KEPT`: the
    /// only watch terminals this database's factorium calls its own.
    #[serde(default)]
    pub opened: Vec<String>,
    /// Terminals of its sessions whose close failed, with why: Orca
    /// refused, or lists them still.
    #[serde(default)]
    pub unclosed: Vec<(String, String)>,
}

/// How many reports the watch keeps in its state.
const KEPT: usize = 50;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WatchEvent {
    WatchStarted {
        terminal: String,
        harness: String,
        account: Option<String>,
        rules: Option<String>,
    },
    /// Written before the outbox (#47); read and ignored.
    BriefHanded,
    WatchEnded,
    /// Closing its session's terminal failed: why.
    WatchCloseFailed {
        terminal: String,
        why: String,
    },
    Looked {
        at: i64,
    },
    EpisodeOpened {
        key: String,
        what: String,
        fix: String,
        at: i64,
        #[serde(default)]
        heard: Vec<String>,
    },
    /// What was said about an open episode, all of it.
    EpisodeHeard {
        key: String,
        heard: Vec<String>,
    },
    EpisodeClosed {
        key: String,
    },
    RoundStarted {
        n: u64,
        episode: String,
        at: i64,
    },
    RoundEnded {
        n: u64,
        why: String,
    },
    /// Written before the outbox (#47), which now records what reached
    /// each watch session; read and ignored.
    RoundSent {
        n: u64,
        terminal: String,
    },
    Reported(Report),
    AdmissionHeld {
        why: String,
        at: i64,
    },
    AdmissionOpen,
    HumanSeen {
        round: u64,
    },
    /// The owner read every Decided record made until `at` (ms).
    DecidedRead {
        at: i64,
    },
}

pub struct Watches;

impl Aggregate for Watches {
    const NAME: &'static str = "watch";
    type State = Watch;
    type Event = WatchEvent;

    fn evolve(s: &mut Watch, event: WatchEvent) {
        match event {
            WatchEvent::WatchStarted { terminal, harness, account, rules } => {
                s.opened.push(terminal.clone());
                let extra = s.opened.len().saturating_sub(KEPT);
                s.opened.drain(..extra);
                (s.terminal, s.harness, s.account, s.rules) = (Some(terminal), Some(harness), account, rules)
            }
            WatchEvent::BriefHanded | WatchEvent::RoundSent { .. } => {}
            WatchEvent::WatchEnded => (s.terminal, s.harness, s.account, s.rules) = (None, None, None, None),
            WatchEvent::WatchCloseFailed { terminal, why } => {
                s.unclosed.retain(|(t, _)| *t != terminal);
                s.unclosed.push((terminal, why));
                let extra = s.unclosed.len().saturating_sub(KEPT);
                s.unclosed.drain(..extra);
            }
            WatchEvent::Looked { at } => s.last_look = at,
            WatchEvent::EpisodeOpened { key, what, fix, at, heard } => {
                s.episodes.insert(key, Episode { what, fix, opened_at: at, heard, ..Episode::default() });
            }
            WatchEvent::EpisodeHeard { key, heard } => {
                if let Some(e) = s.episodes.get_mut(&key) {
                    e.heard = heard;
                }
            }
            WatchEvent::EpisodeClosed { key } => _ = s.episodes.remove(&key),
            WatchEvent::RoundStarted { n, episode, at } => {
                if let Some(e) = s.episodes.get_mut(&episode) {
                    (e.round, e.rounds) = (Some(n), e.rounds + 1);
                }
                s.rounds = n;
                s.round = Some(Round { n, episode, at });
            }
            WatchEvent::RoundEnded { .. } => {
                if let Some(r) = s.round.take()
                    && let Some(e) = s.episodes.get_mut(&r.episode)
                {
                    e.ended = Some(RoundEnd { at: s.last_look, report: None });
                }
            }
            WatchEvent::AdmissionHeld { why, at } => s.held = Some((at, why)),
            WatchEvent::AdmissionOpen => s.held = None,
            WatchEvent::HumanSeen { round } => s.seen = round,
            WatchEvent::DecidedRead { at } => s.decided_read = at,
            WatchEvent::Reported(r) => {
                s.round = None;
                if let Some(e) = s.episodes.get_mut(&r.episode) {
                    e.ended = Some(RoundEnd { at: r.at, report: Some(r.text.clone()) });
                }
                s.reports.push(r);
                let extra = s.reports.len().saturating_sub(KEPT);
                s.reports.drain(..extra);
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Started {
    pub terminal: String,
    pub harness: String,
    pub account: Option<String>,
    /// The hash of the rules its brief was written from: a binary whose
    /// rules hash otherwise restarts it between rounds.
    pub rules: Option<String>,
}

command!(StartWatch, "watch.start", Watches, Started, |_p| watch_id(), |s, p, now| {
    Ok(vec![WatchEvent::WatchStarted {
        terminal: p.terminal.clone(),
        harness: p.harness.clone(),
        account: p.account.clone(),
        rules: p.rules.clone(),
    }])
});

/// How closing the watch's session's terminal went.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ended {
    /// Why its terminal is not closed; `None`: Orca no longer lists it.
    pub failed: Option<String>,
}

// The watch's session was closed: the next look starts a fresh one.
command!(EndWatch, "watch.end", Watches, Ended, |_p| watch_id(), |s, p, _now| {
    let Some(terminal) = &s.terminal else { return Ok(Vec::new()) };
    let mut events = vec![WatchEvent::WatchEnded];
    if let Some(why) = &p.failed {
        events.push(WatchEvent::WatchCloseFailed { terminal: terminal.clone(), why: why.clone() });
    }
    Ok(events)
});

/// One finding, as a look hands it on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub key: String,
    pub what: String,
    pub fix: String,
    /// What was said about it: comments on a waiting worker's issue.
    #[serde(default)]
    pub heard: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Look {
    pub found: Vec<Found>,
    /// The round bound, ms.
    pub bound: i64,
    /// An episode still standing this long (ms) after its round ended is due another.
    #[serde(default)]
    pub again: i64,
    /// Rounds an episode starts at most.
    #[serde(default)]
    pub rounds_max: u32,
    /// Whether the watch's session can take a round now: it runs, and its
    /// turn is closed. A line typed before it is up goes to the shell.
    pub ready: bool,
}

// A look: episodes open for new findings and close for gone ones; a round
// past its bound ends; and, with none running, the oldest episode due a
// round starts one, once the watch's session can take it: one never
// rounded first, then one still standing `again` after its round ended.
command!(LookNow, "watch.look", Watches, Look, |_p| watch_id(), |s, p, now| {
    let mut ev = vec![WatchEvent::Looked { at: now.0 }];
    for f in &p.found {
        match s.episodes.get(&f.key) {
            None => ev.push(WatchEvent::EpisodeOpened {
                key: f.key.clone(),
                what: f.what.clone(),
                fix: f.fix.clone(),
                at: now.0,
                heard: f.heard.clone(),
            }),
            Some(e) if e.heard != f.heard => {
                ev.push(WatchEvent::EpisodeHeard { key: f.key.clone(), heard: f.heard.clone() })
            }
            Some(_) => {}
        }
    }
    let gone: Vec<&String> = s.episodes.keys().filter(|k| !p.found.iter().any(|f| &f.key == *k)).collect();
    for k in &gone {
        ev.push(WatchEvent::EpisodeClosed { key: (*k).clone() });
    }
    let timed_out = s.round.as_ref().filter(|r| now.0 - r.at > p.bound);
    if let Some(r) = timed_out {
        ev.push(WatchEvent::RoundEnded { n: r.n, why: "past its bound".into() });
    }
    if (s.round.is_none() || timed_out.is_some()) && p.ready {
        let ended = timed_out.map(|r| &r.episode);
        let next = (s.episodes.iter())
            .filter(|(k, e)| e.due(now.0, p.again, p.rounds_max) && !gone.contains(k) && ended != Some(*k))
            .min_by_key(|(_, e)| (e.round.is_some(), e.ended.as_ref().map_or(e.opened_at, |d| d.at)))
            .map(|(k, _)| k.clone())
            .or_else(|| p.found.iter().find(|f| !s.episodes.contains_key(&f.key)).map(|f| f.key.clone()));
        if let Some(episode) = next {
            ev.push(WatchEvent::RoundStarted { n: s.rounds + 1, episode, at: now.0 });
        }
    }
    Ok(ev)
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Filed {
    pub text: String,
    pub human: Option<String>,
}

command!(FileReport, "watch.report", Watches, Filed, |_p| watch_id(), |s, p, now| {
    let Some(r) = &s.round else { return no("no_round", "no round is running") };
    if p.text.trim().is_empty() {
        return no("no_text", "a report says what was found and done");
    }
    Ok(vec![WatchEvent::Reported(Report {
        round: r.n,
        episode: r.episode.clone(),
        text: p.text.clone(),
        human: p.human.clone().filter(|h| !h.trim().is_empty()),
        at: now.0,
    })])
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Held {
    /// Why admission holds; `None`: it admits.
    pub why: Option<String>,
}

// Admission's state: held for one reason stays held since it began.
command!(NoteAdmission, "watch.admission", Watches, Held, |_p| watch_id(), |s, p, now| {
    let kind = |w: &str| w.split(':').next().unwrap_or(w).to_string();
    Ok(match (&p.why, &s.held) {
        (Some(w), Some((_, h))) if kind(w) == kind(h) => Vec::new(),
        (Some(w), _) => vec![WatchEvent::AdmissionHeld { why: w.clone(), at: now.0 }],
        (None, Some(_)) => vec![WatchEvent::AdmissionOpen],
        (None, None) => Vec::new(),
    })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Seen {
    pub round: u64,
}

command!(SeeHuman, "watch.seen", Watches, Seen, |_p| watch_id(), |s, p, now| {
    crate::only(p.round > s.seen, WatchEvent::HumanSeen { round: p.round })
});

/// The owner reads every Decided record made until now.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadDecided {}

command!(MarkDecidedRead, "watch.decided_read", Watches, ReadDecided, |_p| watch_id(), |_s, _p, now| Ok(vec![
    WatchEvent::DecidedRead { at: now.0 }
]));
