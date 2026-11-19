//! The binary this process runs, and the one Homebrew links now, which the
//! daemon (between ticks) and a waiter refused as stale exec in its place.
use std::path::{Path, PathBuf};

impl crate::App {
    /// This process runs the binary `me` (canonical): if Homebrew installed
    /// it, [`crate::App::swapped`] asks whether its link names another version.
    pub fn runs_as(&self, me: &Path) {
        let link = self.host.brew().link_of(me);
        self.memory.lock().unwrap().running = link.map(|l| (me.to_path_buf(), l));
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
    /// than the one running (a cut, or by hand); mid-upgrade, it resolves to nothing.
    pub(crate) fn swapped(&self) -> Option<PathBuf> {
        let (me, link) = self.memory.lock().unwrap().running.clone()?;
        (link.canonicalize().ok()? != me).then_some(link)
    }
}
