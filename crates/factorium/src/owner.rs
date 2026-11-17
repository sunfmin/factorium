//! The owner's commands: the checkout list, the tracker, the kit, the
//! accounts, the settings, holds, and doctor.

use std::path::Path;

use factorium_domain::checkout::{Add, AddCheckout, Checkouts, Named, RemoveCheckout};
use factorium_domain::issue::*;
use factorium_domain::kit::*;
use factorium_domain::line::{Hold, HoldLandings, Lines};
use factorium_domain::settings::{HostSettings, Set, SetSetting, Setting};
use factorium_domain::tracker::{self, Tracker, shown};
use factorium_domain::{doctor, gate, heavy, line, watch, worker};
use factorium_domain::{issue_id, line_id};
use factum::{Model, Value};
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
        .command::<worker::GreenLanding>()
        .command::<worker::Wait>()
        .command::<worker::Resume>()
        .command::<worker::EndWorker>()
        .command::<worker::NoteLoad>()
        .aggregate::<gate::Gates>()
        .command::<gate::RecordGate>()
        .aggregate::<Lines>()
        .command::<line::RecordLanding>()
        .command::<line::RecordPromotion>()
        .command::<line::RedPromotion>()
        .command::<line::RecordCut>()
        .command::<line::FailCut>()
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
        .aggregate::<watch::Watches>()
        .command::<watch::StartWatch>()
        .command::<watch::LookNow>()
        .command::<watch::SendRound>()
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
        let git = factorium_host::Git::at(&path);
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

    pub async fn issue_ready(&self, checkout: &str, number: u64) -> Answer {
        self.submit::<ReadyIssue>(Ref { checkout: checkout.into(), number }).await?;
        self.issue_show(checkout, number).await
    }

    pub async fn issue_comment(&self, checkout: &str, number: u64, text: &str) -> Answer {
        self.submit::<CommentIssue>(Comment { checkout: checkout.into(), number, text: text.into() }).await?;
        self.issue_show(checkout, number).await
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
        let kit = self.state::<Kits>(kit_id()).await?;
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
        self.submit::<SetTheKit>(SetKit { entries }).await?;
        self.kit().await
    }

    /// Pins a checkout to one harness of the kit, or unpins it.
    pub async fn kit_pin(&self, checkout: &str, harness: Option<&str>) -> Answer {
        self.checkout(checkout).await?;
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
            .into_iter()
            .map(|(name, harness, _)| {
                let a = kit.accounts.get(&name).unwrap_or(&none);
                json!({ "name": name, "harness": harness, "left": a.left(now), "limited_until": a.limited_until, "windows": a.windows })
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

    // ---- doctor ----

    /// What doctor reads, from the database and a reading.
    pub(crate) async fn seen(&self) -> Answer<doctor::Seen> {
        let mut seen = doctor::Seen::default();
        for w in self.rows(tracker::workers(Some(worker::Status::Waiting), false)).await? {
            let issue = format!("{}#{}", w.checkout, w.number);
            seen.waiting.push((w.worker.to_string(), issue, w.state.why.unwrap_or_default()));
        }
        for c in self.rows(tracker::listed()).await? {
            let name = c.name.as_str();
            let l = self.state::<Lines>(line_id(name)).await?;
            if let Some((tip, why)) = l.red {
                seen.red_promotions.push((name.into(), tip, why));
            }
            if let Some((v, why)) = l.cut_failed {
                seen.failed_cuts.push((name.into(), v, why));
            }
            if let Some(why) = l.held {
                seen.holds.push((name.into(), why));
            }
        }
        let w = self.state::<watch::Watches>(watch::watch_id()).await?;
        seen.held = w.held.clone();
        seen.human = w.reports.iter().filter(|r| r.round > w.seen).filter_map(|r| r.human.clone()).collect();
        let dead = self.engine.dead_letters(factorium_domain::tracker::TRACKER).await?;
        if !dead.is_empty() {
            seen.dead_letters.push((factorium_domain::tracker::TRACKER.into(), dead.len()));
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
        Ok(json!({
            "findings": findings,
            "human": seen.human,
            "holds": seen.holds,
            "reading": reading,
            "watch": { "terminal": w.terminal, "round": w.round, "episodes": w.episodes.len() },
        }))
    }
}
