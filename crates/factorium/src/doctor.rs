//! Doctor: what it reads from the database and the host, for the owner's
//! `doctor` and the watch's look alike, and the owner's view it answers.

use factorium_domain::idle::{Idles, idle_id};
use factorium_domain::line::Lines;
use factorium_domain::line_id;
use factorium_domain::tracker;
use factorium_domain::{daemon, doctor, heavy, watch, worker};
use factum::Value;
use serde_json::json;

use crate::{Answer, App};

impl App {
    /// What doctor reads, from the database and a reading.
    pub(crate) async fn seen(&self) -> Answer<doctor::Seen> {
        let mut seen = doctor::Seen::default();
        for w in self.rows(tracker::workers(Some(worker::Status::Waiting), false)).await? {
            let issue = format!("{}#{}", w.checkout, w.number);
            let wk = self.state::<worker::Workers>(w.worker).await?;
            let on = self.issue(&w.checkout, w.number).await?;
            let heard = on.heard_since(w.state.since).map(|n| n.text.clone()).collect();
            let why = w.state.why.unwrap_or_default();
            let mut parked = Vec::new();
            if wk.parked {
                let blockers = self.blockers(&on).await?;
                parked = blockers.into_iter().filter(|(_, met)| met.is_none()).map(|(name, _)| name).collect();
            }
            let limited = wk.waits_on_limit().map(|l| l.until);
            let worker = w.worker.to_string();
            seen.waiting.push(doctor::WaitingSeen { worker, issue, why, heard, parked, limited });
        }
        for w in self.rows(tracker::workers(Some(worker::Status::Landing), false)).await? {
            let (worker, since) = (w.worker.to_string(), w.state.since);
            seen.landing.push(doctor::LandingSeen { worker, checkout: w.checkout, number: w.number, since });
        }
        for c in self.rows(tracker::listed()).await? {
            let line = self.state::<Lines>(line_id(&c.name)).await?;
            // What the base declares, as last fetched: doctor fetches nothing.
            let git = self.host.git(&c.path);
            let cuts = self.declared(&git, &c.fetched_base()).is_ok_and(|d| d.cut.is_some());
            let stuck = crate::line::stuck_base(&c, &git);
            let idle = self.state::<Idles>(idle_id(&c.name)).await?;
            seen.lines.push(doctor::LineSeen { checkout: c.name, line, cuts, stuck, idle });
        }
        seen.queue = self.state::<heavy::Heavy>(heavy::queue_id()).await?;
        let w = self.state::<watch::Watches>(watch::watch_id()).await?;
        seen.held = w.held.clone().map(|(since, why)| doctor::HeldSeen { since, why });
        seen.daemon = self.state::<daemon::Daemons>(daemon::daemon_id()).await?;
        seen.human = w.reports.iter().filter(|r| r.round > w.seen).filter_map(|r| r.human.clone()).collect();
        (seen.orphans, seen.unclosed) = self.orphans(&w).await?;
        seen.orphaned = self.orphaned().await?;
        let dead = self.engine.dead_letters(factorium_domain::tracker::TRACKER).await?;
        if !dead.is_empty() {
            let projection = factorium_domain::tracker::TRACKER.into();
            seen.dead_letters.push(doctor::DeadLetters { projection, count: dead.len() });
        }
        Ok(seen)
    }

    /// The findings, each with its fix; the reading; and the owner's part of
    /// the watch's reports, which this marks seen.
    pub(crate) async fn doctor_view(&self) -> Answer {
        let seen = self.seen().await?;
        let settings = self.settings().await?;
        let findings = doctor::findings(&seen, &settings, self.now());
        let w = self.state::<watch::Watches>(watch::watch_id()).await?;
        if let Some(last) = w.reports.last() {
            self.submit::<watch::SeeHuman>(watch::Seen { round: last.round }).await?;
        }
        // Admission's reading: what the heavy queue granted left out, as the tick reads it.
        let reading = self.load().await?.admission();
        let holds: Vec<_> =
            seen.lines.iter().filter_map(|l| Some((l.checkout.clone(), l.line.held.clone()?))).collect();
        let skipped: Vec<Value> =
            self.accounts_here().skipped.into_iter().map(|s| json!({ "secret": s.secret, "why": s.why })).collect();
        let parked: Vec<Value> = (seen.waiting.iter().filter(|w| !w.parked.is_empty()))
            .map(|w| json!({ "worker": w.worker, "issue": w.issue, "why": w.why, "waits_for": w.parked }))
            .collect();
        let limited: Vec<Value> = (seen.waiting.iter())
            .filter_map(|w| Some(json!({ "worker": w.worker, "issue": w.issue, "why": w.why, "until": w.limited? })))
            .collect();
        let mut declared = Vec::new();
        for w in self.rows(tracker::workers(Some(worker::Status::Working), false)).await? {
            let wk = self.state::<worker::Workers>(w.worker).await?;
            if let Some(d) = wk.declared.filter(|d| d.until >= self.now()) {
                let issue = format!("{}#{}", w.checkout, w.number);
                declared.push(json!({ "worker": w.worker, "issue": issue, "why": d.why, "until": d.until }));
            }
        }
        let landing: Vec<Value> = (seen.landing.iter())
            .map(|l| {
                let line = seen.landing.iter().filter(|o| o.checkout == l.checkout);
                let place = line.clone().position(|o| o.worker == l.worker).map_or(0, |i| i + 1);
                let issue = format!("{}#{}", l.checkout, l.number);
                json!({ "worker": l.worker, "issue": issue, "place": place, "of": line.count() })
            })
            .collect();
        let idle: Vec<Value> =
            seen.lines.iter().map(|l| json!({ "checkout": l.checkout, "last": l.idle.last })).collect();
        Ok(json!({
            // The engine accounts no session can start on here, and why.
            "accounts_skipped": skipped,
            "heavy": seen.queue.entries,
            // Each checkout's last idle run that ended green or red.
            "idle": idle,
            "findings": findings,
            "human": seen.human,
            "holds": holds,
            // Each landing waiting, with its place in its checkout's line (1 lands next).
            "landing": landing,
            // Waiting workers the watch parked behind what their issues wait for, not yet met.
            "parked": parked,
            // Waiting workers parked until their engine's limit resets, until when (ms).
            "limited": limited,
            "reading": reading,
            // The daemon's last tick past half the hang bound: when, how long, its longest step.
            "slow_tick": seen.daemon.slow,
            // Working workers waiting on a long command of their own, as they declared, until when (ms).
            "waiting": declared,
            "watch": { "terminal": w.terminal, "round": w.round, "episodes": w.episodes.len() },
        }))
    }
}
