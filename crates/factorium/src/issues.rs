//! The tracker: every `issue` command, and the issue reads and notes the
//! daemon, the line, the watch and a worker's life share.

use factorium_domain::checkout::Checkouts;
use factorium_domain::decided;
use factorium_domain::issue::*;
use factorium_domain::line::Lines;
use factorium_domain::line_id;
use factorium_domain::spent;
use factorium_domain::tracker::{self, shown};
use factorium_domain::when::{self, Since};
use factorium_domain::worker::{Started, Status as WorkerStatus, Tokens, Worker, Workers};
use factorium_domain::{checkout_id, issue_id};
use factum::{Aggregate, Command};
use serde_json::json;

use crate::kept::Why;
use crate::life::End;
use crate::{Answer, App, Refusal};

/// A Decided record as [`App::records`] finds it.
pub(crate) struct Found {
    pub checkout: String,
    pub number: u64,
    pub at: Option<i64>,
    pub place: Place,
    pub id: Option<u64>,
    pub record: decided::Record,
}

/// One thing an issue waits for: as the CLI names it, why (none for its
/// time), and how it was met, or `None` while it is not.
#[derive(serde::Serialize)]
pub(crate) struct Waited {
    pub on: String,
    pub why: Option<String>,
    pub met: Option<String>,
}

impl App {
    /// Makes an issue numbered one past the checkout's last, waiting from
    /// the start for the issues in `blocked_by`, of its checkout or of
    /// others, each for its why, so no tick sees it ready without them.
    pub async fn issue_add(
        &self,
        checkout: &str,
        title: &str,
        body: &str,
        waiting_for_human: bool,
        blocked_by: &[Wait],
    ) -> Answer {
        let waiting = waiting_for_human.then_some(None);
        self.issue_add_on(checkout, title, body, waiting, blocked_by, On::default()).await
    }

    /// [`App::issue_add`], its worker to start as `on` says: with the rules
    /// and skills of the role directory it names, on the account it names.
    /// Refused unless the role is a directory named by its absolute path and
    /// the account is one this host can use. `waiting`: it waits for a
    /// person from the start, with why if one is given.
    pub async fn issue_add_on(
        &self,
        checkout: &str,
        title: &str,
        body: &str,
        waiting: Option<Option<&str>>,
        blocked_by: &[Wait],
        on: On,
    ) -> Answer {
        let (title, body) = (title.into(), body.into());
        let open = Open {
            checkout: checkout.into(),
            title,
            body,
            waiting_for_human: waiting.is_some(),
            why: waiting.flatten().map(String::from),
            blocked_by: blocked_by.to_vec(),
            on,
            ..Open::default()
        };
        self.issue_file(open).await
    }

    /// `issue add` as `open` says it, its urgent mark among it: checked as
    /// [`App::issue_add_on`], filed, and answered.
    pub async fn issue_file(&self, open: Open) -> Answer {
        self.checkout(&open.checkout).await?;
        self.on_here(&open.on)?;
        for w in &open.blocked_by {
            self.blocker(&open.checkout, w.by.clone()).await?;
        }
        let checkout = open.checkout.clone();
        let number = self.open_issue(open).await?;
        self.issue_show(&checkout, number).await
    }

    /// Whether `worker`'s issue is marked urgent now: none started, no.
    pub(crate) async fn urgent_worker(&self, worker: factum::Uuid) -> Answer<bool> {
        let Some(st) = self.state::<Workers>(worker).await?.started else { return Ok(false) };
        Ok(self.issue(&st.checkout, st.number).await.is_ok_and(|i| i.urgent))
    }

    /// Records which urgent issues halt the host now
    /// (docs/adr/0015-an-urgent-issue-halts-the-host.md), read from the open issues.
    pub(crate) async fn note_halt(&self) -> Answer<()> {
        let mut urgent = Vec::new();
        for r in self.rows(tracker::issues(None, false)).await?.into_iter().filter(|r| r.urgent) {
            urgent.push(self.issue(&r.checkout, r.number).await?);
        }
        let by: Vec<String> = halts(&urgent).map(|i| format!("{}#{}", i.checkout, i.number)).collect();
        let workers = halts(&urgent).filter_map(|i| i.worker).collect();
        let halt = factorium_domain::heavy::Halt { by, workers };
        self.submit::<factorium_domain::heavy::NoteHalt>(halt).await
    }

    /// `issue edit --urgent`/`--no-urgent`: the owner's mark, which puts its
    /// start, its worker's heavy commands and its landing ahead (docs/adr/0015-an-urgent-issue-halts-the-host.md).
    pub async fn issue_mark_urgent(&self, checkout: &str, number: u64, urgent: bool) -> Answer {
        self.issue(checkout, number).await?;
        self.submit::<MarkUrgent>(Urgency { checkout: checkout.into(), number, urgent }).await?;
        self.issue_show(checkout, number).await
    }

    /// Opens `open` as the checkout's next issue; answers its number.
    pub(crate) async fn open_issue(&self, mut open: Open) -> Answer<u64> {
        open.decided = self.give_numbers(decided::in_body(&open.body).len()).await?;
        loop {
            let last = self.rows(tracker::last_issue(&open.checkout)).await?;
            open.number = last.first().map_or(0, |r| r.number) + 1;
            match self.submit::<OpenIssue>(&open).await {
                // Another command took the number first: take the next.
                Err(r) if r.code == "number_taken" => continue,
                Err(r) => return Err(r),
                Ok(()) => return Ok(open.number),
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
        self.shown_issue(checkout, number, false).await
    }

    /// `issue show --steps`: each attempt in `spent` answers too its
    /// `steps`, where its time went (factorium#308).
    pub async fn issue_show_steps(&self, checkout: &str, number: u64) -> Answer {
        self.shown_issue(checkout, number, true).await
    }

    async fn shown_issue(&self, checkout: &str, number: u64, steps: bool) -> Answer {
        let i =
            self.issue(checkout, number).await?.numbered(&self.state::<decided::Numbers>(decided::numbers_id()).await?);
        let c = self.state::<Checkouts>(checkout_id(checkout)).await?;
        let role = c.role_for(i.on.role.as_deref());
        Ok(json!({
            "checkout": i.checkout,
            "number": i.number,
            "title": i.title,
            "body": i.body,
            "status": i.status,
            "urgent": i.urgent,
            "blocked_by": i.blocked_by(),
            "blocked_across": i.blocked_across(),
            "waits": self.blockers(&i).await?,
            "not_before": i.not_before.map(|at| json!({ "ms": at, "utc": when::utc(at) })),
            "reason": i.reason,
            "worker": i.worker,
            "claim": i.claim,
            "task": i.task,
            "role": role.map(|(r, _)| r),
            "role_from": role.map(|(_, from)| from),
            "account": i.on.account,
            "model": i.on.model,
            "spent": self.issue_spent(&i, steps).await?,
            "decided": i.decided.iter().map(|m| decided::name(m.id)).collect::<Vec<_>>(),
            "comments": i.comments.iter().map(|c| {
                let mut shown = json!({ "text": c.text, "by": c.by.to_string(), "at": c.at });
                if let Some(id) = c.decided {
                    shown["decided"] = json!(decided::name(id));
                }
                shown
            }).collect::<Vec<_>>(),
        }))
    }

    /// Where `worker`'s time went, from its events, its heavy runs' rows and
    /// its session's outbox, `st` its start.
    async fn attempt_steps(&self, worker: factum::Uuid, st: &Started) -> Answer<factorium_domain::steps::Steps> {
        use factorium_domain::outbox::{OutboxEvent, Outboxes, outbox_id};
        use factorium_domain::worker::WorkerEvent;
        let events: Vec<(i64, WorkerEvent)> = self.timed(factum::StreamId::new(Workers::NAME, worker)).await?;
        let outbox: Vec<(i64, OutboxEvent)> =
            self.timed(factum::StreamId::new(Outboxes::NAME, outbox_id(&st.terminal))).await?;
        let runs = self.rows(tracker::runs_of(worker)).await?;
        Ok(factorium_domain::steps::steps(&events, &runs, &outbox, self.now()))
    }

    /// The events of `stream`, each as its aggregate's own type, with the time the log took it.
    async fn timed<E: serde::de::DeserializeOwned>(&self, stream: factum::StreamId) -> Answer<Vec<(i64, E)>> {
        let history = self.engine.history(&stream).await?;
        let decoded = history.iter().map(|e| tracker::decode(e).map(|d| (e.meta.now.0, d)));
        decoded.collect::<Result<_, _>>().map_err(|m| Refusal::new("bad_event", m))
    }

    pub async fn issue_edit(&self, checkout: &str, number: u64, title: Option<&str>, body: Option<&str>) -> Answer {
        // A record the edit adds takes the next number; one it keeps keeps its
        // own. Another edit that came first refuses it (`no_numbers`): edit again.
        let i = self.issue(checkout, number).await?;
        let fresh = self.give_numbers(unnumbered(&i, body.unwrap_or(&i.body))).await?;
        let (title, body) = (title.map(Into::into), body.map(Into::into));
        self.submit::<EditIssue>(Edit { checkout: checkout.into(), number, title, body, fresh }).await?;
        let passed = match i.worker {
            Some(worker) => self.pass_edit(worker, &i, &self.issue(checkout, number).await?).await?,
            None => None,
        };
        let mut shown = self.issue_show(checkout, number).await?;
        if let Some((delivery, why)) = passed {
            shown["delivery"] = json!(delivery);
            if let Some(why) = why {
                shown["undelivered"] = json!(why);
            }
        }
        Ok(shown)
    }

    /// `issue edit --role`/`--no-role`: names `role` (checked as at
    /// [`App::issue_add_on`]) for the workers of the issue that start from
    /// now, or, `None`, no role. A live worker keeps the directory it
    /// started in. A change is kept as a comment saying from what to what,
    /// so the next worker's brief carries why its role differs.
    pub async fn issue_name_role(&self, checkout: &str, number: u64, role: Option<&str>) -> Answer {
        let role = role.map(String::from);
        self.on_here(&On { role: role.clone(), ..On::default() })?;
        let was = self.issue(checkout, number).await?.on.role;
        if was != role {
            self.submit::<NameIssueRole>(NameRole { checkout: checkout.into(), number, role: role.clone() }).await?;
            let named = |r: &Option<String>| r.as_ref().map_or("no role".to_string(), |r| format!("`{r}`"));
            let text = format!(
                "The owner changed this issue's role from {} to {}: each attempt that starts from now starts with it; a worker already started keeps the role it started with.",
                named(&was),
                named(&role)
            );
            self.note(checkout, number, Author::Factorium, &text).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// Closes an issue; a worker on it ends. The attempt whose worktree its
    /// last attempt left on disk is abandoned ([`App::abandon`]). Claimed
    /// ([`App::claimed`]), but never refused for a start under way: the
    /// owner's stop wins. It closes, abandoning nothing, and the start ends
    /// its worker, its worktree kept ([`App::start_worker`]).
    pub async fn issue_close(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        let closed = self.claimed_close(checkout, number, reason).await;
        match closed {
            Err(r) if r.code == "claimed" => {
                // A start's, or one gone since: only another command's refuses.
                match self.issue(checkout, number).await?.claim {
                    Some(c) if c.by != Claimant::Start => Err(r),
                    _ => self.close(checkout, number, reason, true).await,
                }
            }
            closed => closed,
        }
    }

    async fn claimed_close(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        self.claimed(checkout, number, Claimant::Close, async {
            let issue = self.issue(checkout, number).await?;
            // A checkout off the list has no worktrees factorium removes.
            let left = match self.checkout(checkout).await {
                Ok(c) => self.left(&issue).await?.map(|st| (c, st)),
                Err(_) => None,
            };
            let Some((c, st)) = left else { return self.close(checkout, number, reason, false).await };
            let done = self.abandon(&c, &st, Why::Close(reason)).await?;
            Ok(done.shown(self.issue_show(checkout, number).await?))
        })
        .await
    }

    /// Closes an issue for `reason`; a worker on it ends, keeping its worktree if `keep`.
    pub(crate) async fn close(&self, checkout: &str, number: u64, reason: &str, keep: bool) -> Answer {
        self.leave::<CloseIssue>(checkout, number, reason, keep).await
    }

    /// Leaves an issue to a person: only `issue ready` moves it again. A
    /// worker on it ends, keeping its worktree; one still starting, its
    /// start ends ([`App::start_worker`]).
    pub async fn issue_wait_for_human(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        self.leave::<WaitForHuman>(checkout, number, reason, true).await
    }

    /// The issue leaves its worker: command `C` moves it, for `reason`, and
    /// the worker on it, if one is, ends as [`Issue::left_why`] says,
    /// keeping its worktree if `keep`.
    async fn leave<C: Command<Params = Because>>(
        &self,
        checkout: &str,
        number: u64,
        reason: &str,
        keep: bool,
    ) -> Answer {
        let worker = self.issue(checkout, number).await?.worker;
        self.submit::<C>(Because { checkout: checkout.into(), number, reason: reason.into() }).await?;
        if let (Some(w), Some(why)) = (worker, self.issue(checkout, number).await?.left_why()) {
            self.end_worker(w, End::because(why, keep)).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// Starts an issue over on purpose: the attempt whose worktree its last
    /// attempt kept is abandoned ([`App::abandon`]), so its next start
    /// begins from the proving tip; nothing is lost. Claimed
    /// ([`App::claimed`]), so refused while a worker is on the issue; its
    /// state (ready or waiting for a person) stays.
    pub async fn issue_start_over(&self, checkout: &str, number: u64, why: &str) -> Answer {
        if why.trim().is_empty() {
            return Err(Refusal::new("no_reason", "say why the issue starts over"));
        }
        self.claimed(checkout, number, Claimant::StartOver, async {
            let (c, issue) = (self.checkout(checkout).await?, self.issue(checkout, number).await?);
            let Some(st) = self.kept_now(&c, &issue).await? else {
                let mut shown = self.issue_show(checkout, number).await?;
                shown["discarded"] = json!(false);
                shown["said"] = json!("nothing was kept to discard: the next start begins from the proving tip");
                return Ok(shown);
            };
            let done = self.abandon(&c, &st, Why::StartOver(why)).await?;
            Ok(done.shown(self.issue_show(checkout, number).await?))
        })
        .await
    }

    /// Runs `act` holding issue `number`'s claim for `by`, freed once it
    /// is done (factorium#189): what `act` reads of the issue's worktrees,
    /// no start changes before it acts. Refused while another holds it; a
    /// claim whose process is gone is cleared first. A closed issue takes
    /// no start, so `act` runs on one unclaimed.
    pub(crate) async fn claimed<T>(
        &self,
        checkout: &str,
        number: u64,
        by: Claimant,
        act: impl Future<Output = Answer<T>>,
    ) -> Answer<T> {
        self.clear_dead_claim(&self.issue(checkout, number).await?).await?;
        let claim = Claiming { checkout: checkout.into(), number, claim: Claim { by, pid: std::process::id() } };
        match self.submit::<ClaimIssue>(claim.clone()).await {
            Err(r) if r.code == "ended" => return act.await,
            claimed => claimed?,
        }
        let done = act.await;
        self.unclaim(claim).await?;
        done
    }

    /// Frees `claim`, if it holds its issue still: closed, it holds none.
    pub(crate) async fn unclaim(&self, claim: Claiming) -> Answer<()> {
        match self.submit::<UnclaimIssue>(claim).await {
            Err(r) if r.code == "ended" => Ok(()),
            freed => freed,
        }
    }

    /// Clears `issue`'s claim if the process that holds it is gone.
    pub(crate) async fn clear_dead_claim(&self, issue: &Issue) -> Answer<()> {
        match &issue.claim {
            Some(c) if !self.host.alive(c.pid) => {
                let (checkout, number) = (issue.checkout.clone(), issue.number);
                self.unclaim(Claiming { checkout, number, claim: c.clone() }).await
            }
            _ => Ok(()),
        }
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
        self.issue(checkout, number).await?;
        let record = decided::in_comment(text).is_some();
        let decided = self.give_numbers(usize::from(record)).await?.first().copied();
        let comment = Comment { checkout: checkout.into(), number, text: text.into(), by, decided };
        self.submit::<CommentIssue>(comment).await?;
        Ok(())
    }

    /// The next `count` numbers of the host's Decided records, once the
    /// records made before numbers were given have theirs ([`App::number_decided`]).
    pub(crate) async fn give_numbers(&self, count: usize) -> Answer<Vec<u64>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        if !self.state::<decided::Numbers>(decided::numbers_id()).await?.swept {
            self.number_decided().await?;
        }
        self.numbers(count, false).await
    }

    /// `count` numbers given, as the sequence recorded them.
    async fn numbers(&self, count: usize, sweep: bool) -> Answer<Vec<u64>> {
        let events = self.decide::<decided::GiveNumbers>(decided::Give { count: count as u64, sweep }).await?;
        let mut given = Vec::new();
        for e in &events {
            if let Ok(decided::NumbersEvent::NumbersGiven { from, count, .. }) = tracker::decode(e) {
                given.extend(from..from + count);
            }
        }
        Ok(given)
    }

    /// Numbers every Decided record that has none, oldest first by when it
    /// was made (a body's, when its issue was filed): once for the records
    /// made before numbers were given, and after for any whose numbering
    /// did not finish. An open issue takes its numbers as its own event; a
    /// closed one takes none, so the sequence places them (ADR 0024).
    pub(crate) async fn number_decided(&self) -> Answer<()> {
        let swept = self.state::<decided::Numbers>(decided::numbers_id()).await?.swept;
        let mut open: Vec<_> = self.records().await?.into_iter().filter(|f| f.id.is_none()).collect();
        if open.is_empty() && swept {
            return Ok(());
        }
        open.sort_by_key(|f| f.at.unwrap_or(0));
        let ids = self.numbers(open.len(), true).await?;
        let mut placed = std::collections::BTreeMap::new();
        for (f, id) in open.into_iter().zip(ids) {
            let key = (f.checkout.clone(), f.number);
            let n = placed.entry(key).or_insert_with(|| decided::Numbering {
                checkout: f.checkout,
                number: f.number,
                body: Vec::new(),
                comments: Vec::new(),
            });
            match f.place {
                Place::Body => n.body.push(decided::Mark::of(&f.record, id)),
                Place::Comment(c) => n.comments.push((c, id)),
            }
        }
        let mut closed = Vec::new();
        for n in placed.into_values() {
            match self.submit::<NumberDecided>(n.clone()).await {
                Err(r) if r.code == "ended" => closed.push(n),
                done => done?,
            }
        }
        self.submit::<decided::PlaceNumbers>(decided::Place { placed: closed }).await
    }

    /// Every Decided record of every issue of any checkout, its numbers
    /// placed outside it among them ([`Issue::numbered`]): with when it was
    /// made (a body's, when its issue was filed; a comment's, if known).
    pub(crate) async fn records(&self) -> Answer<Vec<Found>> {
        let given = self.state::<decided::Numbers>(decided::numbers_id()).await?;
        let mut found = Vec::new();
        for r in self.rows(tracker::issues(None, true)).await? {
            let i = self.issue(&r.checkout, r.number).await?.numbered(&given);
            let records = i.records();
            let filed = match records.first() {
                Some((Place::Body, ..)) => Some(self.filed(&i).await?),
                _ => None,
            };
            for (place, id, record) in records {
                let at = match place {
                    Place::Body => filed,
                    Place::Comment(c) => i.comments[c].at,
                };
                found.push(Found { checkout: r.checkout.clone(), number: r.number, at, place, id, record });
            }
        }
        Ok(found)
    }

    /// When (ms) `issue` was filed: its first event's time.
    pub(crate) async fn filed(&self, issue: &Issue) -> Answer<i64> {
        let stream = factum::StreamId::new(Issues::NAME, issue_id(&issue.checkout, issue.number));
        Ok(self.engine.history(&stream).await?.first().map_or(0, |e| e.meta.now.0))
    }

    /// Makes issue `number` wait for `by`, an issue of its checkout or of
    /// another, for `why` (refused without one), or a time (which replaces
    /// the one it waited for), or, with `undo`, stop waiting for it.
    pub async fn issue_block(
        &self,
        checkout: &str,
        number: u64,
        by: impl Into<Blocker>,
        why: Option<&str>,
        undo: bool,
    ) -> Answer {
        let by = by.into();
        if !undo {
            self.blocker(checkout, by.clone()).await?;
        }
        let why = why.map(String::from);
        let block = Block { checkout: checkout.into(), number, by: by.clone(), why: why.clone(), undo };
        self.submit::<BlockIssue>(block).await?;
        // Its worker is parked behind it (ADR 0020): back to work, once idle, as soon as all it waits for is met.
        if let (false, Some(worker)) = (undo, self.issue(checkout, number).await?.worker) {
            let on = match by.of(checkout) {
                Blocker::Here(n) => format!("#{n}"),
                Blocker::Across(a) => a.to_string(),
                Blocker::NotBefore(at) => format!("not before {}", when::utc(at)),
            };
            let why = format!("waits for {on}: {}", why.as_deref().unwrap_or("(no reason given)"));
            self.submit::<factorium_domain::worker::Park>(factorium_domain::worker::Because { worker, why }).await?;
        }
        self.issue_show(checkout, number).await
    }

    /// `by` as an issue of `checkout` waits for it, refused unless that
    /// issue exists and, for its cut, its repo declares one.
    async fn blocker(&self, checkout: &str, by: Blocker) -> Answer<Blocker> {
        let by = by.of(checkout);
        match &by {
            Blocker::Here(n) => drop(self.issue(checkout, *n).await?),
            Blocker::NotBefore(_) => {}
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

    /// What `issue` waits for, each named, and how it was met (its issue's
    /// close, such as the landing), or `None` while it is not: met once its
    /// issue is closed (or gone), and, for its cut, its landing is in a cut.
    /// Its landing is recorded before it closes, so a closed issue the line
    /// does not hold either landed in a cut or never landed. Its time, last,
    /// is met once the clock reaches it. Each says why it is waited for: its
    /// time, nothing.
    pub(crate) async fn blockers(&self, issue: &Issue) -> Answer<Vec<Waited>> {
        let mut all = Vec::new();
        for (on, a) in issue.waits_for() {
            let met = match self.issue(&a.checkout, a.number).await {
                Ok(i) if i.status != Status::Closed => None,
                _ if a.cut && self.state::<Lines>(line_id(&a.checkout)).await?.awaits_cut(a.number) => None,
                Ok(i) => Some(i.reason.unwrap_or_else(|| "closed".into()) + if a.cut { ", in a cut" } else { "" }),
                Err(_) => Some("gone".into()),
            };
            all.push(Waited { on, why: Some(a.why().into()), met });
        }
        if let Some(at) = issue.not_before {
            let met = (self.now() >= at).then(|| "its time came".to_string());
            all.push(Waited { on: format!("not before {}", when::utc(at)), why: None, met });
        }
        Ok(all)
    }
}

impl App {
    /// Refuses `role` unless it is a directory named by its absolute path.
    pub(crate) fn role_here(&self, role: Option<&str>) -> Answer<()> {
        if let Some(role) = role {
            let path = std::path::Path::new(role);
            if !path.is_absolute() || !path.is_dir() {
                let why = format!("the role `{role}` is not a directory named by its absolute path");
                return Err(Refusal::new("no_role", why));
            }
        }
        Ok(())
    }

    /// Refuses `on` unless its role is a directory named by its absolute
    /// path and its account one stablehand keeps that a session may start
    /// on here (`stablehand account show`).
    fn on_here(&self, on: &On) -> Answer<()> {
        self.role_here(on.role.as_deref())?;
        if let Some(account) = &on.account {
            let refused =
                |why: String| Refusal::new("no_account", format!("account {account} cannot be used here: {why}"));
            let kept = self.kept_account(account).map_err(|r| refused(r.message))?;
            if let Some(why) = kept.unusable {
                return Err(refused(why));
            }
        }
        Ok(())
    }

    /// When worker `wk`, started at `st`, ended (a live one: now), and the
    /// tokens its session spent: as recorded when it ended, a live one's as
    /// its session files say now.
    fn attempt_end(&self, wk: &Worker, st: &Started) -> (i64, Option<Tokens>) {
        if wk.status == WorkerStatus::Ended { (wk.since, wk.spent) } else { (self.now(), self.spent(st)) }
    }

    /// `spent`: where the time of `checkout`'s issues went, those numbered
    /// within `numbers` (none: all of them), each attempt's from `since` on
    /// (none: from its start); an attempt that ended before `since` is left
    /// out, and an issue with no attempt left.
    pub async fn spent_of(&self, checkout: &str, numbers: Option<(u64, u64)>, since: Option<Since>) -> Answer {
        let since = since.map(|s| s.at(self.now()));
        let mut answer = json!(self.spent_in(checkout, numbers, since).await?);
        answer["checkout"] = json!(checkout);
        answer["since"] = json!(since);
        Ok(answer)
    }

    /// `spent` of no checkout: each checkout's `landings`, a row each with
    /// its `checkout`, the numbers its own `spent` gives from `since`.
    pub async fn spent_all(&self, since: Option<Since>) -> Answer {
        let since = since.map(|s| s.at(self.now()));
        let mut rows = Vec::new();
        for c in self.listed().await? {
            let mut row = json!(self.spent_in(&c.name, None, since).await?.landings);
            row["checkout"] = json!(c.name);
            rows.push(row);
        }
        Ok(json!({ "since": since, "checkouts": rows }))
    }

    async fn spent_in(&self, checkout: &str, numbers: Option<(u64, u64)>, since: Option<i64>) -> Answer<spent::Spent> {
        self.checkout(checkout).await?;
        if let Some((n, _)) = numbers.filter(|(from, to)| from == to) {
            self.issue(checkout, n).await?;
        }
        let rows = self.rows(tracker::issues(Some(checkout), true)).await?;
        let mut issues = Vec::new();
        for r in rows.iter().filter(|r| numbers.is_none_or(|(from, to)| from <= r.number && r.number <= to)) {
            let i = self.issue(checkout, r.number).await?;
            let mut attempts = Vec::new();
            for attempt in 1..=i.attempts {
                let worker = factorium_domain::worker_id(checkout, i.number, attempt);
                let wk = self.state::<Workers>(worker).await?;
                let Some(st) = &wk.started else { continue };
                let ((to, tokens), from) = (self.attempt_end(&wk, st), since.map_or(st.at, |s| s.max(st.at)));
                if to >= from {
                    attempts.push(spent::Attempt { from, to, tokens, steps: self.attempt_steps(worker, st).await? });
                }
            }
            if !attempts.is_empty() {
                issues.push(spent::Issue { number: i.number, title: i.title.clone(), attempts });
            }
        }
        Ok(spent::spent(&issues))
    }

    /// What issue `i` took: each attempt's time, from its start to its end
    /// (a live one's, to now), in ms, and the tokens its session spent, as
    /// recorded when it ended (a live one's, as its session files say now;
    /// `null`: its harness's files do not say); and their sums, `tokens`
    /// summing the attempts that say. Each attempt answers too the role it
    /// started with and the hash of what that role gave it (`null`: none);
    /// with `steps`, where its time went.
    async fn issue_spent(&self, i: &Issue, steps: bool) -> Answer<serde_json::Value> {
        let (mut ms, mut tokens, mut attempts) = (0, None::<Tokens>, Vec::new());
        for attempt in 1..=i.attempts {
            let worker = factorium_domain::worker_id(&i.checkout, i.number, attempt);
            let wk = self.state::<Workers>(worker).await?;
            let Some(st) = &wk.started else { continue };
            let (ended, (end, spent)) = (wk.status == WorkerStatus::Ended, self.attempt_end(&wk, st));
            let took = (end - st.at).max(0);
            (ms, tokens) = (ms + took, Tokens::summed(tokens, spent));
            let mut shown = json!({ "attempt": attempt, "worker": worker, "harness": st.harness,
                "account": st.account, "ended": ended, "ms": took, "tokens": spent,
                "role": st.role.as_ref().map(|r| json!({ "dir": r.dir, "from": r.from })),
                "role_hash": st.role.as_ref().map(|r| &r.read.hash) });
            if steps {
                shown["steps"] = json!(self.attempt_steps(worker, st).await?);
            }
            attempts.push(shown);
        }
        Ok(json!({ "ms": ms, "tokens": tokens, "attempts": attempts }))
    }
}
