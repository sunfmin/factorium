//! Idle work as the daemon runs it: a checkout's `[idle]` steps on its base
//! tip, in their own clone, as a heavy command that starts only while the
//! host is idle and the queue holds nothing else. It runs beside the ticks,
//! never inside one: a tick that finds it ended records it; a tick that
//! finds the host no longer idle — a worker at work, anything else queued —
//! ends it, unrecorded, so the next idle window runs it again. The tasks
//! a run's `tasks` line prints are filed as issues before it is recorded.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use factorium_domain::admission;
use factorium_domain::checkout::Checkout;
use factorium_domain::heavy::{End, EndHeavy, Heavy, queue_id};
use factorium_domain::idle::{self, Idles, RecordIdle, Run, host_idle, idle_id};
use factorium_domain::issue::Open;
use factorium_domain::settings::Settings;
use factorium_domain::task::{self, ToFile};
use factorium_domain::tracker;
use factorium_host::Priority;
use factum::{Clock, Uuid, Value};
use serde_json::json;

use crate::{Answer, App, LineClone, fail};

/// The idle work running now, of one checkout: the host runs one at a time.
pub(crate) struct Running {
    checkout: String,
    tip: String,
    /// Its heavy-queue entry.
    id: Uuid,
    at: i64,
    /// Where its `tasks` line prints, if it has one.
    tasks: Option<std::path::PathBuf>,
    stop: Arc<AtomicBool>,
    run: std::thread::JoinHandle<Ended>,
}

/// How a run's steps ended.
struct Ended {
    green: bool,
    /// Ended because the host was no longer idle.
    yielded: bool,
    tail: String,
    took_ms: u64,
}

/// Runs `steps` in `dir` one after another until one fails, all of them
/// within `limit` on `clock`, or until `stop` is set. The limit is the
/// app's clock, not the time a background step is given the CPU: a busy
/// host slows the step, and a scenario moves the clock past it.
fn steps(
    dir: std::path::PathBuf,
    steps: Vec<String>,
    limit: Duration,
    clock: Arc<dyn Clock>,
    stop: Arc<AtomicBool>,
) -> Ended {
    let started = Instant::now();
    let deadline = clock.now().0.saturating_add(limit.as_millis() as i64);
    let past = || clock.now().0 >= deadline;
    let mut tail = String::new();
    for step in &steps {
        let end = || stop.load(Ordering::Relaxed) || past();
        let ran = factorium_host::run(&dir, step, &[], Duration::MAX, Priority::Background, &end);
        if !ran.ok {
            let timed_out = past();
            let why = if timed_out {
                format!("past its limit of {}s", limit.as_secs())
            } else {
                format!("step `{step}` failed")
            };
            let took_ms = started.elapsed().as_millis() as u64;
            let yielded = stop.load(Ordering::Relaxed) && !timed_out;
            return Ended { green: false, yielded, tail: format!("{why}:\n{}", ran.tail), took_ms };
        }
        tail = ran.tail;
    }
    Ended { green: true, yielded: false, tail, took_ms: started.elapsed().as_millis() as u64 }
}

impl App {
    /// The running idle work, if any: recorded once it ended; ended if the
    /// host is no longer idle or its queue entry was taken off.
    pub(crate) async fn idle_check(&self) -> Answer<Option<Value>> {
        let Some(r) = self.memory.lock().unwrap().idle.take() else { return Ok(None) };
        let q = self.state::<Heavy>(queue_id()).await?;
        let workers: Vec<_> = self.live_workers().await?.into_iter().map(|w| w.state.status).collect();
        let others = q.entries.iter().any(|e| e.id != r.id) || !q.entries.iter().any(|e| e.id == r.id);
        let busy = others || host_idle(&workers, &Default::default(), Ok(())).is_err();
        if !r.run.is_finished() && !busy {
            self.memory.lock().unwrap().idle = Some(r);
            return Ok(None);
        }
        let Running { checkout, tip, id, at, tasks, stop, run } = r;
        stop.store(busy, Ordering::Relaxed);
        let ended = tokio::task::spawn_blocking(move || run.join()).await.ok().and_then(Result::ok).unwrap_or(Ended {
            green: false,
            yielded: false,
            tail: "its run panicked".into(),
            took_ms: 0,
        });
        self.submit::<EndHeavy>(End { id }).await?;
        if ended.yielded {
            return Ok(Some(json!({ "idle": checkout, "tip": tip, "yielded": "the host is no longer idle" })));
        }
        let Ended { mut green, mut tail, took_ms, .. } = ended;
        let mut filed = Vec::new();
        if let (true, Some(out)) = (green, tasks) {
            match self.file_tasks(&checkout, &tip, &out).await? {
                Ok(numbers) => filed = numbers,
                Err(why) => (green, tail) = (false, why),
            }
        }
        let run = Run { tip: tip.clone(), at, green, took_ms, tail: tail.clone() };
        self.submit::<RecordIdle>(idle::Ran { checkout: checkout.clone(), run }).await?;
        Ok(Some(json!({ "idle": checkout, "tip": tip, "green": green, "tail": tail, "filed": filed })))
    }

    /// Files as issues of `checkout` the tasks its run printed to `out` at
    /// `tip`; answers their numbers, or why what it printed is no tasks.
    async fn file_tasks(&self, checkout: &str, tip: &str, out: &std::path::Path) -> Answer<Result<Vec<u64>, String>> {
        let said = std::fs::read_to_string(out).unwrap_or_default();
        let mut issues = Vec::new();
        for row in self.rows(tracker::issues(Some(checkout), true)).await? {
            issues.push(self.issue(checkout, row.number).await?);
        }
        let found = match task::to_file(&said, tip, &issues, self.now()) {
            Ok(found) => found,
            Err(why) => return Ok(Err(why)),
        };
        let mut numbers = Vec::new();
        for ToFile { title, body, task } in found {
            let open = Open { checkout: checkout.into(), title, body, task: Some(task), ..Open::default() };
            numbers.push(self.open_issue(open).await?);
        }
        Ok(Ok(numbers))
    }

    /// Starts the idle work due longest of `checkouts`, if the host is idle
    /// and none runs.
    pub(crate) async fn idle_start(&self, checkouts: &[Checkout], settings: &Settings) -> Answer<Option<Value>> {
        if self.memory.lock().unwrap().idle.is_some() {
            return Ok(None);
        }
        let q = self.state::<Heavy>(queue_id()).await?;
        let workers: Vec<_> = self.live_workers().await?.into_iter().map(|w| w.state.status).collect();
        if host_idle(&workers, &q, admission::room(&self.reading(), settings)).is_err() {
            return Ok(None);
        }
        // Due by what the base declared as last fetched: no fetch while none is due.
        let mut due = Vec::new();
        for c in checkouts {
            let git = self.host.git(&c.path);
            let Some(every) = self.declared(&git, &c.fetched_base()).ok().and_then(|d| d.idle) else {
                continue;
            };
            let state = self.state::<Idles>(idle_id(&c.name)).await?;
            if state.due(every.every as i64 * 1000, self.now()) {
                due.push((state.last.map_or(i64::MIN, |r| r.at), c));
            }
        }
        let Some((_, c)) = due.into_iter().min_by_key(|(at, _)| *at) else { return Ok(None) };
        let git = self.host.git(&c.path);
        let tip = git.fetch(&c.base).map_err(fail("git"))?;
        let Some(mut work) = self.declared(&git, &tip)?.idle else { return Ok(None) };
        let here = git.fresh(&self.data().clone_of(LineClone::Idle, &c.name), &tip).map_err(fail("git"))?;
        let tasks = work.tasks.as_ref().map(|line| {
            let out = here.dir.with_extension("tasks");
            let _ = std::fs::remove_file(&out);
            work.steps.push(format!("( {line} ) > '{}'", out.display()));
            out
        });
        let id = Uuid::now_v7();
        if !self.start_once(id, idle::job(&c.name), settings).await? {
            return Ok(None);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (dir, flag, limit, clock) =
            (here.dir.clone(), stop.clone(), Duration::from_secs(work.limit), self.clock.clone());
        let run = std::thread::spawn(move || steps(dir, work.steps, limit, clock, flag));
        let (checkout, at) = (c.name.clone(), self.now());
        self.memory.lock().unwrap().idle = Some(Running { checkout, tip: tip.clone(), id, at, tasks, stop, run });
        Ok(Some(json!({ "idle": c.name, "tip": tip, "started": true })))
    }

    /// Ends the running idle work, unrecorded: the daemon is about to stop
    /// or be replaced, and what it started must not outlive it.
    pub(crate) async fn idle_end(&self) {
        let Some(r) = self.memory.lock().unwrap().idle.take() else { return };
        r.stop.store(true, Ordering::Relaxed);
        let _ = tokio::task::spawn_blocking(move || r.run.join()).await;
        let _ = self.submit::<EndHeavy>(End { id: r.id }).await;
    }
}
