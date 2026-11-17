//! The owner's commands: the checkout list, the tracker, the kit, the
//! accounts, the settings, holds, and doctor.

use std::path::Path;

use factorium_domain::checkout::{Add, AddCheckout, Checkouts, Named, RemoveCheckout};
use factorium_domain::issue::*;
use factorium_domain::kit::*;
use factorium_domain::line::{Hold, HoldLandings, Lines};
use factorium_domain::settings::{HostSettings, Set, SetSetting, Setting};
use factorium_domain::tracker::{self, Tracker, shown};
use factorium_domain::{daemon, doctor, gate, heavy, line, watch, worker};
use factorium_domain::{issue_id, line_id};
use factorium_harness::Harness;
use factum::{Model, Uuid, Value};
use serde_json::json;

use crate::{Answer, App, Refusal};

pub(crate) fn model() -> Model {
    Model::new()
        .aggregate::<Checkouts>()
        .command::<AddCheckout>()
        .command::<RemoveCheckout>()
        .aggregate::<Issues>()
        .command::<OpenIssue>()
        .command::<EditIssue>()
        .command::<BlockIssue>()
        .command::<UnblockIssue>()
        .command::<ParkIssue>()
        .command::<ReadyIssue>()
        .command::<CloseIssue>()
        .command::<TakeIssue>()
        .command::<ReleaseIssue>()
        .command::<CommentIssue>()
        .aggregate::<worker::Workers>()
        .command::<worker::StartWorker>()
        .command::<worker::AskLanding>()
        .command::<worker::RedLanding>()
        .command::<worker::ToldRed>()
        .command::<worker::GreenLanding>()
        .command::<worker::Wait>()
        .command::<worker::Resume>()
        .command::<worker::Hear>()
        .command::<worker::EndWorker>()
        .command::<worker::NoteLoad>()
        .command::<worker::NoteQuiet>()
        .command::<worker::HandBrief>()
        .aggregate::<gate::Gates>()
        .command::<gate::RecordGate>()
        .aggregate::<Lines>()
        .command::<line::RecordLanding>()
        .command::<line::Promote>()
        .command::<line::RecordCut>()
        .command::<HoldLandings>()
        .aggregate::<Kits>()
        .command::<SetTheKit>()
        .command::<PinCheckout>()
        .command::<ReadWindows>()
        .command::<LimitAccount>()
        .aggregate::<HostSettings>()
        .command::<SetSetting>()
        .aggregate::<heavy::Heavy>()
        .command::<heavy::EnqueueHeavy>()
        .command::<heavy::StartHeavy>()
        .command::<heavy::EndHeavy>()
        .aggregate::<daemon::Daemons>()
        .command::<daemon::StartDaemon>()
        .command::<daemon::NoteTick>()
        .aggregate::<watch::Watches>()
        .command::<watch::StartWatch>()
        .command::<watch::EndWatch>()
        .command::<watch::LookNow>()
        .command::<watch::SendRound>()
        .command::<watch::HandWatchBrief>()
        .command::<watch::FileReport>()
        .command::<watch::NoteAdmission>()
        .command::<watch::SeeHuman>()
        .projection(Tracker)
}

impl App {
    // ---- checkouts ----

    /// Puts a repo on the list. `base` is where promotions land, `proving`
    /// where landings land. The repo needs an `origin` holding `base`; a
    /// proving branch it lacks is made at `base`.
    pub async fn checkout_add(&self, name: &str, path: &Path, base: &str, proving: &str) -> Answer {
        let path =
            std::fs::canonicalize(path).map_err(|e| Refusal::new("no_path", format!("{}: {e}", path.display())))?;
        let git = self.host.git(&path);
        let base_tip = git.fetch(base).map_err(|e| Refusal::new("no_base", format!("origin has no {base}: {e}")))?;
        if git.fetch(proving).is_err() {
            git.push_requiring(&base_tip, proving, None).map_err(crate::fail("no_proving"))?;
        }
        let add = Add {
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
            base: base.into(),
            proving: proving.into(),
        };
        self.submit::<AddCheckout>(&add).await?;
        Ok(json!(add))
    }

    pub async fn checkout_remove(&self, name: &str) -> Answer {
        self.submit::<RemoveCheckout>(Named { name: name.into() }).await?;
        Ok(json!({ "name": name, "removed": true }))
    }

    pub async fn checkout_list(&self) -> Answer {
        Ok(shown(&self.rows(tracker::listed()).await?))
    }

    // ---- issues ----

    /// Makes an issue numbered one past the checkout's last, waiting from
    /// the start for the issues of the checkout in `blocked_by`, so no tick
    /// sees it ready without them.
    pub async fn issue_add(&self, checkout: &str, title: &str, body: &str, parked: bool, blocked_by: &[u64]) -> Answer {
        self.checkout(checkout).await?;
        for by in blocked_by {
            self.issue(checkout, *by).await?;
        }
        loop {
            let last = self.rows(tracker::last_issue(checkout)).await?;
            let number = last.first().map_or(0, |r| r.number) + 1;
            let open = Open {
                checkout: checkout.into(),
                number,
                title: title.into(),
                body: body.into(),
                parked,
                blocked_by: blocked_by.to_vec(),
            };
            match self.submit::<OpenIssue>(&open).await {
                // Another command took the number first: take the next.
                Err(r) if r.code == "number_taken" => continue,
                Err(r) => return Err(r),
                Ok(()) => return self.issue_show(checkout, number).await,
            }
        }
    }

    /// The issues of a checkout, or of every checkout; open ones unless
    /// `closed` says to list the closed ones too.
    pub async fn issue_list(&self, checkout: Option<&str>, closed: bool) -> Answer {
        Ok(shown(&self.rows(tracker::issues(checkout, closed)).await?))
    }

    pub(crate) async fn issue(&self, checkout: &str, number: u64) -> Answer<Issue> {
        let i = self.state::<Issues>(issue_id(checkout, number)).await?;
        if i.opened() { Ok(i) } else { Err(Refusal::new("no_issue", format!("{checkout}#{number} does not exist"))) }
    }

    pub async fn issue_show(&self, checkout: &str, number: u64) -> Answer {
        let i = self.issue(checkout, number).await?;
        Ok(json!({
            "checkout": i.checkout,
            "number": i.number,
            "title": i.title,
            "body": i.body,
            "status": i.status.as_str(),
            "blocked_by": i.blocked_by,
            "reason": i.reason,
            "worker": i.worker,
            "comments": i.comments,
        }))
    }

    pub async fn issue_edit(&self, checkout: &str, number: u64, title: Option<&str>, body: Option<&str>) -> Answer {
        let edit = Edit { checkout: checkout.into(), number, title: title.map(Into::into), body: body.map(Into::into) };
        self.submit::<EditIssue>(edit).await?;
        self.issue_show(checkout, number).await
    }

    /// Closes an issue; a worker on it ends.
    pub async fn issue_close(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        let worker = self.issue(checkout, number).await?.worker;
        self.submit::<CloseIssue>(Because { checkout: checkout.into(), number, reason: reason.into() }).await?;
        if let Some(w) = worker {
            self.end_worker(w, &format!("its issue closed: {reason}"), true).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// Parks an issue; a worker on it ends, keeping its worktree.
    pub async fn issue_park(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        let worker = self.issue(checkout, number).await?.worker;
        self.submit::<ParkIssue>(Because { checkout: checkout.into(), number, reason: reason.into() }).await?;
        if let Some(w) = worker {
            self.end_worker(w, &format!("its issue was parked: {reason}"), false).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// Starts an issue over on purpose: the worktree its last attempt kept
    /// is removed, so its next start begins from the proving tip. Nothing is
    /// lost: what was not committed there is committed, the branch is kept
    /// as `<branch>-abandoned`, and a comment names it with `why`. Refused
    /// while a worker is on the issue; its state (ready or parked) stays.
    pub async fn issue_start_over(&self, checkout: &str, number: u64, why: &str) -> Answer {
        if why.trim().is_empty() {
            return Err(Refusal::new("no_reason", "say why the issue starts over"));
        }
        let issue = self.issue(checkout, number).await?;
        if issue.worker.is_some() {
            return Err(Refusal::new(
                "live_worker",
                format!(
                    "a worker is on {checkout}#{number}: park it first (factorium issue park {checkout} {number} …)"
                ),
            ));
        }
        let c = self.checkout(checkout).await?;
        let Some((worktree, abandoned)) = self.discard_kept(&c, &issue).await? else {
            let mut shown = self.issue_show(checkout, number).await?;
            shown["discarded"] = json!(false);
            shown["said"] = json!("nothing was kept to discard: the next start begins from the proving tip");
            return Ok(shown);
        };
        let text = format!(
            "Started over: attempt {}'s worktree was discarded; its branch is kept as `{abandoned}` (not landed). Why: {why}",
            issue.attempts
        );
        self.submit::<CommentIssue>(Comment { checkout: checkout.into(), number, text }).await?;
        let mut shown = self.issue_show(checkout, number).await?;
        shown["discarded"] = json!(worktree);
        shown["abandoned"] = json!(abandoned);
        Ok(shown)
    }

    pub async fn issue_ready(&self, checkout: &str, number: u64) -> Answer {
        self.submit::<ReadyIssue>(Ref { checkout: checkout.into(), number }).await?;
        self.issue_show(checkout, number).await
    }

    /// `issue comment`: records the comment, then passes it to the worker
    /// on the issue, if one is ([`App::pass_comment`]). The answer's
    /// `delivery` says how; `undelivered`, why it did not reach the worker.
    /// Comments factorium writes itself are recorded only.
    pub async fn issue_comment(&self, checkout: &str, number: u64, text: &str) -> Answer {
        self.submit::<CommentIssue>(Comment { checkout: checkout.into(), number, text: text.into() }).await?;
        let (delivery, why) = match self.issue(checkout, number).await?.worker {
            Some(worker) => self.pass_comment(worker, text).await?,
            None => ("none", None),
        };
        let mut shown = self.issue_show(checkout, number).await?;
        shown["delivery"] = json!(delivery);
        if let Some(why) = why {
            shown["undelivered"] = json!(why);
        }
        Ok(shown)
    }

    /// Makes issue `number` wait for issue `by` of the same checkout, or,
    /// with `undo`, stop waiting for it.
    pub async fn issue_block(&self, checkout: &str, number: u64, by: u64, undo: bool) -> Answer {
        let block = Block { checkout: checkout.into(), number, by };
        if undo {
            self.submit::<UnblockIssue>(block).await?;
        } else {
            self.issue(checkout, by).await?;
            self.submit::<BlockIssue>(block).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// The workers, live ones unless `all`.
    pub async fn workers(&self, all: bool) -> Answer {
        Ok(shown(&self.rows(tracker::workers(None, all)).await?))
    }

    // ---- the kit and the accounts ----

    pub async fn kit(&self) -> Answer {
        let mut kit = self.state::<Kits>(kit_id()).await?;
        // The command that will run, as a launch makes it.
        for e in &mut kit.entries {
            if let Some(h) = factorium_harness::Harness::read(&e.harness) {
                e.command = h.command(&e.command);
            }
        }
        Ok(json!({ "entries": kit.entries, "pins": kit.pins }))
    }

    /// Sets the kit: `harness=command[@model]` entries, in priority order.
    pub async fn kit_set(&self, entries: &[String]) -> Answer {
        let parse = |e: &String| -> Answer<Entry> {
            let (h, rest) =
                e.split_once('=').ok_or_else(|| Refusal::new("bad_entry", format!("{e}: harness=command[@model]")))?;
            let (command, model) = match rest.rsplit_once('@') {
                Some((c, m)) if !m.contains(' ') => (c, Some(m.to_string())),
                _ => (rest, None),
            };
            Ok(Entry { harness: h.trim().into(), command: command.trim().into(), model })
        };
        let entries = entries.iter().map(parse).collect::<Answer<Vec<_>>>()?;
        for e in &entries {
            known(&e.harness)?;
        }
        self.submit::<SetTheKit>(SetKit { entries }).await?;
        self.kit().await
    }

    /// Pins a checkout to one harness of the kit, or unpins it.
    pub async fn kit_pin(&self, checkout: &str, harness: Option<&str>) -> Answer {
        self.checkout(checkout).await?;
        harness.map(known).transpose()?;
        self.submit::<PinCheckout>(Pin { checkout: checkout.into(), harness: harness.map(Into::into) }).await?;
        self.kit().await
    }

    /// The engine accounts mytokens holds, as this machine can use them,
    /// with what each has left.
    pub async fn accounts(&self) -> Answer {
        let kit = self.state::<Kits>(kit_id()).await?;
        let now = self.now();
        let none = Account::default();
        let rows: Vec<Value> = self
            .accounts_here()
            .usable
            .into_iter()
            .map(|u| {
                let a = kit.accounts.get(&u.name).unwrap_or(&none);
                json!({ "name": u.name, "harness": u.harness.key(), "left": a.left(now), "limited_until": a.limited_until, "windows": a.windows })
            })
            .collect();
        Ok(Value::Array(rows))
    }

    // ---- settings and holds ----

    /// The settings, or, given a value, sets one.
    pub async fn settings_set(&self, name: Option<&str>, value: Option<u64>) -> Answer {
        if let (Some(name), Some(value)) = (name, value) {
            self.submit::<SetSetting>(Set { name: name.into(), value }).await?;
        }
        let s = self.settings().await?;
        let row = |t: Setting| json!({ "name": t.name(), "value": s.get(t), "unit": t.unit().name(), "default": t.default(), "what": t.what() });
        Ok(match name {
            Some(n) => row(Setting::named(n).ok_or_else(|| Refusal::new("no_setting", n))?),
            None => Setting::all().map(row).collect(),
        })
    }

    /// Holds a checkout's landings, or with `None` releases them.
    pub async fn hold(&self, checkout: &str, why: Option<&str>) -> Answer {
        self.checkout(checkout).await?;
        self.submit::<HoldLandings>(Hold { checkout: checkout.into(), why: why.map(Into::into) }).await?;
        let l = self.state::<Lines>(line_id(checkout)).await?;
        Ok(json!({ "checkout": checkout, "held": l.held }))
    }

    /// `factorium unqueue`: takes entry `id` off the heavy queue, waiting or
    /// running; its owner, if it still asks, queues again behind the rest.
    pub async fn unqueue(&self, id: Uuid) -> Answer {
        let q = self.state::<heavy::Heavy>(heavy::queue_id()).await?;
        let Some(entry) = q.entries.into_iter().find(|e| e.id == id) else {
            return Err(Refusal::new("no_entry", format!("the heavy queue holds no entry {id}")));
        };
        self.submit::<heavy::EndHeavy>(heavy::End { id }).await?;
        Ok(json!({ "unqueued": entry }))
    }

    // ---- doctor ----

    /// What doctor reads, from the database and a reading.
    pub(crate) async fn seen(&self) -> Answer<doctor::Seen> {
        let mut seen = doctor::Seen::default();
        for w in self.rows(tracker::workers(Some(worker::Status::Waiting), false)).await? {
            let issue = format!("{}#{}", w.checkout, w.number);
            let heard = self.state::<worker::Workers>(w.worker).await?.heard.into_iter().map(|(_, t)| t).collect();
            seen.waiting.push((w.worker.to_string(), issue, w.state.why.unwrap_or_default(), heard));
        }
        for w in self.rows(tracker::workers(Some(worker::Status::Landing), false)).await? {
            seen.landing.push((w.worker.to_string(), w.checkout, w.number, w.state.since));
        }
        for c in self.rows(tracker::listed()).await? {
            let line = self.state::<Lines>(line_id(&c.name)).await?;
            // What the base declares, as last fetched: doctor fetches nothing.
            let git = self.host.git(&c.path);
            let cuts = self.declared(&git, &format!("refs/remotes/origin/{}", c.base)).is_ok_and(|d| d.cut.is_some());
            seen.lines.push(doctor::LineSeen { checkout: c.name, line, cuts });
        }
        seen.queue = self.state::<heavy::Heavy>(heavy::queue_id()).await?;
        let w = self.state::<watch::Watches>(watch::watch_id()).await?;
        seen.held = w.held.clone();
        seen.daemon = self.state::<daemon::Daemons>(daemon::daemon_id()).await?;
        seen.human = w.reports.iter().filter(|r| r.round > w.seen).filter_map(|r| r.human.clone()).collect();
        seen.orphans = self.orphans(&w).await?;
        let dead = self.engine.dead_letters(factorium_domain::tracker::TRACKER).await?;
        if !dead.is_empty() {
            seen.dead_letters.push((factorium_domain::tracker::TRACKER.into(), dead.len()));
        }
        Ok(seen)
    }

    /// Orca's terminals this database's factorium opened (a worker's, ended
    /// or not, or a watch session's) that neither a live worker nor the
    /// watch's session now holds. A terminal another daemon, on another
    /// database, opened is never this one's, whatever its title. Orca
    /// unreadable: none known.
    async fn orphans(&self, watch: &watch::Watch) -> Answer<Vec<(String, String)>> {
        let Ok(terminals) = self.host.orca().terminals() else { return Ok(Vec::new()) };
        let (mut opened, mut held) = (watch.opened.clone(), Vec::new());
        for w in self.rows(tracker::workers(None, true)).await? {
            if w.state.status != worker::Status::Ended {
                held.push(w.terminal.clone());
            }
            opened.push(w.terminal);
        }
        held.extend(watch.terminal.clone());
        Ok(terminals.into_iter().filter(|(h, _)| opened.contains(h) && !held.contains(h)).collect())
    }

    /// The findings, each with its fix; the reading; and the owner's part of
    /// the watch's reports, which this marks seen.
    pub async fn doctor(&self) -> Answer {
        let seen = self.seen().await?;
        let settings = self.settings().await?;
        let findings = doctor::findings(&seen, &settings, self.now());
        let w = self.state::<watch::Watches>(watch::watch_id()).await?;
        if let Some(last) = w.reports.last() {
            self.submit::<watch::SeeHuman>(watch::Seen { round: last.round }).await?;
        }
        let reading = self.host.reading();
        let holds: Vec<_> =
            seen.lines.iter().filter_map(|l| Some((l.checkout.clone(), l.line.held.clone()?))).collect();
        let skipped: Vec<Value> =
            self.accounts_here().skipped.into_iter().map(|s| json!({ "secret": s.secret, "why": s.why })).collect();
        Ok(json!({
            // The engine accounts no session can start on here, and why.
            "accounts_skipped": skipped,
            "heavy": seen.queue.entries,
            "findings": findings,
            "human": seen.human,
            "holds": holds,
            "reading": reading,
            "watch": { "terminal": w.terminal, "round": w.round, "episodes": w.episodes.len() },
        }))
    }
}

/// Refuses `harness` unless it names a harness factorium starts.
fn known(harness: &str) -> Answer<()> {
    if Harness::read(harness).is_some() {
        return Ok(());
    }
    let keys: Vec<&str> = Harness::all().iter().map(|h| h.key()).collect();
    Err(Refusal::new("no_harness", format!("{harness} is none of {}", keys.join(", "))))
}
