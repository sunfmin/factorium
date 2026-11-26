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
//! `factorium idle` runs it all now, in its own process, waiting its turn:
//! it too yields to nothing, and is recorded and files its tasks the same way.

use factorium_domain::admission;
use factorium_domain::checkout::Checkout;
use factorium_domain::heavy::{Heavy, queue_id};
use factorium_domain::idle::{self, Idles, RecordIdle, Run, ToFile, host_idle, idle_id};
use factorium_domain::issue::Open;
use factorium_domain::settings::{Settings, Time};
use factorium_domain::tracker;
use factorium_host::Ran;
use factum::Value;
use serde_json::json;

use crate::heavy::{For, Job, Turn};
use crate::load::Load;
use crate::{Answer, App, LineDir, Refusal, fail};

/// The idle work running now, of one checkout: the host runs one at a time.
pub(crate) struct Running {
    checkout: String,
    tip: String,
    at: i64,
    /// Its work dir.
    dir: std::path::PathBuf,
    /// Its steps, and their limit in all, s.
    steps: Vec<String>,
    limit: u64,
    /// Where its `tasks` line prints, if it has one.
    tasks: Option<std::path::PathBuf>,
}

impl Running {
    /// Its work dir.
    pub(crate) fn dir(&self) -> &std::path::Path {
        &self.dir
    }
}

impl App {
    /// The running idle work, if any: recorded once it ended; unrecorded if
    /// it was unqueued, or gave way as the host is no longer idle.
    pub(crate) async fn idle_check(&self) -> Answer<Option<Value>> {
        let workers: Vec<_> = self.live_workers().await?.into_iter().map(|w| w.state.status).collect();
        let busy = host_idle(&workers, &Default::default(), Ok(())).is_err();
        let Some((For::Idle(idle), ended)) = self.beside(|f| matches!(f, For::Idle(_)), busy, false).await?.pop()
        else {
            return Ok(None);
        };
        match ended {
            Ok(rans) => self.idle_ran(*idle, rans).await.map(Some),
            Err(ends) => {
                let _ = self.work_dir_done(LineDir::Idle, &idle.checkout, &idle.dir);
                Ok(Some(json!({ "idle": idle.checkout, "tip": idle.tip, "yielded": ends.to_string() })))
            }
        }
    }

    /// Records `idle`'s run, whose steps ran so: the tasks it printed filed
    /// first, if it was green. Answers the record, with the issues filed.
    async fn idle_ran(&self, idle: Running, rans: Vec<Ran>) -> Answer<Value> {
        let Running { checkout, tip, at, steps, limit, tasks, dir } = idle;
        let removed = self.work_dir_done(LineDir::Idle, &checkout, &dir);
        let took_ms = rans.iter().map(|r| r.took_ms).sum();
        let (mut green, mut tail) = match rans.last().filter(|r| !r.ok) {
            None => (true, rans.last().map(|r| r.tail.clone()).unwrap_or_default()),
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
        let mut did = json!({ "idle": checkout, "tip": tip, "at": at, "green": green, "took_ms": took_ms, "tail": tail, "filed": filed });
        if let Err(e) = removed {
            did["work_dir"] = json!(e);
        }
        Ok(did)
    }

    /// Files as issues of `checkout` the tasks its run printed to `out` at
    /// `tip`; answers their numbers, or why what it printed is no tasks.
    async fn file_tasks(&self, checkout: &str, tip: &str, out: &std::path::Path) -> Answer<Result<Vec<u64>, String>> {
        let said = std::fs::read_to_string(out).unwrap_or_default();
        let mut issues = Vec::new();
        for row in self.rows(tracker::issues(Some(checkout), true)).await? {
            issues.push(self.issue(checkout, row.number).await?);
        }
        let git = self.host.git(&self.checkout(checkout).await?.path);
        let found = match idle::to_file(&said, tip, &issues, |sha| git.is_ancestor(sha, tip)) {
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

    /// Starts the idle work due longest of `checkouts` whose idle work is
    /// on here (`checkout edit --no-idle` turns it off), if none runs: due
    /// `every` after the last run while the host is idle, or its tasks alone
    /// once overdue by `tasks_bound`, idle or not.
    pub(crate) async fn idle_start(
        &self,
        checkouts: &[Checkout],
        load: &Load,
        settings: &Settings,
    ) -> Answer<Option<Value>> {
        // One at a time: `factorium idle`'s too, run in a process of its own.
        if self.beside_runs(|f| matches!(f, For::Idle(_))) || idle::queued(&load.queue).is_some() {
            return Ok(None);
        }
        let workers: Vec<_> = self.live_workers().await?.into_iter().map(|w| w.state.status).collect();
        let idle = host_idle(&workers, &load.queue, admission::room(&load.room(), settings)).is_ok();
        // Due by what the base declared as last fetched: no fetch while none is due.
        let mut due = Vec::new();
        for c in checkouts.iter().filter(|c| !c.idle_off) {
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
        let Some(idle) = self.idle_ready(c, overdue).await? else { return Ok(None) };
        let (checkout, tip, dir, steps) =
            (idle.checkout.clone(), idle.tip.clone(), idle.dir.clone(), idle.steps.clone());
        let job = Job::idle(idle::job(&checkout), &dir, &steps, idle.limit);
        let turn = Turn::Beside { key: None, yields: !overdue, of: For::Idle(Box::new(idle)) };
        if self.heavy(job, turn).await?.is_none() {
            let _ = self.work_dir_done(LineDir::Idle, &checkout, &dir);
            return Ok(None);
        }
        Ok(Some(json!({ "idle": checkout, "tip": tip, "started": true })))
    }

    /// `c`'s idle work at its base's tip, fetched now, in a work dir of its
    /// own: its steps, then its `tasks` line; `overdue`, the tasks alone.
    /// `None`: the tip declares no `[idle]`.
    async fn idle_ready(&self, c: &Checkout, overdue: bool) -> Answer<Option<Running>> {
        let git = self.host.git(&c.path);
        let tip = git.fetch(&c.base).map_err(fail("git"))?;
        let Some(mut work) = self.declared(&git, &tip)?.idle else { return Ok(None) };
        let short = &tip[..tip.len().min(7)];
        let about = format!("factorium's idle work of {} on its base {short}: its steps and tasks line.", c.name);
        let here = self.work_dir(c, LineDir::Idle, &c.name, &about, &tip)?;
        work.steps.retain(|_| !overdue); // overdue: the tasks alone
        let tasks = work.tasks.as_ref().map(|line| {
            let out = self.data().idle_tasks(&c.name);
            let _ = std::fs::create_dir_all(out.parent().unwrap_or(&out));
            let _ = std::fs::remove_file(&out);
            work.steps.push(format!("( {line} ) > '{}'", out.display()));
            out
        });
        let (checkout, steps, limit, at, dir) = (c.name.clone(), work.steps, work.limit, self.now(), here.dir);
        Ok(Some(Running { checkout, tip, at, dir, steps, limit, tasks }))
    }

    /// `factorium idle`: runs `checkout`'s idle work now, its steps then its
    /// tasks line, whether the host is idle or `every` has passed: through
    /// the heavy queue at background priority, yielding to nothing but the
    /// hold bound. Waits for it and answers its record, recorded as any
    /// run's. Refused while idle work runs or waits on the host, or when the
    /// base declares no `[idle]`.
    pub async fn idle_now(&self, checkout: &str) -> Answer {
        let c = self.checkout(checkout).await?;
        if let Some(e) = idle::queued(&self.state::<Heavy>(queue_id()).await?) {
            let why = format!(
                "`{}` is on the heavy queue (entry {}): the host runs one idle run at a time. \
                 Wait for it, or end it with `factorium unqueue {}`",
                e.ask.what, e.ask.id, e.ask.id
            );
            return Err(Refusal::new("idle_runs", why));
        }
        let Some(idle) = self.idle_ready(&c, false).await? else {
            let why = format!(
                "{} declares no `[idle]` in factorium.toml at the tip of {}: declare its `steps` or `tasks` there (SPEC §5)",
                c.name, c.base
            );
            return Err(Refusal::new("no_idle", why));
        };
        let job = Job::idle(idle::job(&c.name), &idle.dir, &idle.steps, idle.limit);
        let rans = self.heavy(job, Turn::Wait).await?.expect("a waited turn");
        self.idle_ran(idle, rans).await
    }
}
