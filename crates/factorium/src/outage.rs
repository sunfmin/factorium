//! A store the daemon cannot read: the watch is reached outside
//! it. Each look that reads the store through keeps, in the data directory,
//! where the watch's session is; a look that fails on the store briefs that
//! session directly, as [`Outage`] decides, and keeps the outage there too.
//! Doctor that cannot read the store says so as its one finding.

use factorium_domain::outage::{self, Outage, Reach};
use factorium_domain::watch::Watch;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::{Answer, App, Refusal};

impl App {
    /// After a look that read the store through: where the watch's session
    /// is reached, and no outage.
    pub(crate) fn keep_reach(&self, watch: &Watch) {
        let reach = watch.terminal.clone().map(|terminal| Reach {
            terminal,
            harness: watch.harness.clone().unwrap_or_default(),
            account: watch.account.clone(),
        });
        let data = self.data();
        keep(&data.watch_reach(), reach.as_ref());
        keep::<Outage>(&data.outage(), None);
    }

    /// A look failed with `r`. Failing on the store, the watch's session is
    /// briefed directly, when the outage is due; a brief that is not typed
    /// is tried on the next failing look.
    pub(crate) fn look_failed(&self, r: &Refusal) {
        if r.code != "store" {
            return;
        }
        let (data, now) = (self.data(), self.now());
        let mut o = Outage::failed(read(&data.outage()), &r.message, now);
        if o.due(now) {
            match self.brief_watch(&o) {
                Ok(()) => o.briefed = Some(now),
                Err(why) => eprintln!("outage: the watch was not briefed: {why}"),
            }
        }
        keep(&data.outage(), Some(&o));
    }

    /// Types the outage's brief at the watch's session, if one is known and
    /// its harness is up.
    fn brief_watch(&self, o: &Outage) -> Result<(), String> {
        let reach: Reach = read(&self.data().watch_reach()).ok_or("no watch session is known")?;
        let watch = Watch {
            terminal: Some(reach.terminal),
            harness: Some(reach.harness),
            account: reach.account,
            ..Watch::default()
        };
        let s = self.watch_session(&watch).ok_or("the watch's harness is not known")?;
        self.up(&s).map_err(|r| r.to_string())?;
        let text = o.brief(env!("CARGO_PKG_VERSION"), &self.data().root().display().to_string());
        self.host.orca().deliver(s.terminal(), &text, None).map(drop)
    }

    /// `factorium doctor`: what [`App::doctor_view`] answers, or, when the
    /// store cannot be read, that as its one finding (`unreadable`).
    pub async fn doctor(&self) -> Answer {
        match self.doctor_view().await {
            Err(r) if r.code == "store" => {
                let found = outage::finding(&r.message, &self.data().root().display().to_string());
                Ok(json!({ "findings": [found], "unreadable": r.to_string() }))
            }
            other => other,
        }
    }
}

fn read<T: DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Writes `what` at `path`, or removes the file for `None`. Kept as best
/// it can be: a failed write is said in the log, and the store goes on.
fn keep<T: Serialize>(path: &std::path::Path, what: Option<&T>) {
    let done = match what {
        Some(w) => std::fs::write(path, serde_json::to_vec(w).unwrap_or_default()),
        None => std::fs::remove_file(path)
            .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }),
    };
    if let Err(e) = done {
        eprintln!("outage: {}: {e}", path.display());
    }
}
