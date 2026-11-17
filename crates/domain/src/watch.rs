//! The watch: one resident agent session. A look reads doctor's findings;
//! each finding is an episode while it stands, and an episode starts at
//! most one round — a brief, a bound, and the report the watch files. One
//! round runs at a time.

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
}

/// How many reports the watch keeps in its state.
const KEPT: usize = 50;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WatchEvent {
    WatchStarted { terminal: String, harness: String },
    Looked { at: i64 },
    EpisodeOpened { key: String, what: String, fix: String, at: i64 },
    EpisodeClosed { key: String },
    RoundStarted { n: u64, episode: String, at: i64 },
    RoundEnded { n: u64, why: String },
    Reported(Report),
    AdmissionHeld { why: String, at: i64 },
    AdmissionOpen,
    HumanSeen { round: u64 },
}

pub struct Watches;

impl Aggregate for Watches {
    const NAME: &'static str = "watch";
    type State = Watch;
    type Event = WatchEvent;

    fn evolve(s: &mut Watch, event: WatchEvent) {
        match event {
            WatchEvent::WatchStarted { terminal, harness } => (s.terminal, s.harness) = (Some(terminal), Some(harness)),
            WatchEvent::Looked { at } => s.last_look = at,
            WatchEvent::EpisodeOpened { key, what, fix, at } => {
                s.episodes.insert(key, Episode { what, fix, opened_at: at, round: None });
            }
            WatchEvent::EpisodeClosed { key } => _ = s.episodes.remove(&key),
            WatchEvent::RoundStarted { n, episode, at } => {
                if let Some(e) = s.episodes.get_mut(&episode) {
                    e.round = Some(n);
                }
                s.rounds = n;
                s.round = Some(Round { n, episode, at });
            }
            WatchEvent::RoundEnded { .. } => s.round = None,
            WatchEvent::AdmissionHeld { why, at } => s.held = Some((at, why)),
            WatchEvent::AdmissionOpen => s.held = None,
            WatchEvent::HumanSeen { round } => s.seen = round,
            WatchEvent::Reported(r) => {
                s.round = None;
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
}

command!(StartWatch, "watch.start", Watches, Started, |_p| watch_id(), |s, p, now| {
    Ok::<_, factum_core::Rejection>(vec![WatchEvent::WatchStarted {
        terminal: p.terminal.clone(),
        harness: p.harness.clone(),
    }])
});

/// One finding, as a look hands it on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub key: String,
    pub what: String,
    pub fix: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Look {
    pub found: Vec<Found>,
    /// The round bound, ms.
    pub bound: i64,
}

command!(
    /// A look: episodes open for new findings and close for gone ones; a round
    /// past its bound ends; and, with none running, the oldest episode without
    /// a round starts one.
    LookNow,
    "watch.look",
    Watches,
    Look,
    |_p| watch_id(),
    |s, p, now| {
        let mut ev = vec![WatchEvent::Looked { at: now.0 }];
        for f in &p.found {
            if !s.episodes.contains_key(&f.key) {
                ev.push(WatchEvent::EpisodeOpened {
                    key: f.key.clone(),
                    what: f.what.clone(),
                    fix: f.fix.clone(),
                    at: now.0,
                });
            }
        }
        let gone: Vec<&String> = s.episodes.keys().filter(|k| !p.found.iter().any(|f| &f.key == *k)).collect();
        for k in &gone {
            ev.push(WatchEvent::EpisodeClosed { key: (*k).clone() });
        }
        let mut running = s.round.clone();
        if let Some(r) = &running
            && now.0 - r.at > p.bound
        {
            ev.push(WatchEvent::RoundEnded { n: r.n, why: "past its bound".into() });
            running = None;
        }
        if running.is_none() {
            let mut fresh: Vec<(i64, &String)> = s
                .episodes
                .iter()
                .filter(|(k, e)| e.round.is_none() && !gone.contains(k))
                .map(|(k, e)| (e.opened_at, k))
                .collect();
            fresh.sort();
            let next = fresh
                .first()
                .map(|(_, k)| (*k).clone())
                .or_else(|| p.found.iter().find(|f| !s.episodes.contains_key(&f.key)).map(|f| f.key.clone()));
            if let Some(episode) = next {
                ev.push(WatchEvent::RoundStarted { n: s.rounds + 1, episode, at: now.0 });
            }
        }
        Ok::<_, factum_core::Rejection>(ev)
    }
);

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

command!(
    /// Admission's state: held for one reason stays held since it began.
    NoteAdmission,
    "watch.admission",
    Watches,
    Held,
    |_p| watch_id(),
    |s, p, now| {
        let kind = |w: &str| w.split(':').next().unwrap_or(w).to_string();
        Ok(match (&p.why, &s.held) {
            (Some(w), Some((_, h))) if kind(w) == kind(h) => Vec::new(),
            (Some(w), _) => vec![WatchEvent::AdmissionHeld { why: w.clone(), at: now.0 }],
            (None, Some(_)) => vec![WatchEvent::AdmissionOpen],
            (None, None) => Vec::new(),
        })
    }
);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Seen {
    pub round: u64,
}

command!(SeeHuman, "watch.seen", Watches, Seen, |_p| watch_id(), |s, p, now| Ok(if p.round > s.seen {
    vec![WatchEvent::HumanSeen { round: p.round }]
} else {
    Vec::new()
}));
