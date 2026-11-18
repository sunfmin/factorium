//! The owner's commands: the checkout list, the tracker, the kit, the
//! accounts, the settings, holds, and doctor.

use std::path::Path;

use factorium_domain::checkout::{Add, AddCheckout, Named, RemoveCheckout};
use factorium_domain::idle::{Idles, idle_id};
use factorium_domain::issue::*;
use factorium_domain::kit::*;
use factorium_domain::line::{Hold, HoldLandings, Lines};
use factorium_domain::settings::{Set, SetSetting, Setting};
use factorium_domain::tracker::{self, shown};
use factorium_domain::{daemon, doctor, heavy, watch, worker};
use factorium_domain::{issue_id, line_id};
use factorium_harness::Harness;
use factum::{Uuid, Value};
use serde_json::json;

use crate::life::End;
use crate::{Answer, App, Refusal};

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
    /// the start for the issues in `blocked_by`, of its checkout or of
    /// others, so no tick sees it ready without them.
    pub async fn issue_add(
        &self,
        checkout: &str,
        title: &str,
        body: &str,
        waiting_for_human: bool,
        blocked_by: &[Blocker],
    ) -> Answer {
        self.checkout(checkout).await?;
        let (mut here, mut across) = (Vec::new(), Vec::new());
        for by in blocked_by {
            match self.blocker(checkout, by.clone()).await? {
                Blocker::Here(n) => here.push(n),
                Blocker::Across(a) => across.push(a),
            }
        }
        loop {
            let last = self.rows(tracker::last_issue(checkout)).await?;
            let number = last.first().map_or(0, |r| r.number) + 1;
            let open = Open {
                checkout: checkout.into(),
                number,
                title: title.into(),
                body: body.into(),
                waiting_for_human,
                blocked_by: here.clone(),
                across: across.clone(),
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
            "status": i.status,
            "blocked_by": i.blocked_by,
            "blocked_across": i.across.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "reason": i.reason,
            "worker": i.worker,
            "comments": i.comments.iter().map(|c| json!({ "text": c.text, "by": c.by.to_string(), "at": c.at })).collect::<Vec<_>>(),
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
            self.end_worker(w, End::because(format!("its issue closed: {reason}"), false)).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// Leaves an issue to a person: only `issue ready` moves it again. A
    /// worker on it ends, keeping its worktree.
    pub async fn issue_wait_for_human(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        let worker = self.issue(checkout, number).await?.worker;
        self.submit::<WaitForHuman>(Because { checkout: checkout.into(), number, reason: reason.into() }).await?;
        if let Some(w) = worker {
            self.end_worker(w, End::because(format!("its issue waits for a person: {reason}"), true)).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// Starts an issue over on purpose: the worktree its last attempt kept
    /// is removed, so its next start begins from the proving tip. Nothing is
    /// lost: what was not committed there is committed, the branch is kept
    /// as `<branch>-abandoned`, and a comment names it with `why`. Refused
    /// while a worker is on the issue; its state (ready or waiting for a person) stays.
    pub async fn issue_start_over(&self, checkout: &str, number: u64, why: &str) -> Answer {
        if why.trim().is_empty() {
            return Err(Refusal::new("no_reason", "say why the issue starts over"));
        }
        let issue = self.issue(checkout, number).await?;
        if issue.worker.is_some() {
            return Err(Refusal::new(
                "live_worker",
                format!(
                    "a worker is on {checkout}#{number}: leave it to a person first (factorium issue wait-for-human {checkout} {number} --why …)"
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
        self.note(checkout, number, Author::Factorium, &text).await?;
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
        self.note(checkout, number, Author::Owner, text).await?;
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

    /// Records a comment on an issue, the one record of what is said about
    /// it: by the owner, its workers, the watch, and factorium. Recording
    /// passes it to no one.
    pub(crate) async fn note(&self, checkout: &str, number: u64, by: Author, text: &str) -> Answer<()> {
        self.submit::<CommentIssue>(Comment { checkout: checkout.into(), number, text: text.into(), by }).await?;
        Ok(())
    }

    /// Makes issue `number` wait for `by`, an issue of its checkout or of
    /// another, or, with `undo`, stop waiting for it.
    pub async fn issue_block(&self, checkout: &str, number: u64, by: impl Into<Blocker>, undo: bool) -> Answer {
        let by = by.into().of(checkout);
        let by = if undo { by } else { self.blocker(checkout, by).await? };
        let c = checkout.to_string();
        match (by, undo) {
            (Blocker::Here(by), false) => self.submit::<BlockIssue>(Block { checkout: c, number, by }).await?,
            (Blocker::Here(by), true) => self.submit::<UnblockIssue>(Block { checkout: c, number, by }).await?,
            (Blocker::Across(by), false) => {
                self.submit::<BlockIssueAcross>(BlockAcross { checkout: c, number, by }).await?
            }
            (Blocker::Across(by), true) => {
                self.submit::<UnblockIssueAcross>(BlockAcross { checkout: c, number, by }).await?
            }
        }
        self.issue_show(checkout, number).await
    }

    /// `by` as an issue of `checkout` waits for it, refused unless that
    /// issue exists and, for its cut, its repo declares one.
    async fn blocker(&self, checkout: &str, by: Blocker) -> Answer<Blocker> {
        let by = by.of(checkout);
        match &by {
            Blocker::Here(n) => drop(self.issue(checkout, *n).await?),
            Blocker::Across(a) => {
                let c = self.checkout(&a.checkout).await?;
                self.issue(&a.checkout, a.number).await?;
                if a.cut && self.declared(&self.host.git(&c.path), &c.base)?.cut.is_none() {
                    let why =
                        format!("{} declares no cut for {a} to wait for: name {}#{}", a.checkout, a.checkout, a.number);
                    return Err(Refusal::new("no_cut", why));
                }
            }
        }
        Ok(by)
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
                json!({ "name": u.name, "harness": u.harness.key(), "left": a.left(now), "limited_until": a.limited_until })
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
            let why = w.state.why.unwrap_or_default();
            seen.waiting.push(doctor::WaitingSeen { worker: w.worker.to_string(), issue, why, heard });
        }
        for w in self.rows(tracker::workers(Some(worker::Status::Landing), false)).await? {
            let (worker, since) = (w.worker.to_string(), w.state.since);
            seen.landing.push(doctor::LandingSeen { worker, checkout: w.checkout, number: w.number, since });
        }
        for c in self.rows(tracker::listed()).await? {
            let line = self.state::<Lines>(line_id(&c.name)).await?;
            // What the base declares, as last fetched: doctor fetches nothing.
            let git = self.host.git(&c.path);
            let cuts = self.declared(&git, &format!("refs/remotes/origin/{}", c.base)).is_ok_and(|d| d.cut.is_some());
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
        let dead = self.engine.dead_letters(factorium_domain::tracker::TRACKER).await?;
        if !dead.is_empty() {
            let projection = factorium_domain::tracker::TRACKER.into();
            seen.dead_letters.push(doctor::DeadLetters { projection, count: dead.len() });
        }
        Ok(seen)
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
