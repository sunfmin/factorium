//! The binary this process runs, and the one Homebrew links now, which the
//! daemon (between ticks) and a waiter refused as stale exec in its place,
//! once that binary is proven to open this host's database (factorium#141).
use std::path::{Path, PathBuf};
use std::time::Duration;

use factorium_host::{Priority, shell};
use uuid::Uuid;

/// How long a binary has to answer `doctor` on a copy of the database.
const PROOF_LIMIT: Duration = Duration::from_secs(120);

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
        let to = link.canonicalize().ok()?;
        if to == me {
            return None;
        }
        let known = self.memory.lock().unwrap().proven.get(&to).cloned();
        let proven =
            known.unwrap_or_else(|| self.remember(&to, prove(&self.db, &to, &self.data().proof(Uuid::now_v7()))));
        match proven {
            Ok(()) => Some(link),
            Err(why) => {
                eprintln!("factorium: not swapping to {}: {why}", to.display());
                None
            }
        }
    }

    /// Whether the binary `bin` opens this host's database, proven once per
    /// binary, off the runtime's threads: as [`crate::App::swapped`] asks.
    pub(crate) async fn proves(&self, bin: &Path) -> Result<(), String> {
        let bin = bin.canonicalize().map_err(|e| format!("cannot find {}: {e}", bin.display()))?;
        if let Some(known) = self.memory.lock().unwrap().proven.get(&bin).cloned() {
            return known;
        }
        let (db, to, copy) = (self.db.clone(), bin.clone(), self.data().proof(Uuid::now_v7()));
        let proven = tokio::task::spawn_blocking(move || prove(&db, &to, &copy)).await;
        self.remember(&bin, proven.unwrap_or_else(|e| Err(format!("proving panicked: {e}"))))
    }

    fn remember(&self, bin: &Path, proven: Result<(), String>) -> Result<(), String> {
        self.memory.lock().unwrap().proven.insert(bin.to_path_buf(), proven.clone());
        proven
    }
}

/// Proves `bin` on a copy of the database `db` made at `copy` by SQLite's
/// backup (never the live file): it answers `doctor` there. The copy is
/// removed once proven, and kept, named in why not, where it fails.
fn prove(db: &Path, bin: &Path, copy: &Path) -> Result<(), String> {
    let dir = copy.parent().unwrap_or(Path::new("."));
    let copied = std::fs::create_dir_all(dir).map_err(|e| e.to_string()).and_then(|()| {
        let live = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY);
        live.and_then(|l| l.backup(rusqlite::MAIN_DB, copy, None::<fn(_)>)).map_err(|e| e.to_string())
    });
    if let Err(e) = copied {
        return Err(format!("cannot copy {} to {}: {e}", db.display(), copy.display()));
    }
    let q = |p: &Path| factorium_harness::quoted(&p.to_string_lossy());
    let line = format!("{} --db {} doctor", q(bin), q(copy));
    let ran = shell::run(dir, &line, &[], PROOF_LIMIT, Priority::Normal, &|_| {}, &|| false);
    if ran.ok {
        let _ = std::fs::remove_dir_all(dir);
        return Ok(());
    }
    let how = if ran.timed_out { format!("ran past {}s", PROOF_LIMIT.as_secs()) } else { "failed".into() };
    Err(format!(
        "{} cannot open this host's database: doctor on its copy {} {how}:\n{}",
        bin.display(),
        copy.display(),
        ran.tail
    ))
}
