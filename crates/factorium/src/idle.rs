//! Idle work as the daemon runs it: a checkout's `[idle]` steps on its base
//! tip, in their own clone, as a heavy command that starts only while the
//! host is idle and the queue holds nothing else. It runs beside the ticks
//! through the heavy queue ([`Turn::Beside`]), which notes its sessions,
//! ends it when it is unqueued, and gives way for it when the host is no
//! longer idle — a worker at work, anything else queued — unrecorded, so the
//! next idle window runs it again. What is idle work's own is here: what is
//! due, its clone, the tasks a run's `tasks` line prints, filed as issues
//! before it is recorded, and the record. Tasks overdue by `tasks_bound` run
//! without an idle host, and only them: such a run yields to nothing.

use std::time::Duration;

use factorium_domain::admission;
use factorium_domain::checkout::Checkout;
use factorium_domain::heavy::{Heavy, queue_id};
use factorium_domain::idle::{self, Idles, RecordIdle, Run, host_idle, idle_id};
use factorium_domain::issue::Open;
use factorium_domain::settings::{Settings, Time};
use factorium_domain::task::{self, ToFile};
use factorium_domain::tracker;
use factorium_host::Priority;
use factum::Value;
use serde_json::json;

use crate::heavy::{Job, Turn};
use crate::{Answer, App, LineClone, fail};

/// The idle work running now, of one checkout: the host runs one at a time.
pub(crate) struct Running {
    checkout: String,
    tip: String,
    at: i64,
    /// Its steps, and their limit in all, s.
    steps: Vec<String>,
    limit: u64,
    /// Where its `tasks` line prints, if it has one.
    tasks: Option<std::path::PathBuf>,
}

impl App {
    /// The running idle work, if any: recorded once it ended; unrecorded if
    /// it was unqueued, or gave way as the host is no longer idle.
    pub(crate) async fn idle_check(&self) -> Answer<Option<Value>> {
        let workers: Vec<_> = self.live_workers().await?.into_iter().map(|w| w.state.status).collect();
        let busy = host_idle(&workers, &Default::default(), Ok(())).is_err();
        let Some(ended) = self.beside(busy, false).await? else { return Ok(None) };
        let Some(Running { checkout, tip, at, steps, limit, tasks }) = self.memory.lock().unwrap().idle.take() else {
            return Ok(None);
        };
        let rans = match ended {
            Ok(rans) => rans,
            Err(ends) => return Ok(Some(json!({ "idle": checkout, "tip": tip, "yielded": ends.to_string() }))),
        };
        let took_ms = rans.iter().map(|r| r.took_ms).sum();
        let (mut green, mut tail) = match rans.last() {
            None => (true, String::new()),
            Some(r) if r.ok => (true, r.tail.clone()),
            Some(r) if r.timed_out => (false, format!("past its limit of {limit}s:\n{}", r.tail)),
            Some(r) => (false, format!("step `{}` failed:\n{}", steps[rans.len() - 1], r.tail)),
        };
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

    /// Starts the idle work due longest of `checkouts`, if none runs: due
    /// `every` after the last run while the host is idle, or its tasks alone
    /// once overdue by `tasks_bound`, idle or not.
    pub(crate) async fn idle_start(&self, checkouts: &[Checkout], settings: &Settings) -> Answer<Option<Value>> {
        if self.memory.lock().unwrap().beside.is_some() {
            return Ok(None);
        }
        let q = self.state::<Heavy>(queue_id()).await?;
        let workers: Vec<_> = self.live_workers().await?.into_iter().map(|w| w.state.status).collect();
        let idle = host_idle(&workers, &q, admission::room(&self.reading(), settings)).is_ok();
        // Due by what the base declared as last fetched: no fetch while none is due.
        let mut due = Vec::new();
        for c in checkouts {
            let git = self.host.git(&c.path);
            let Some(work) = self.declared(&git, &c.fetched_base()).ok().and_then(|d| d.idle) else {
                continue;
            };
            let (state, every) = (self.state::<Idles>(idle_id(&c.name)).await?, work.every as i64 * 1000);
            let overdue = work.tasks.is_some() && state.due(every + settings.ms(Time::TasksBound), self.now());
            if overdue || (idle && state.due(every, self.now())) {
                due.push((state.last.map_or(i64::MIN, |r| r.at), c, overdue && !idle));
            }
        }
        let Some((_, c, overdue)) = due.into_iter().min_by_key(|(at, ..)| *at) else { return Ok(None) };
        let git = self.host.git(&c.path);
        let tip = git.fetch(&c.base).map_err(fail("git"))?;
        let Some(mut work) = self.declared(&git, &tip)?.idle else { return Ok(None) };
        let here = git.fresh(&self.data().clone_of(LineClone::Idle, &c.name), &tip).map_err(fail("git"))?;
        work.steps.retain(|_| !overdue); // overdue: the tasks alone
        let tasks = work.tasks.as_ref().map(|line| {
            let out = here.dir.with_extension("tasks");
            let _ = std::fs::remove_file(&out);
            work.steps.push(format!("( {line} ) > '{}'", out.display()));
            out
        });
        let (what, dir, steps, limit) = (idle::job(&c.name), &here.dir, &work.steps, Duration::from_secs(work.limit));
        let job =
            Job { what, worker: None, bound_at: None, dir, steps, env: &[], limit, priority: Priority::Background };
        let at = self.now();
        if self.heavy(job, Turn::Beside { yields: !overdue }).await?.is_none() {
            return Ok(None);
        }
        let (checkout, steps, limit) = (c.name.clone(), work.steps, work.limit);
        self.memory.lock().unwrap().idle = Some(Running { checkout, tip: tip.clone(), at, steps, limit, tasks });
        Ok(Some(json!({ "idle": c.name, "tip": tip, "started": true })))
    }
}
