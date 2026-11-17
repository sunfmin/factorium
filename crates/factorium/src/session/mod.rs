//! A harness session, from open to a confirmed close: the one module that
//! reaches Orca's terminals. It opens a session's terminal (the kit's pick,
//! its harness's launch line); sees what it is doing from its screen and its
//! session files (the session file is the turn); types at it only once its
//! harness is up; and closes it, confirmed against Orca's list of
//! terminals, a failed close recorded. Doctor's orphans are read from that
//! same list.

mod health;
mod launch;

use factorium_domain::tracker;
use factorium_domain::watch::{self, EndWatch, Watch, Watches, watch_id};
use factorium_domain::worker::{self as w, Status};
use factorium_harness::{Harness, Home, Stop, Turn};
use factorium_host::{Delivery, Term};
use factum::Uuid;

pub use launch::{Caller, DB_ENV};
pub(crate) use launch::{Opened, Pick, Role};

use crate::{Answer, App, Refusal};

/// How many times a question a harness opened mid-session is dismissed
/// before what is typed is given up.
const DISMISS_TRIES: u32 = 3;

/// One harness session, as it was started.
pub(crate) struct Session {
    terminal: String,
    harness: Harness,
    account: Option<String>,
    /// Its working directory, and the brief it was handed.
    cwd: String,
    brief: String,
}

impl Session {
    /// Its terminal's handle.
    pub(crate) fn terminal(&self) -> &str {
        &self.terminal
    }
}

/// What a session is doing, seen from outside.
enum Seen {
    /// Its terminal is gone.
    Gone,
    /// Orca did not answer: nothing is known.
    Unknown,
    /// Its account is used up: the line that says so.
    Quota(String),
    /// Its harness stopped: the line that says so.
    Stopped(String),
    /// Its harness is not up in its terminal (not yet, or no longer): why.
    Down(String),
    /// A turn is open: the hash of its screen.
    Open(u64),
    /// Its turn closed at this time, in ms; `None`: no session file says.
    Closed(Option<i64>),
}

impl App {
    /// Where a session of `harness` on `account` keeps its files.
    pub(crate) fn session_home(&self, harness: Harness, account: Option<&str>) -> Home {
        harness.home(&self.host.home(), account, &self.dir(&["kept", harness.key()]))
    }

    /// A worker's session.
    pub(crate) fn worker_session(st: &w::Started) -> Answer<Session> {
        let harness = Harness::read(&st.harness).ok_or_else(|| Refusal::new("no_harness", st.harness.clone()))?;
        Ok(Session {
            terminal: st.terminal.clone(),
            harness,
            account: st.account.clone(),
            cwd: st.worktree.clone(),
            brief: st.brief.clone(),
        })
    }

    /// The watch's session, if one was started.
    pub(crate) fn watch_session(&self, watch: &Watch) -> Option<Session> {
        let (home, brief) = self.watch_home();
        Some(Session {
            terminal: watch.terminal.clone()?,
            harness: watch.harness.as_deref().and_then(Harness::read)?,
            account: watch.account.clone(),
            cwd: home.to_string_lossy().into_owned(),
            brief: brief.to_string_lossy().into_owned(),
        })
    }

    /// Whether a session's terminal is gone (its program ended, or Orca no
    /// longer knows it). `Err`: Orca gave no answer to tell, and why.
    pub(crate) fn gone(&self, s: &Session) -> Result<bool, String> {
        match self.host.orca().screen(&s.terminal) {
            Term::Running(_) => Ok(false),
            Term::Exited => Ok(true),
            Term::Unreadable(why) => Err(why),
        }
    }

    /// Waits, up to a minute, for the program a session's terminal was
    /// opened with to be up and its screen to settle.
    fn settle(&self, terminal: &str) {
        let _ = self.host.orca().ready(terminal);
    }

    fn see(&self, s: &Session) -> Seen {
        let screen = match self.host.orca().screen(&s.terminal) {
            Term::Running(screen) => screen,
            Term::Exited => return Seen::Gone,
            Term::Unreadable(_) => return Seen::Unknown,
        };
        match s.harness.stopped(&screen) {
            Some(Stop::Quota(said)) => return Seen::Quota(said),
            Some(Stop::Refused(said)) => return Seen::Stopped(said),
            None => {}
        }
        if let Some(why) = s.harness.not_up(&screen) {
            return Seen::Down(why);
        }
        match self.turn(s) {
            Some(Turn::Open { .. }) => Seen::Open(hash(&screen)),
            Some(Turn::Closed { since, .. }) => Seen::Closed(Some(since as i64 * 1000)),
            None => Seen::Closed(None),
        }
    }

    /// Types `text` and Enter at a session, only once its harness's own
    /// screen is up there: typed at a shell, a line is a command it runs.
    pub(crate) fn type_at(&self, s: &Session, text: &str) -> Answer<()> {
        self.up(s)?;
        self.host.orca().send(&s.terminal, text).map_err(crate::fail("orca"))
    }

    /// Types `text` and Enter at a session whose harness is up, and answers
    /// whether it started a turn. `now`: first stops the running turn with
    /// the harness's own key, so a session stuck in a call reads it at once.
    /// Orca misses a turn that starts right after an interrupt or ends
    /// within a second; the session's own file, where its harness keeps
    /// one, says whether the text was taken as a prompt.
    pub(crate) fn deliver_at(&self, s: &Session, text: &str, now: bool) -> Answer<Delivery> {
        self.up(s)?;
        let since = (self.now() / 1000).saturating_sub(1) as u64;
        let key = now.then(|| s.harness.interrupt());
        let seen = self.host.orca().deliver(&s.terminal, text, key).map_err(crate::fail("orca"))?;
        let home = self.session_home(s.harness, s.account.as_deref());
        let took = || s.harness.prompted(&home, &s.cwd, &s.brief, text, since) == Some(true);
        Ok(if seen == Delivery::Queued && took() { Delivery::Submitted } else { seen })
    }

    /// Types `text` at a worker's session that is idle at its prompt, and
    /// answers whether it reached it: it started a turn, or waits behind the
    /// one running. `Err`: why it did not.
    pub(crate) fn reach(&self, s: &Session, text: &str) -> Result<(), String> {
        let open = || matches!(self.turn(s), Some(Turn::Open { .. }));
        match self.deliver_at(s, text, false) {
            Ok(Delivery::Submitted) => Ok(()),
            Ok(Delivery::Queued) if open() => Ok(()),
            Ok(Delivery::Queued) => {
                let shows = match self.host.orca().screen(&s.terminal) {
                    Term::Running(screen) => {
                        screen.iter().rev().take(3).rev().map(|r| r.trim()).collect::<Vec<_>>().join(" / ")
                    }
                    _ => "unreadable".into(),
                };
                Err(format!("typed, it started no turn and none runs; its screen ends: {shows}"))
            }
            Err(r) => Err(r.message),
        }
    }

    /// The turn a session's files say it is in.
    fn turn(&self, s: &Session) -> Option<Turn> {
        s.harness.turn(&self.session_home(s.harness, s.account.as_deref()), &s.cwd, &s.brief)
    }

    /// Refuses unless a session's harness is up in its terminal, ready to be
    /// typed at: typed at a shell, a line is a command it runs. A question
    /// its harness opened on its own mid-session is dismissed with its own
    /// key first: what is typed there never reaches the prompt.
    fn up(&self, s: &Session) -> Answer<()> {
        let mut dismissed = 0;
        loop {
            let screen = match self.host.orca().screen(&s.terminal) {
                Term::Running(screen) => screen,
                Term::Exited => return Err(Refusal::new("gone", "its session's terminal is gone")),
                Term::Unreadable(why) => return Err(Refusal::new("orca", why)),
            };
            if let Some(why) = s.harness.not_up(&screen) {
                return Err(Refusal::new("not_up", format!("its harness is not up: {why}")));
            }
            let Some((asks, key)) = s.harness.asking(&screen) else { return Ok(()) };
            if dismissed == DISMISS_TRIES {
                return Err(Refusal::new("not_up", format!("its harness asks and stays asking: {asks}")));
            }
            self.host.orca().press(&s.terminal, key).map_err(crate::fail("orca"))?;
            dismissed += 1;
        }
    }

    /// Opens a terminal in `worktree`, titled `title`, running `line`;
    /// answers its handle.
    fn open_terminal(&self, worktree: &std::path::Path, title: &str, line: &str) -> Result<String, String> {
        self.host.orca().open(worktree, title, line)
    }

    /// Orca's terminals now, as (handle, title): what confirms a close, and
    /// what doctor's orphans are read from.
    fn terminals(&self) -> Result<Vec<(String, String)>, String> {
        self.host.orca().terminals()
    }

    /// Closes a terminal, confirmed: Orca no longer lists it, whatever its
    /// close answered (a terminal Orca forgot is closed). `Err`: why it is
    /// not, or not known to be.
    fn close_terminal(&self, terminal: &str) -> Result<(), String> {
        let refused = self.host.orca().close(terminal).err();
        let still = match self.terminals() {
            Ok(listed) if !listed.iter().any(|(h, _)| h == terminal) => return Ok(()),
            Ok(_) => "Orca lists it still after its close".to_string(),
            Err(why) => format!("Orca's terminals could not be listed to confirm its close: {why}"),
        };
        Err(refused.map_or(still.clone(), |r| format!("Orca refused to close it ({r}); {still}")))
    }

    /// Closes a worker's terminal and records how that went. Run before the
    /// worker is recorded ended: Orca lists a terminal until its close
    /// returns (seconds, for an agent's), and an ended worker's listed
    /// terminal is an orphan, which the watch would close in its stead.
    pub(crate) async fn close_worker_session(&self, worker: Uuid, st: &w::Started) -> Answer<()> {
        let failed = self.close_terminal(&st.terminal).err();
        match self.submit::<w::NoteClosed>(w::Closed { worker, failed }).await {
            // An ended worker's record stays as it was; Orca's list shows the rest.
            Err(r) if r.code == "ended" => Ok(()),
            other => other,
        }
    }

    /// Closes the watch's session's terminal, if it has one, and records it
    /// has no session; answers that terminal.
    pub(crate) async fn close_watch_session(&self) -> Answer<Option<String>> {
        let watch = self.state::<Watches>(watch_id()).await?;
        let Some(t) = &watch.terminal else { return Ok(None) };
        let failed = self.close_terminal(t).err();
        self.submit::<EndWatch>(watch::Ended { failed }).await?;
        Ok(Some(t.clone()))
    }

    /// Orca's terminals this database's factorium opened (a worker's, ended
    /// or not, or a watch session's) that neither a live worker nor the
    /// watch's session now holds, as (handle, title); and, apart from them,
    /// those whose close failed that Orca lists still, as (handle, title,
    /// why). A terminal another daemon, on another database, opened is
    /// never this one's, whatever its title. Orca unreadable: no orphans
    /// known, and every failed close as recorded.
    pub(crate) async fn orphans(&self, watch: &Watch) -> Answer<Orphans> {
        let (mut opened, mut held) = (watch.opened.clone(), Vec::new());
        let mut unclosed: Vec<(String, String, String)> =
            watch.unclosed.iter().map(|(t, why)| (t.clone(), launch::WATCH_TITLE.into(), why.clone())).collect();
        for r in self.rows(tracker::workers(None, true)).await? {
            if r.state.status != Status::Ended {
                held.push(r.terminal.clone());
            }
            if let Some(why) = r.close_failed {
                unclosed.push((r.terminal.clone(), format!("{}#{}", r.checkout, r.number), why));
            }
            opened.push(r.terminal);
        }
        held.extend(watch.terminal.clone());
        let Ok(listed) = self.terminals() else { return Ok((Vec::new(), unclosed)) };
        unclosed.retain(|(h, _, _)| listed.iter().any(|(l, _)| l == h) && !held.contains(h));
        let orphans = listed
            .into_iter()
            .filter(|(h, _)| opened.contains(h) && !held.contains(h) && !unclosed.iter().any(|(u, _, _)| u == h))
            .collect();
        Ok((orphans, unclosed))
    }
}

/// Doctor's orphans, (handle, title), and the terminals whose close failed,
/// (handle, title, why).
pub(crate) type Orphans = (Vec<(String, String)>, Vec<(String, String, String)>);

/// A screen's hash: whether it changed between two looks.
fn hash(screen: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    screen.hash(&mut h);
    h.finish()
}
