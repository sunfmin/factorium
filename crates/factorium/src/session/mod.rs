//! A harness session, from open to a confirmed close: the one module that
//! reaches Orca's terminals. It opens a session's terminal (the kit's pick,
//! its harness's launch line); sees what it is doing from its screen and its
//! session files (the session file is the turn); passes it messages through
//! its outbox ([`outbox`]), typed only once its harness is up; and closes it, confirmed against Orca's list of
//! terminals, a failed close recorded. Doctor's orphans are read from that
//! same list.

mod health;
mod launch;
mod outbox;

use factorium_domain::doctor::{Terminal, Unclosed};
use factorium_domain::health::Sight;
use factorium_domain::tracker;
use factorium_domain::watch::{self, EndWatch, Watch, Watches, watch_id};
use factorium_domain::worker::{self as w, Status};
use factorium_harness::{Harness, Home, Stop, Turn};
use factorium_host::Term;
use factum::Uuid;

pub use launch::{Caller, DB_ENV};
pub(crate) use launch::{Opened, Pick, Role, program_dir};
pub(crate) use outbox::heard;

use crate::{Answer, App, Refusal};

/// How many times a question a harness opened mid-session is dismissed
/// before what is typed is given up.
const DISMISS_TRIES: u32 = 3;

/// One harness session, as it was started.
pub(crate) struct Session {
    terminal: String,
    /// The worker it is, if it is a worker's.
    worker: Option<Uuid>,
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

impl App {
    /// Where a session of `harness` on `account` keeps its files.
    pub(crate) fn session_home(&self, harness: Harness, account: Option<&str>) -> Home {
        harness.home(&self.host.home(), account, &self.data().kept(harness.key()))
    }

    /// Worker `worker`'s session.
    pub(crate) fn worker_session(worker: Uuid, st: &w::Started) -> Answer<Session> {
        let harness = Harness::read(&st.harness).ok_or_else(|| Refusal::new("no_harness", st.harness.clone()))?;
        Ok(Session {
            terminal: st.terminal.clone(),
            worker: Some(worker),
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
            worker: None,
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

    /// What a session is doing, seen from outside; `None`: Orca did not
    /// answer, so nothing is known.
    fn see(&self, s: &Session) -> Option<Sight> {
        let screen = match self.host.orca().screen(&s.terminal) {
            Term::Running(screen) => screen,
            Term::Exited => return Some(Sight::Gone),
            Term::Unreadable(_) => return None,
        };
        match s.harness.stopped(&screen) {
            Some(Stop::Quota(said)) => return Some(Sight::Quota(said)),
            Some(Stop::Refused(said)) => return Some(Sight::Stopped(said)),
            None => {}
        }
        if let Some(why) = s.harness.not_up(&screen) {
            return Some(Sight::Down(why));
        }
        Some(match self.turn(s) {
            Some(Turn::Open { .. }) => Sight::Open(hash(&screen)),
            Some(Turn::Closed { since, .. }) => Sight::Closed(Some(since as i64 * 1000)),
            None => Sight::Closed(None),
        })
    }

    /// The turn a session's files say it is in.
    fn turn(&self, s: &Session) -> Option<Turn> {
        s.harness.turn(&self.session_home(s.harness, s.account.as_deref()), &s.cwd, &s.brief)
    }

    /// Refuses unless a session's harness is up in its terminal, ready to be
    /// typed at: typed at a shell, a line is a command it runs. A question
    /// its harness opened on its own mid-session is dismissed with its own
    /// key first: what is typed there never reaches the prompt.
    pub(crate) fn up(&self, s: &Session) -> Answer<()> {
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

    /// Closes a worker's terminal and records how that went; answers why it
    /// is not closed, if it is not. Run before the worker is recorded
    /// ended ([`App::finish`]).
    pub(crate) async fn close_worker_session(&self, worker: Uuid, st: &w::Started) -> Answer<Option<String>> {
        let failed = self.close_terminal(&st.terminal).err();
        match self.submit::<w::NoteClosed>(w::Closed { worker, failed: failed.clone() }).await {
            // An ended worker's record stays as it was, and one never
            // recorded started has none: Orca's list shows the rest.
            Err(r) if r.code == "ended" || r.code == "no_worker" => Ok(failed),
            other => other.map(|()| failed),
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
        let unclose =
            |handle: &str, title: String, why: &str| Unclosed { handle: handle.into(), title, why: why.into() };
        let mut unclosed: Vec<Unclosed> =
            watch.unclosed.iter().map(|(t, why)| unclose(t, launch::WATCH_TITLE.into(), why)).collect();
        for r in self.rows(tracker::workers(None, true)).await? {
            if r.state.status != Status::Ended {
                held.push(r.terminal.clone());
            }
            if let Some(why) = r.close_failed {
                unclosed.push(unclose(&r.terminal, format!("{}#{}", r.checkout, r.number), &why));
            }
            opened.push(r.terminal);
        }
        held.extend(watch.terminal.clone());
        let Ok(listed) = self.terminals() else { return Ok((Vec::new(), unclosed)) };
        unclosed.retain(|u| listed.iter().any(|(l, _)| *l == u.handle) && !held.contains(&u.handle));
        let orphans = listed
            .into_iter()
            .filter(|(h, _)| opened.contains(h) && !held.contains(h) && !unclosed.iter().any(|u| u.handle == *h))
            .map(|(handle, title)| Terminal { handle, title })
            .collect();
        Ok((orphans, unclosed))
    }
}

/// Doctor's orphans and the terminals whose close failed.
pub(crate) type Orphans = (Vec<Terminal>, Vec<Unclosed>);

/// A screen's hash: whether it changed between two looks.
fn hash(screen: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    screen.hash(&mut h);
    h.finish()
}
