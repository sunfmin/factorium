//! Proof that a factorium binary opens a database before it runs on it: it
//! answers `doctor` on a copy, never the live file (factorium#141). The copy
//! is SQLite's `VACUUM INTO`, one read transaction that a writer does not
//! restart; an online backup restarted on every write and never finished
//! on a busy host (factorium#241). Each binary is
//! proven once on the host: the processes that ask take turns on its lock,
//! and how it was proven is kept beside its copy for the next to read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::shell::{Priority, run};

/// How long a binary has to answer `doctor` on the copy.
const LIMIT: Duration = Duration::from_secs(120);

/// How a binary was proven: it opens the database, or why not.
pub type Proven = Result<(), String>;

/// Each binary (canonical) proven, or being proven on a thread of its own.
#[derive(Default)]
pub struct Proofs(Mutex<BTreeMap<PathBuf, Option<Proven>>>);

/// How a proof ended: `kept` when it says how this binary opens this
/// database whoever asks next; not when it says only how this try went (the
/// copy failed, or doctor ran past its limit).
struct Ended {
    proven: Proven,
    kept: bool,
}

/// How a binary was proven, as kept on the host.
#[derive(Serialize, Deserialize)]
struct Kept {
    why_not: Option<String>,
}

impl Proofs {
    /// How `bin` was proven, if it was.
    pub fn of(&self, bin: &Path) -> Option<Proven> {
        self.0.lock().unwrap().get(bin).cloned().flatten()
    }

    /// Proves `bin` on a copy of `db` under `dir`, unless it was proven on
    /// this host: then answers how. The copy is removed once proven, and
    /// kept, named in why not, where it fails.
    pub fn prove(&self, db: &Path, bin: &Path, dir: &Path) -> Proven {
        if let Some(proven) = self.of(bin) {
            return proven;
        }
        let proven = on_host(db, bin, dir);
        self.0.lock().unwrap().insert(bin.to_path_buf(), Some(proven.clone()));
        proven
    }

    /// How `bin` was proven, here or kept on the host, if it was; else its
    /// proof is started on a thread of its own, once, and a later ask
    /// answers how it ended: so none who asks waits on it.
    pub fn started(self: &Arc<Self>, db: &Path, bin: &Path, dir: &Path) -> Option<Proven> {
        {
            let mut m = self.0.lock().unwrap();
            if let Some(known) = m.get(bin) {
                return known.clone();
            }
            if let Some(kept) = kept(bin, dir) {
                m.insert(bin.to_path_buf(), Some(kept.clone()));
                return Some(kept);
            }
            m.insert(bin.to_path_buf(), None);
        }
        let (proofs, db, bin, dir) = (self.clone(), db.to_path_buf(), bin.to_path_buf(), dir.to_path_buf());
        std::thread::spawn(move || {
            let proven = on_host(&db, &bin, &dir);
            proofs.0.lock().unwrap().insert(bin, Some(proven));
        });
        None
    }
}

/// The name `bin` is kept under on the host: its path, size and time, so a
/// binary installed again at the same path is proven again.
fn key(bin: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(bin).ok();
    let stamp = m.map_or([0; 3], |m| [m.size(), m.mtime() as u64, m.mtime_nsec() as u64]);
    let stamp: Vec<u8> = stamp.iter().flat_map(|n| n.to_le_bytes()).collect();
    // FNV-1a: a short name, the same in every process.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bin.as_os_str().as_encoded_bytes().iter().chain(&stamp) {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// How `bin` was proven on the host, as kept in `dir`, if it was.
fn kept(bin: &Path, dir: &Path) -> Option<Proven> {
    let k = std::fs::read(dir.join(format!("{}.json", key(bin)))).ok()?;
    serde_json::from_slice::<Kept>(&k).ok().map(|k| k.why_not.map_or(Ok(()), Err))
}

/// Proves `bin` once on the host: under its lock in `dir`, read as kept by
/// the one that proved it before, or proven and kept.
fn on_host(db: &Path, bin: &Path, dir: &Path) -> Proven {
    let k = key(bin);
    let (lock, file_kept, copy) = (dir.join(format!("{k}.lock")), dir.join(format!("{k}.json")), dir.join(&k));
    let file = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock));
    let file = file.map_err(|e| format!("cannot open {}: {e}", lock.display()))?;
    // Another process proving it holds this until it has kept how.
    file.lock().map_err(|e| format!("cannot lock {}: {e}", lock.display()))?;
    if let Some(proven) = kept(bin, dir) {
        return proven;
    }
    let _ = std::fs::remove_dir_all(&copy);
    let ended = prove(db, bin, &copy.join("factorium.db"));
    if ended.kept {
        let k = Kept { why_not: ended.proven.clone().err() };
        // Whole or not there, for one who reads it without the lock.
        let part = file_kept.with_extension("part");
        let wrote = std::fs::write(&part, serde_json::to_vec(&k).unwrap_or_default());
        let _ = wrote.and_then(|()| std::fs::rename(&part, &file_kept));
    }
    ended.proven
}

/// Copies `db` to `copy` in one read transaction: a writer does not restart
/// it, so it ends in the time one read of the file takes; a lock held past
/// the busy timeout fails it, saying so.
fn copy_of(db: &Path, copy: &Path) -> Result<(), String> {
    use rusqlite::OpenFlags;
    std::fs::create_dir_all(copy.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    let live = rusqlite::Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY);
    let live = live.and_then(|l| l.busy_timeout(Duration::from_secs(5)).map(|()| l));
    live.and_then(|l| l.execute("VACUUM INTO ?1", [copy.to_string_lossy()])).map(drop).map_err(|e| e.to_string())
}

fn prove(db: &Path, bin: &Path, copy: &Path) -> Ended {
    let dir = copy.parent().unwrap_or(Path::new("."));
    if let Err(e) = copy_of(db, copy) {
        let proven = Err(format!("cannot copy {} to {}: {e}", db.display(), copy.display()));
        let _ = std::fs::remove_dir_all(dir);
        return Ended { proven, kept: false };
    }
    let env = [("PROOF_BIN", bin), ("PROOF_DB", copy)].map(|(k, v)| (k.to_string(), v.to_string_lossy().into()));
    let line = r#""$PROOF_BIN" --db "$PROOF_DB" doctor"#;
    let ran = run(dir, line, &env, LIMIT, Priority::Normal, &|_| {}, &|| false);
    if ran.ok {
        let _ = std::fs::remove_dir_all(dir);
        return Ended { proven: Ok(()), kept: true };
    }
    let how = if ran.timed_out { format!("ran past {}s", LIMIT.as_secs()) } else { "failed".into() };
    let (b, c) = (bin.display(), copy.display());
    let proven = Err(format!("{b} cannot open this host's database: doctor on its copy {c} {how}:\n{}", ran.tail));
    Ended { proven, kept: !ran.timed_out }
}
