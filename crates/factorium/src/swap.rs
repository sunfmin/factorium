//! The binary this process runs, and the one Homebrew links now, which the
//! daemon (between ticks) and a waiter refused as stale exec in its place;
//! the binary kept to go back to, and the rollback to it when a new
//! binary's ticks all fail (factorium#143).
use std::path::{Path, PathBuf};

use factorium_domain::daemon::{NoteRollback, Rollback, RolledBack};
use factorium_domain::settings::{Setting, Settings};
use serde::{Deserialize, Serialize};

/// What a binary whose ticks all failed hands the kept one it execs: the
/// rollback to record once that one ticks well, the binary not to swap
/// to again, and the link to watch for a newer one.
#[derive(Serialize, Deserialize)]
struct Handoff {
    rolled_back: RolledBack,
    refused: PathBuf,
    link: PathBuf,
}

/// The version of a binary placed as `<version>/bin/<name>`: in Homebrew's
/// keg, or kept.
fn version_of(binary: &Path) -> String {
    let v = binary.parent().and_then(Path::parent).and_then(Path::file_name);
    v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default()
}

impl crate::App {
    /// This process runs the binary `me` (canonical): if Homebrew installed
    /// it, [`crate::App::swapped`] asks whether its link names another
    /// version; if it is the kept binary a rollback execs, the link is the
    /// one the rollback handed it.
    pub fn runs_as(&self, me: &Path) {
        let handoff = self.is_kept(me).then(|| self.handoff()).flatten();
        let link = self.host.brew().link_of(me).or(handoff.as_ref().map(|h| h.link.clone()));
        let mut m = self.memory.lock().unwrap();
        m.running = link.map(|l| (me.to_path_buf(), l));
        m.refused = handoff.map(|h| h.refused);
    }

    /// The Homebrew link to this process's own binary, if Homebrew installed it.
    pub(crate) fn linked(&self) -> Option<PathBuf> {
        let unknown = self.memory.lock().unwrap().running.is_none();
        if let Some(me) = unknown.then(|| std::env::current_exe().and_then(|e| e.canonicalize()).ok()).flatten() {
            self.runs_as(&me);
        }
        self.memory.lock().unwrap().running.clone().map(|(_, link)| link)
    }

    /// The link to exec in place of this binary: Homebrew links a version other
    /// than the one running (a cut, or by hand) and than the one a rollback
    /// went back from; mid-upgrade, it resolves to nothing.
    pub(crate) fn swapped(&self) -> Option<PathBuf> {
        let m = self.memory.lock().unwrap();
        let (me, link) = m.running.clone()?;
        let now = link.canonicalize().ok()?;
        (now != me && Some(&now) != m.refused.as_ref()).then_some(link)
    }

    /// The kept binary to exec in place of this one, once its ticks all failed.
    pub fn fallback(&self) -> Option<PathBuf> {
        self.memory.lock().unwrap().fallback.clone()
    }

    /// A tick's outcome, for the binary that ran it (one Homebrew installed,
    /// or kept). Its first good tick keeps it to go back to, and records the
    /// rollback that brought it here, or ends the last one. Its failing ticks
    /// before any went well, `tick_failures_max` of them, go back to the kept
    /// binary: read from no database, which this binary may not read.
    pub(crate) async fn proving(&self, error: Option<&str>) {
        let Some((me, link)) = self.memory.lock().unwrap().running.clone() else { return };
        let failed = {
            let mut m = self.memory.lock().unwrap();
            if m.proven {
                return;
            }
            m.proven = error.is_none();
            m.failed += u64::from(!m.proven);
            m.failed
        };
        let Some(error) = error else {
            let _ = self.keep(&me).inspect_err(|e| eprintln!("keep {}: {e}", me.display()));
            // The kept binary runs only as a rollback execs it, handed the rollback to record.
            let rolled_back = self.is_kept(&me).then(|| self.handoff().map(|h| h.rolled_back)).flatten();
            let back = rolled_back.is_some();
            let recorded = self.submit::<NoteRollback>(Rollback { rolled_back }).await;
            if recorded.inspect_err(|e| eprintln!("rollback: {e}")).is_ok() && back {
                let _ = std::fs::remove_file(self.data().rollback());
            }
            return;
        };
        let max = self.settings().await.unwrap_or_else(|_| Settings::default()).get(Setting::TickFailuresMax);
        let Some(kept) = self.kept_binary().filter(|k| failed >= max && version_of(k) != version_of(&me)) else {
            return;
        };
        let rolled_back = RolledBack { from: version_of(&me), to: version_of(&kept), error: error.into() };
        let handoff = Handoff { rolled_back, refused: me, link };
        let wrote = std::fs::write(self.data().rollback(), serde_json::to_vec(&handoff).unwrap_or_default());
        if wrote.inspect_err(|e| eprintln!("rollback: {e}")).is_ok() {
            self.memory.lock().unwrap().fallback = Some(kept);
        }
    }

    /// Keeps a copy of `me` to go back to, in place of the one kept before.
    fn keep(&self, me: &Path) -> std::io::Result<()> {
        let dir = self.data().binary();
        let to = dir.join(version_of(me)).join("bin").join(me.file_name().unwrap_or_default());
        if self.is_kept(me) || to.exists() {
            return Ok(());
        }
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(to.parent().unwrap_or(&dir))?;
        let part = to.with_extension("part");
        std::fs::copy(me, &part)?;
        std::fs::rename(part, to)
    }

    /// The kept binary, if one is.
    fn kept_binary(&self) -> Option<PathBuf> {
        let version = std::fs::read_dir(self.data().binary()).ok()?.flatten().next()?.path().join("bin");
        let binary = std::fs::read_dir(version).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_none());
        binary.and_then(|b| b.canonicalize().ok())
    }

    fn is_kept(&self, me: &Path) -> bool {
        self.data().binary().canonicalize().is_ok_and(|d| me.starts_with(d))
    }

    fn handoff(&self) -> Option<Handoff> {
        serde_json::from_slice(&std::fs::read(self.data().rollback()).ok()?).ok()
    }
}
