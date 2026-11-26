//! The daemon's tick: one pass over everything that moves without a
//! command — landings, promotions, cuts, issues to bump a pin of another
//! checkout, workers watched, one start if admission allows, idle work, and
//! the watch's look. The host and the heavy queue are read once a pass
//! ([`crate::load`]), after the lines' gates. No step waits for a heavy run:
//! those run beside the ticks. Each tick is timed by its steps, and one past
//! half the hang bound is kept for doctor, naming the step it spent most in.
//! The process that ticks, one per database, is [`process`].

mod process;

use factorium_domain::admission;
use factorium_domain::bump;
use factorium_domain::checkout::Checkout;
use factorium_domain::daemon::{self, NoteTick, Step, Steps, Ticked};
use factorium_domain::issue::{Author, urgent_first};
use factorium_domain::settings::Time;
use factorium_domain::tracker::{self, IssueRow, WorkerRow};
use factorium_domain::watch::{Held, NoteAdmission};
use factorium_domain::worker::{Of, Status as WorkerStatus, Unpark, Workers};
use factum::Value;
use serde::Serialize;
use serde_json::json;

use crate::{Answer, App, LineClone, Refusal};

pub use process::handed_lock;

/// What one tick did.
#[derive(Debug, Default, Serialize)]
pub struct Tick {
    pub seen: Vec<Value>,
    pub landed: Vec<Value>,
    /// Issues closed whose worker landed: the close a dead daemon left undone.
    pub closed: Vec<Value>,
    /// Issues closed whose worker finished without a landing (`worker done
    /// --no-landing`), with its evidence.
    pub finished: Vec<Value>,
    pub lines: Vec<Value>,
    /// Idle work that ended, yielded or started.
    pub idle: Vec<Value>,
    pub started: Option<Value>,
    /// Parked workers sent back to work: what their issues waited for is met.
    pub resumed: Vec<Value>,
    /// Workers told their role changed under them.
    pub roles: Vec<Value>,
    /// Issues filed to bump a pin of another checkout behind its base tip.
    pub bumped: Vec<Value>,
    /// Why nothing started, when something was ready.
    pub held: Option<String>,
    pub watch: Option<Value>,
    /// The binary to run in place of this one once the tick ends: Homebrew
    /// links another version of the daemon's own formula, whoever cut it.
    pub restart: Option<String>,
    /// The upgrade of the daemon's own formula from the tap, started or ended.
    pub upgrade: Option<Value>,
    /// How long the tick took, ms on the app's clock.
    pub took_ms: i64,
    /// The step it spent most in.
    pub longest: Step,
    /// Past half the hang bound.
    pub slow: bool,
}

impl Tick {
    /// Its line in daemon.log, if daemon.log has it: it was slow, or did something (landed, moved
    /// a line, saw, started, resumed or told a worker its role changed, or moved idle work).
    pub fn line(&self) -> Option<String> {
        self.logged().then(|| serde_json::to_string(self).unwrap_or_default())
    }

    fn logged(&self) -> bool {
        self.slow
            || self.started.is_some()
            || !self.landed.is_empty()
            || !self.lines.is_empty()
            || !self.seen.is_empty()
            || !self.idle.is_empty()
            || !self.resumed.is_empty()
            || !self.roles.is_empty()
            || self.upgrade.is_some()
    }
}

impl App {
    async fn listed(&self) -> Answer<Vec<Checkout>> {
        self.rows(tracker::listed()).await
    }

    pub(crate) async fn live_workers(&self) -> Answer<Vec<WorkerRow>> {
        self.rows(tracker::workers(None, false)).await
    }

    /// One pass of the daemon, recorded: a failing one adds to the run of
    /// failures doctor reads, and the watch still looks.
    pub async fn tick(&self) -> Answer<Tick> {
        let start = self.now();
        let mut steps = Steps::at(start);
        let mut done = self.pass(&mut steps).await;
        let error = done.as_ref().err().map(Refusal::to_string);
        self.proving(error.as_deref()).await;
        steps.ended("proving the binary", self.now());
        let took_ms = self.now() - start;
        let every = self.settings().await.unwrap_or_default().ms(Time::TickEvery);
        let slow = daemon::slow(took_ms, every).then(|| (took_ms, steps.longest.clone()));
        if let Ok(t) = &mut done {
            (t.took_ms, t.longest, t.slow) = (took_ms, steps.longest, slow.is_some());
        }
        let noted = self.submit::<NoteTick>(Ticked { error, slow }).await;
        if done.is_err() || noted.is_err() {
            let settings = self.settings().await.unwrap_or_default();
            if let Err(e) = self.look_if_due(&settings).await {
                eprintln!("look: {e}");
                self.look_failed(&e);
            }
        }
        noted.and(done)
    }

    async fn pass(&self, steps: &mut Steps) -> Answer<Tick> {
        let mut t = Tick::default();
        self.engine.settle().await?;
        let settings = self.settings().await?;
        steps.ended("settling the store", self.now());
        // Idle work that ended, or yields: before the lines, so it never runs ahead of them.
        t.idle.extend(self.idle_check().await?);
        t.closed = self.close_landed().await?;
        t.finished = self.finish_unlanded().await?;
        // Before the lines and the starts: what an urgent issue halts, and what it no longer does.
        let halt = self.note_halt().await?;
        steps.ended("idle work's check and closing landed and finished issues", self.now());
        // The lines, as each says what is due.
        let checkouts = self.listed().await?;
        (t.landed, t.lines) = self.asking_once(self.lines(&checkouts)).await?;
        steps.ended("the lines' moves", self.now());
        t.bumped = self.bumps(&checkouts).await?;
        steps.ended("bumps", self.now());
        // The host and the heavy queue, read once, after the lines' gates
        // ran: the workers' shares, admission and idle work all ask it.
        let load = self.load().await?;
        // A resource whose holder's waiter died is given back, on record.
        self.give_back(&load.queue, |pid| load.alive(pid)).await?;
        steps.ended("reading the host", self.now());
        // Workers: their sessions, and their share of the host.
        let hold = settings.ms(Time::HoldBound);
        let live = self.live_workers().await?;
        let workers: Vec<_> = live.iter().map(|w| w.worker).collect();
        self.retry_workers(&workers).await?;
        t.seen = self.workers_health(&workers, |w| load.worker(w, self.now(), hold), &settings).await?;
        steps.ended("watching workers", self.now());
        t.roles = self.tell_role_changes().await?;
        steps.ended("telling workers their role changed", self.now());
        // One start, if admission allows.
        if let Some(IssueRow { checkout, number, urgent, .. }) = self.next_issue().await? {
            let reading = load.admission();
            // Urgent, only the memory and disk floors hold it; what else held it, it went past.
            let held = match urgent {
                true => admission::admit_urgent(&reading, &settings),
                false if !halt.is_empty() => Err(format!("halt: behind urgent {}", halt.join(", "))),
                false => admission::admit(&reading, &load.queue, self.now(), &settings),
            };
            // What it goes ahead of, read while it is still among the ready ones.
            let ahead_of = match (urgent, &held) {
                (true, Ok(())) => self.ahead_of(&checkout, number).await?,
                _ => Vec::new(),
            };
            let held = match held {
                Ok(()) => match self.start_worker(&checkout, number).await {
                    Ok(mut started) => {
                        if urgent {
                            let past = admission::holds(&reading, &load.queue, self.now(), &settings);
                            started["urgent"] = json!({ "ahead_of": ahead_of, "past": past });
                        }
                        t.started = Some(started);
                        // An urgent start halts the rest from now, not from the next tick.
                        self.note_halt().await?;
                        None
                    }
                    Err(r) if r.code == "no_harness" => Some(format!("kit: {}", r.message)),
                    // Claimed since it was picked: it starts on a later tick.
                    Err(r) if r.code == "claimed" => Some(format!("{checkout}#{number}: {}", r.message)),
                    Err(r) => return Err(r),
                },
                Err(why) => Some(why),
            };
            self.submit::<NoteAdmission>(Held { why: held.clone() }).await?;
            t.held = held;
        } else {
            self.submit::<NoteAdmission>(Held { why: None }).await?;
        }
        steps.ended("starting a worker", self.now());
        // Idle work, once the lines and the workers have what they need.
        t.idle.extend(self.idle_start(&checkouts, &load, &settings).await?);
        if admission::disk_short(&load.room(), &settings) {
            self.reclaim();
        }
        steps.ended("idle work's start", self.now());
        t.resumed = self.resume_parked().await?;
        steps.ended("resuming parked workers", self.now());
        t.watch = self.look_if_due(&settings).await?;
        steps.ended("the watch's look", self.now());
        t.upgrade = self.upgrade_from_tap(&checkouts, &settings).await?;
        steps.ended("upgrading from the tap", self.now());
        t.restart = self.swap_due(false).map(|b| b.to_string_lossy().into_owned());
        self.note_refusal().await?;
        Ok(t)
    }

    /// The linked binary to exec once proven; not while a message is typed
    /// beside the ticks (the next daemon would type it again), nor, after a
    /// good tick, while a line move runs beside them, which a restart would
    /// end. After a `failed` one it would wait for ever: only a good tick
    /// finds the move ended, so the restart ends it, and the newer binary
    /// moves the line again.
    fn swap_due(&self, failed: bool) -> Option<std::path::PathBuf> {
        let held = (!failed && self.moves_run()) || self.typing(|_| true);
        (!held).then(|| self.swapped(false)).flatten()
    }

    /// The binary to run in place of this one after a tick: a good tick's
    /// restart. After a failed one, the linked binary once proven, since an
    /// older daemon whose store cannot read an event a newer binary wrote
    /// is mended by the swap alone (factorium#252); else the kept one a
    /// rollback goes back to.
    pub fn after(&self, ticked: &Answer<Tick>) -> Option<std::path::PathBuf> {
        match ticked {
            Ok(t) => t.restart.as_ref().map(Into::into),
            Err(_) => self.swap_due(true).or_else(|| self.fallback()),
        }
    }

    /// One issue to bump, in each checkout whose proving tip pins another
    /// checkout by git rev behind that one's base tip, while none is open
    /// (factorium#127). Read from refs as last fetched: no fetch of its own.
    async fn bumps(&self, checkouts: &[Checkout]) -> Answer<Vec<Value>> {
        let mut filed = Vec::new();
        for c in checkouts {
            let proving = c.fetched_proving();
            let pins = self.host.git(&c.path).shown(&proving, "Cargo.toml").map(|t| bump::pins(&t)).unwrap_or_default();
            if pins.is_empty() {
                continue;
            }
            let open: Vec<String> =
                self.rows(tracker::issues(Some(&c.name), false)).await?.into_iter().map(|r| r.title).collect();
            for d in checkouts.iter().filter(|d| d.name != c.name && !bump::open(&d.name, &open)) {
                let git = self.host.git(&d.path);
                let Some(pin) = git.origin().ok().and_then(|u| pins.iter().find(|p| bump::same_repo(&p.url, &u)))
                else {
                    continue;
                };
                let Ok(tip) = git.sha(&d.fetched_base()) else { continue };
                let landings = git.landings(&pin.rev, &tip).unwrap_or_default();
                if landings.is_empty() || !git.is_ancestor(&pin.rev, &tip) {
                    continue;
                }
                let (title, body) = bump::issue(&d.name, &pin.rev, &tip, &landings);
                filed.push(self.issue_add(&c.name, &title, &body, false, &[]).await?);
            }
        }
        Ok(filed)
    }

    /// The ready issues no worker holds and nothing open blocks, ahead of
    /// which the urgent `checkout`#`number` started: those not urgent before
    /// it in the ordinary order.
    async fn ahead_of(&self, checkout: &str, number: u64) -> Answer<Vec<String>> {
        let mut ahead = Vec::new();
        for r in self.rows(tracker::untaken()).await? {
            if (r.checkout.as_str(), r.number) == (checkout, number) {
                break;
            }
            let issue = self.issue(&r.checkout, r.number).await?;
            if !r.urgent && self.blockers(&issue).await?.iter().all(|w| w.met.is_some()) {
                ahead.push(format!("{}#{}", r.checkout, r.number));
            }
        }
        Ok(ahead)
    }

    /// The first ready issue no worker holds and nothing open blocks, the
    /// urgent ones first, then those that start ahead (a promotion's mutants
    /// issue), then in the ordinary order ([`tracker::untaken`]);
    /// whose account, if it names one, can start now, and whose role has
    /// what it needs of this host (each role checked once a tick).
    async fn next_issue(&self) -> Answer<Option<IssueRow>> {
        let mut checked = std::collections::BTreeSet::new();
        // Roles found lacking this tick: one that passes starts its issue at once.
        let mut lacking = std::collections::BTreeSet::new();
        let mut held = None;
        let mut rows = self.rows(tracker::untaken()).await?;
        // Those that start ahead (a promotion's mutants issue), then the urgent before all.
        urgent_first(&mut rows, |r| r.ahead);
        urgent_first(&mut rows, |r| r.urgent);
        for r in rows {
            let (checkout, number) = (r.checkout.clone(), r.number);
            if !checked.contains(&checkout) && self.checkout(&checkout).await.is_err() {
                continue;
            }
            checked.insert(checkout.clone());
            let issue = self.issue(&checkout, number).await?;
            // Claimed, a command acts on its worktrees: it starts once that
            // is done. A claim whose process is gone is cleared.
            self.clear_dead_claim(&issue).await?;
            if self.issue(&checkout, number).await?.claim.is_some() {
                continue;
            }
            if !self.blockers(&issue).await?.iter().all(|w| w.met.is_some()) {
                continue;
            }
            // One that cannot start now (its account used up, or what its
            // checkout is pinned to at its limit) waits behind those that
            // can; with none, its start answers why it is held.
            if self.choice(&checkout, &issue.on).await.is_err() {
                held.get_or_insert(r);
                continue;
            }
            // Its role lacking what it needs of this host, it waits; doctor
            // hands the watch what it lacks.
            let role = self.checkout(&checkout).await?.role_for(issue.on.role.as_deref()).map(|(r, _)| r.to_string());
            if let Some(role) = role
                && (lacking.contains(&role) || !self.role_ready(&role).await?)
            {
                lacking.insert(role);
                continue;
            }
            return Ok(Some(r));
        }
        Ok(held)
    }

    /// Each worker the watch parked behind what its issue waits for, idle
    /// (waiting, or in a wait it declared), sent back to work once all of
    /// it is met, told what was met and how, and to merge the proving tip;
    /// and each parked on its engine's limit, once it reset. Unparked
    /// first, so a restart that does not reach it leaves it to the watch.
    async fn resume_parked(&self) -> Answer<Vec<Value>> {
        let mut resumed = Vec::new();
        for w in self.rows(tracker::workers(None, false)).await? {
            let wk = self.state::<Workers>(w.worker).await?;
            let idle = match wk.status {
                WorkerStatus::Waiting => true,
                WorkerStatus::Working => wk.declared.is_some(),
                _ => false,
            };
            if !idle {
                continue;
            }
            let text = match wk.waits_on_limit() {
                // Its engine's limit reset: what was held for it is typed first.
                Some(limit) if limit.until <= self.now() => {
                    "Your engine's limit reset. Pick up your issue where you left it.".to_string()
                }
                Some(_) => continue,
                None if !wk.parked => continue,
                None => {
                    let met: Option<Vec<String>> = (self.blockers(&self.issue(&w.checkout, w.number).await?).await?)
                        .into_iter()
                        .map(|w| Some(format!("{} ({})", w.on, w.met?)))
                        .collect();
                    let Some(met) = met.filter(|m| !m.is_empty()) else { continue };
                    let p = self.checkout(&w.checkout).await?.proving;
                    format!(
                        "What your issue waited for is met: {}. Merge the proving tip into your branch \
                         (`git fetch origin {p} && git merge origin/{p}`) before you gate again.",
                        met.join(", ")
                    )
                }
            };
            self.submit::<Unpark>(Of { worker: w.worker }).await?;
            let did = self.restart(w.worker, Author::Factorium, &text, false).await;
            resumed.push(did.unwrap_or_else(|r| json!({ "worker": w.worker, "refused": r.to_string() })));
        }
        Ok(resumed)
    }

    /// Frees disk: the build outputs of the line's worktrees, rebuilt when
    /// next needed.
    fn reclaim(&self) {
        for kind in LineClone::ALL {
            let Ok(dirs) = std::fs::read_dir(self.data().clones(kind)) else { continue };
            for d in dirs.flatten() {
                let _ = std::fs::remove_dir_all(d.path().join("target"));
            }
        }
    }
}
