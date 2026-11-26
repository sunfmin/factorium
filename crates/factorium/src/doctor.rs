//! Doctor: what it reads from the database and the host, for the owner's
//! `doctor` and the watch's look alike, and the owner's view it answers.

use factorium_domain::idle::{Idles, idle_id};
use factorium_domain::issue::{self, IssueEvent, Issues};
use factorium_domain::kit::{Kits, kit_id};
use factorium_domain::line::Lines;
use factorium_domain::line_id;
use factorium_domain::stack::{Stacks, stack_id};
use factorium_domain::{daemon, decided, doctor, heavy, next, watch, worker};
use factorium_domain::{issue_id, tracker};
use factum::{Aggregate, By, StreamId, Value};
use serde_json::json;

use crate::{Answer, App, Caller};

impl App {
    /// Where the landing of `worker` (as doctor names it) is in `checkout`'s stack, if it is:
    /// its depth, what it merged onto, its merge, and its gate's state, as the queue holds it.
    async fn stacked(&self, checkout: &str, worker: &str, queue: &heavy::Queue) -> Answer<Value> {
        let stack = self.state::<Stacks>(stack_id(checkout)).await?;
        let Some(at) = stack.layers.iter().position(|l| l.worker.to_string() == worker) else { return Ok(Value::Null) };
        let layer = &stack.layers[at];
        let entry = queue.entries.iter().find(|e| e.ask.lands == Some(layer.worker));
        let gate = match (&layer.green, &layer.red, entry) {
            (Some(_), _, _) if at == 0 => "green",
            (Some(_), _, _) => "green, waits for the gates below it",
            (_, Some(_), _) => "red, told once the gates below it are green",
            (None, None, Some(e)) if e.started_at.is_some() => "its gate runs",
            (None, None, _) => "its gate waits its turn in the heavy queue",
        };
        Ok(json!({ "depth": at + 1, "on": layer.on, "merge": layer.merged, "gate": gate }))
    }

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
                let waits = self.blockers(&on).await?.into_iter().filter(|w| w.met.is_none());
                parked = waits.map(|w| doctor::Unmet { on: w.on, why: w.why }).collect();
            }
            let limited = wk.waits_on_limit().map(|l| l.until);
            let worker = w.worker.to_string();
            seen.waiting.push(doctor::WaitingSeen { worker, issue, why, heard, parked, limited });
        }
        for w in self.rows(tracker::workers(Some(worker::Status::Landing), false)).await? {
            let (worker, since) = (w.worker.to_string(), w.state.since);
            let urgent = self.issue(&w.checkout, w.number).await?.urgent;
            seen.landing.push(doctor::LandingSeen { worker, checkout: w.checkout, number: w.number, since, urgent });
        }
        // In the order they land: the urgent first, as the line takes them.
        factorium_domain::issue::urgent_first(&mut seen.landing, |l| l.urgent);
        for c in self.rows(tracker::listed()).await? {
            let line = self.state::<Lines>(line_id(&c.name)).await?;
            // What the base declares, as last fetched: doctor fetches nothing.
            let git = self.host.git(&c.path);
            let cuts = self.declared(&git, &c.fetched_base()).is_ok_and(|d| d.cut.is_some());
            let stuck = crate::line::stuck_base(&c, &git);
            let idle = self.state::<Idles>(idle_id(&c.name)).await?;
            let idle_off = c.idle_off;
            let mutants = match line.awaited {
                Some(f) if self.issue(&c.name, f.number).await?.status != issue::Status::Closed => {
                    Some((f.number, f.at + self.settings().await?.ms(factorium_domain::settings::Time::MutantsBound)))
                }
                _ => None,
            };
            seen.lines.push(doctor::LineSeen { checkout: c.name, line, cuts, stuck, idle, idle_off, mutants });
        }
        seen.ready = self.ready_seen().await?;
        seen.stablehand = self.stablehand_seen().await?;
        seen.unmoved = self.unmoved(&self.state::<Kits>(kit_id()).await?);
        seen.queue = self.state::<heavy::Heavy>(heavy::queue_id()).await?;
        // Admission's reading: what the heavy queue granted left out, as the tick reads it.
        let load = self.load().await?;
        seen.reading = load.admission();
        let gone = self.gone_holders(&seen.queue, |pid| load.alive(pid)).await?;
        seen.holders_gone = gone.into_iter().map(|(id, why)| (id, why.into())).collect();
        let w = self.state::<watch::Watches>(watch::watch_id()).await?;
        seen.held = w.held.clone().map(|(since, why)| doctor::HeldSeen { since, why });
        seen.daemon = self.state::<daemon::Daemons>(daemon::daemon_id()).await?;
        seen.human = w.reports.iter().filter(|r| r.round > w.seen).filter_map(|r| r.human.clone()).collect();
        (seen.orphans, seen.unclosed) = self.orphans(&w).await?;
        seen.orphaned = self.orphaned().await?;
        seen.kept_dirs = self.work_dirs_kept();
        let dead = self.engine.dead_letters(factorium_domain::tracker::TRACKER).await?;
        if !dead.is_empty() {
            let projection = factorium_domain::tracker::TRACKER.into();
            seen.dead_letters.push(doctor::DeadLetters { projection, count: dead.len() });
        }
        Ok(seen)
    }

    /// Each ready issue no worker holds, of a listed checkout, as the
    /// daemon's next start reads it (claims are read, not cleared): whether
    /// it waits for a blocker or a claim, and why the kit cannot start it.
    async fn ready_seen(&self) -> Answer<Vec<doctor::ReadySeen>> {
        let mut ready = Vec::new();
        for r in self.rows(tracker::untaken()).await? {
            if self.checkout(&r.checkout).await.is_err() {
                continue;
            }
            let i = self.issue(&r.checkout, r.number).await?;
            let waits = i.claim.is_some() || !self.blockers(&i).await?.iter().all(|w| w.met.is_some());
            // As the daemon's next start does, any pick refused holds it.
            let kit = match waits {
                true => None,
                false => self.choice(&r.checkout, &i.on).await.err().map(|e| e.message),
            };
            ready.push(doctor::ReadySeen { issue: format!("{}#{}", r.checkout, r.number), waits, kit });
        }
        Ok(ready)
    }

    /// Each checkout at its cap (`workers_per_checkout`) that holds ready
    /// issues: its live workers, the cap, and those issues (none urgent, none
    /// waiting for a blocker, its time or a claim), in the order they start.
    async fn capped(&self, ready: &[doctor::ReadySeen], cap: u64) -> Answer<Vec<Value>> {
        let workers = self.live_workers().await?;
        let live = next::live(workers.iter().map(|w| w.checkout.as_str()));
        let mut held = std::collections::BTreeMap::<String, Vec<String>>::new();
        for r in next::order(self.rows(tracker::untaken()).await?, std::iter::empty()) {
            let issue = format!("{}#{}", r.checkout, r.number);
            let waits = ready.iter().find(|s| s.issue == issue).is_none_or(|s| s.waits);
            if !waits && next::capped(&live, &r.checkout, r.urgent, cap) {
                held.entry(r.checkout).or_default().push(issue);
            }
        }
        let at = |c: &str| live.get(c).copied().unwrap_or(0);
        Ok(held
            .into_iter()
            .map(|(c, waiting)| json!({ "checkout": c, "live": at(&c), "cap": cap, "waiting": waiting }))
            .collect())
    }

    /// Every issue that waits for a person, oldest wait first: its title,
    /// the question its body asks, why, when it started waiting and who left
    /// it so.
    async fn asks(&self) -> Answer<Vec<Value>> {
        let mut asks = Vec::new();
        for r in self.rows(tracker::asking()).await? {
            let i = self.issue(&r.checkout, r.number).await?;
            let stream = StreamId::new(Issues::NAME, issue_id(&r.checkout, r.number));
            let mut history = Vec::new();
            for e in self.engine.history(&stream).await? {
                let by_watch = e.meta.by == By::Principal(Caller::Watch.by().into());
                let event: IssueEvent = tracker::decode(&e).map_err(|m| crate::Refusal::new("bad_event", m))?;
                history.push((event, e.meta.now.0, by_watch));
            }
            let (since, by) = issue::asked(history).unzip();
            asks.push(json!({
                "issue": format!("{}#{}", i.checkout, i.number),
                "title": i.title,
                "question": issue::question(&i.body),
                "why": i.reason,
                "since": since,
                "utc": since.map(factorium_domain::when::utc),
                "by": by,
            }));
        }
        asks.sort_by_key(|a| a["since"].as_i64());
        Ok(asks)
    }

    /// Every Decided record made after `since` (ms), in any issue's body or
    /// comment of any checkout, newest first, each with its number (`id`):
    /// read from the issue, its one home. Any record without a number takes
    /// one first ([`App::number_decided`]).
    pub(crate) async fn decided(&self, since: i64) -> Answer<Vec<Value>> {
        self.number_decided().await?;
        let mut found = Vec::new();
        for f in self.records().await?.into_iter().filter(|f| f.at.is_some_and(|at| at > since)) {
            let at = f.at.unwrap_or_default();
            let mut v = json!(f.record);
            v["id"] = json!(f.id.map(decided::name));
            v["issue"] = json!(format!("{}#{}", f.checkout, f.number));
            (v["at"], v["utc"]) = (json!(at), json!(factorium_domain::when::utc(at)));
            found.push(v);
        }
        // Newest first; of one time, the later number first.
        found.sort_by_key(|d| std::cmp::Reverse((d["at"].as_i64(), d["id"].as_str().and_then(decided::parse))));
        Ok(found)
    }

    /// `decided`: the Decided records of the last 7 days, newest first; with
    /// `read`, marks every one made until now read, so doctor lists none of
    /// them; with `id` (`D17`, or `17`), that one record, whatever its age
    /// and whether it was read.
    pub async fn decided_view(&self, read: bool, id: Option<&str>) -> Answer {
        if let Some(name) = id {
            let Some(n) = decided::parse(name) else {
                return Err(crate::Refusal::new(
                    "bad_id",
                    format!("`{name}`: name a Decided record as D<n>, e.g. D17"),
                ));
            };
            let all = self.decided(i64::MIN).await?;
            let one = all.into_iter().find(|d| d["id"] == json!(decided::name(n)));
            return one
                .ok_or_else(|| crate::Refusal::new("no_record", format!("no Decided record is {}", decided::name(n))));
        }
        if read {
            self.submit::<watch::MarkDecidedRead>(watch::ReadDecided {}).await?;
            let w = self.state::<watch::Watches>(watch::watch_id()).await?;
            return Ok(json!({ "read_until": w.decided_read, "utc": factorium_domain::when::utc(w.decided_read) }));
        }
        const WEEK: i64 = 7 * 24 * 60 * 60 * 1000;
        Ok(json!({ "decided": self.decided(self.now() - WEEK).await? }))
    }

    /// The findings, each with its fix; the reading; and the owner's part of
    /// the watch's reports, which this marks seen.
    pub(crate) async fn doctor_view(&self) -> Answer {
        let seen = self.seen().await?;
        let settings = self.settings().await?;
        let findings = doctor::findings(&seen, &settings, self.now());
        let w = self.state::<watch::Watches>(watch::watch_id()).await?;
        // Standing past its rounds: the watch gave up on it; the owner's now.
        let max = settings.get(factorium_domain::settings::Setting::RoundsMax) as u32;
        let unresolved: Vec<Value> = (w.unresolved(max))
            .map(|(key, e)| {
                let last = e.ended.as_ref().and_then(|d| d.report.clone());
                json!({ "key": key, "what": e.what, "rounds": e.rounds, "last_report": last })
            })
            .collect();
        if let Some(last) = w.reports.last() {
            self.submit::<watch::SeeHuman>(watch::Seen { round: last.round }).await?;
        }
        let unstarted = doctor::unstarted(&seen, &settings, self.now());
        let holds: Vec<_> =
            seen.lines.iter().filter_map(|l| Some((l.checkout.clone(), l.line.held.clone()?))).collect();
        let parked: Vec<Value> = (seen.waiting.iter().filter(|w| !w.parked.is_empty()))
            .map(|w| json!({ "worker": w.worker, "issue": w.issue, "why": w.why, "waits_for": w.parked }))
            .collect();
        // Ready issues no worker holds, not to start before their time.
        let mut timed = Vec::new();
        for r in self.rows(tracker::untaken()).await? {
            let i = self.issue(&r.checkout, r.number).await?;
            if let Some(at) = i.not_before.filter(|at| *at > self.now()) {
                let issue = format!("{}#{}", r.checkout, r.number);
                timed.push(json!({ "issue": issue, "until": at, "utc": factorium_domain::when::utc(at) }));
            }
        }
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
        let mut landing = Vec::new();
        for l in &seen.landing {
            let line = seen.landing.iter().filter(|o| o.checkout == l.checkout);
            let place = line.clone().position(|o| o.worker == l.worker).map_or(0, |i| i + 1);
            let issue = format!("{}#{}", l.checkout, l.number);
            let stacked = self.stacked(&l.checkout, &l.worker, &seen.queue).await?;
            landing.push(json!({ "worker": l.worker, "issue": issue, "place": place, "of": line.count(), "urgent": l.urgent, "stack": stacked }));
        }
        let idle: Vec<Value> =
            seen.lines.iter().map(|l| json!({ "checkout": l.checkout, "last": l.idle.last })).collect();
        let now = self.now();
        let mutants: Vec<Value> = (seen.lines.iter())
            .filter_map(|l| {
                let (number, until) = l.mutants.filter(|(_, until)| now < *until)?;
                let (issue, utc) = (format!("{}#{number}", l.checkout), factorium_domain::when::utc(until));
                Some(json!({ "checkout": l.checkout, "issue": issue, "until": until, "utc": utc, "left_s": (until - now) / 1000 }))
            })
            .collect();
        let promoted: Vec<Value> = (seen.lines.iter().filter(|l| l.line.promoted.is_some()))
            .map(|l| json!({ "checkout": l.checkout, "tip": l.line.promoted, "gated": l.line.gated }))
            .collect();
        let mut view = json!({
            "heavy": seen.queue.entries,
            // Each resource a heavy entry holds or waits for: its holder (since, until it
            // declared) and those waiting for it, in the order they start: each one's declared hold, its
            // window, and why it is not next.
            "resources": seen.queue.resources(self.now(), settings.order()),
            // Each checkout's last idle run that ended green or red.
            "idle": idle,
            // Each checkout's last promotion: its tip, and how its gate ran ("slow steps only, tip
            // gated green by <issue>'s landing", or "fast gate and slow steps").
            "promoted": promoted,
            "findings": findings,
            // Findings the watch's rounds did not end, each with its last report.
            "unresolved": unresolved,
            "human": seen.human,
            // Issues waiting for a person, oldest first: answer each with
            // `issue ready` (do it), `issue close` (don't), or `issue comment` then `ready`.
            "asks": self.asks().await?,
            // What deciders decided for the owner since `decided --read`, newest first.
            "decided": self.decided(w.decided_read).await?,
            "holds": holds,
            // Promotions waiting for their mutants issue (where slimfit holds them at the base): the issue, and
            // until when (ms, UTC, seconds left), `mutants_bound` after its filing; past it, they go on without it.
            "mutants": mutants,
            // Each landing waiting, with its place in its checkout's line (the urgent first, then each
            // as it asked), and where it is in its checkout's stack: its depth (1 merged onto the proving
            // tip), the commit it merged onto, its merge, and its gate's state; none while it waits to be
            // stacked (docs/adr/0019-stacked-landing-gates.md). Landings go on while its checkout's
            // promotion or cut runs (docs/adr/0018-a-checkout-s-moves-run-apart.md).
            "landing": landing,
            // Waiting workers the watch parked behind what their issues wait for, not yet met.
            "parked": parked,
            // Each checkout at its cap of live workers (`workers_per_checkout`, those in landing among them),
            // with the ready issues waiting for one of them to end; an urgent one is not held.
            "capped": self.capped(&seen.ready, settings.get(factorium_domain::settings::Setting::WorkersPerCheckout)).await?,
            // Ready issues parked until their time (ms, and in UTC): none starts before it.
            "not_before": timed,
            // Waiting workers parked until their engine's limit resets, until when (ms).
            "limited": limited,
            "reading": seen.reading,
            // The daemon's last tick past half the hang bound: when, how long, its longest step, and the
            // quick ticks in a row since (a finding until five).
            "slow_tick": seen.daemon.slow,
            // Working workers waiting on a long command of their own, as they declared, until when (ms).
            "waiting": declared,
            "watch": { "terminal": w.terminal, "round": w.round, "episodes": w.episodes.len() },
        });
        // Ready issues wait and admission starts none: each reason it holds now. Absent while one could start.
        if !unstarted.is_empty() {
            view["unstarted"] = json!(unstarted);
        }
        Ok(view)
    }
}
