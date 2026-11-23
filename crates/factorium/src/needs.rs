//! A role directory checked before a worker starts with it (factorium#277):
//! what rolesmith says it needs of this host is there, a program it lacks
//! installed first. What is still lacking is kept on the daemon for doctor,
//! so the watch is handed it, and the role's issues wait.

use std::collections::BTreeMap;
use std::path::Path;

use factorium_domain::daemon::{Daemons, NoteRole, RoleChecked, daemon_id};
use factorium_domain::needs::{self, Lacking};

use crate::{Answer, App};

/// What each role directory checked in one tick was found to be: checked
/// once a tick, however many of its issues are ready.
pub(crate) type Checked = BTreeMap<String, bool>;

impl App {
    /// Whether a worker may start with role `dir` now, checked once a tick
    /// (`checked`). A directory without `rolesmith.toml` needs nothing.
    pub(crate) async fn role_ready(&self, dir: &str, checked: &mut Checked) -> Answer<bool> {
        if let Some(ready) = checked.get(dir) {
            return Ok(*ready);
        }
        let ready = !Path::new(dir).join(needs::RECORD).is_file() || self.check_role(dir).await?;
        checked.insert(dir.into(), ready);
        Ok(ready)
    }

    /// `rolesmith doctor --project <dir>`; a program it lacks is installed
    /// (`rolesmith tool install --project <dir>`), unless an install
    /// already left the same lack, and it is checked again. What it lacks
    /// after is kept for doctor; nothing lacking clears what was.
    async fn check_role(&self, dir: &str) -> Answer<bool> {
        let before = self.state::<Daemons>(daemon_id()).await?.roles.remove(dir);
        let mut lacking = self.role_doctor(dir);
        if lacking.as_ref().is_some_and(|l| l.to_install(before.as_ref())) {
            let said = self.host.programs().run("rolesmith", &["tool", "install", "--project", dir, "--json"]);
            let answer: serde_json::Value = serde_json::from_str(said.out.trim()).unwrap_or_default();
            let failed = (!said.ok).then(|| needs::refusal(&answer["error"], &said.err));
            lacking = self.role_doctor(dir).map(|after| match failed {
                Some(why) => Lacking { failed: Some(format!("`rolesmith tool install` failed: {why}")), ..after },
                None => after,
            });
        } else if let (Some(now), Some(b)) = (lacking.as_mut(), before.as_ref()) {
            // Not installed again: why the last install failed still stands.
            if now.lacks == b.lacks && now.failed.is_none() {
                now.failed.clone_from(&b.failed);
            }
        }
        let ready = lacking.is_none();
        self.submit::<NoteRole>(RoleChecked { dir: dir.into(), lacking }).await?;
        Ok(ready)
    }

    /// What `rolesmith doctor --project <dir>` says `dir` lacks.
    fn role_doctor(&self, dir: &str) -> Option<Lacking> {
        let said = self.host.programs().run("rolesmith", &["doctor", "--project", dir, "--json"]);
        needs::read_doctor(said.ok, &said.out, &said.err)
    }
}
