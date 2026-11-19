//! The binary this process runs, and the one Homebrew links now, which the
//! daemon (between ticks) and a waiter refused as stale exec in its place,
//! once that binary is proven to open this host's database (factorium#141).
use std::path::{Path, PathBuf};
use uuid::Uuid;

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
    /// than the one running (a cut, or by hand), and that version opens this
    /// host's database; mid-upgrade, it resolves to nothing.
    pub(crate) fn swapped(&self) -> Option<PathBuf> {
        let (me, link) = self.memory.lock().unwrap().running.clone()?;
        let to = link.canonicalize().ok().filter(|to| *to != me)?;
        let proven = self.proofs.of(&to).unwrap_or_else(|| {
            let proven = self.proofs.prove(&self.db, &to, &self.data().proof(Uuid::now_v7()));
            proven.inspect_err(|why| eprintln!("not swapping to {}: {why}", to.display()))
        });
        proven.ok().map(|()| link)
    }

    /// Whether the binary `bin` opens this host's database, proven once per
    /// binary (as [`crate::App::swapped`] asks), off the runtime's threads.
    pub(crate) async fn proves(&self, bin: &Path) -> Result<(), String> {
        let bin = bin.canonicalize().map_err(|e| format!("cannot find {}: {e}", bin.display()))?;
        let (proofs, db, copy) = (self.proofs.clone(), self.db.clone(), self.data().proof(Uuid::now_v7()));
        let proven =
            tokio::task::spawn_blocking(move || proofs.of(&bin).unwrap_or_else(|| proofs.prove(&db, &bin, &copy)));
        proven.await.unwrap_or_else(|e| Err(format!("proving panicked: {e}")))
    }
}
