//! A host that cuts no version of the daemon's own formula still keeps it
//! current (factorium#269): each `upgrade_every`, while no line move runs,
//! the daemon upgrades its formula from the tap, beside the ticks. The
//! daemon swaps to what that links by the rule every link is swapped to by
//! (§4): once it proves on a copy of this host's database.

use factorium_domain::checkout::Checkout;
use factorium_domain::daemon::{self, NoteUpgrade, Upgraded};
use factorium_domain::settings::{Settings, Time};
use serde_json::{Value, json};

use crate::{Answer, App};

impl App {
    /// The tick's upgrade from the tap: one that ended is recorded, and
    /// answers what it did; else one due is started beside the ticks.
    pub(crate) async fn upgrade_from_tap(&self, checkouts: &[Checkout], settings: &Settings) -> Answer<Option<Value>> {
        let ended = {
            let mut m = self.memory.lock().unwrap();
            match m.upgrading.as_ref().map(tokio::task::JoinHandle::is_finished) {
                Some(true) => m.upgrading.take(),
                Some(false) => return Ok(None),
                None => None,
            }
        };
        if let Some(ended) = ended {
            let said = ended.await.unwrap_or_else(|e| Err(format!("brew panicked: {e}")));
            let failed = said.err();
            self.submit::<NoteUpgrade>(Upgraded { failed: failed.clone() }).await?;
            return Ok(Some(json!({ "upgraded": failed.is_none(), "failed": failed })));
        }
        let Some(formula) = self.own_formula() else { return Ok(None) };
        let last = self.state::<daemon::Daemons>(daemon::daemon_id()).await?.upgrade;
        if self.moves_run()
            || !daemon::upgrade_due(last.as_ref(), settings.ms(Time::UpgradeEvery), self.now())
            || self.cuts(checkouts, &formula)
        {
            return Ok(None);
        }
        let host = self.host.clone();
        let f = formula.clone();
        let up = tokio::task::spawn_blocking(move || host.brew().upgrade_newest(&f));
        self.memory.lock().unwrap().upgrading = Some(up);
        Ok(Some(json!({ "upgrading": formula })))
    }

    /// The formula Homebrew installed this daemon's binary from, as its link
    /// names it (`<prefix>/opt/<formula>/bin/<name>`), if it did.
    fn own_formula(&self) -> Option<String> {
        let link = self.linked()?;
        Some(link.parent()?.parent()?.file_name()?.to_string_lossy().into_owned())
    }

    /// Whether a checkout here cuts `formula`: its base, as last fetched,
    /// declares a `[cut]` naming it (`tap/name` or `name`). That host
    /// installs it at the cut (§1.14).
    fn cuts(&self, checkouts: &[Checkout], formula: &str) -> bool {
        checkouts.iter().any(|c| {
            let git = self.host.git(&c.path);
            let declared = git.sha(&c.fetched_base()).ok().and_then(|tip| self.declared(&git, &tip).ok());
            let cut = declared.and_then(|d| d.cut).and_then(|cut| cut.formula);
            cut.is_some_and(|f| f.rsplit('/').next() == Some(formula))
        })
    }
}
