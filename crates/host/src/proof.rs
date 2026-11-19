//! Proof that a factorium binary opens a database before it runs on it: it
//! answers `doctor` on a copy made by SQLite's backup, never the live file
//! (factorium#141). Each binary is proven once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::shell::{Priority, run};

/// How long a binary has to answer `doctor` on the copy.
const LIMIT: Duration = Duration::from_secs(120);

/// Each binary (canonical) proven: it opens the database, or why not.
#[derive(Default)]
pub struct Proofs(Mutex<BTreeMap<PathBuf, Result<(), String>>>);

impl Proofs {
    /// How `bin` was proven, if it was.
    pub fn of(&self, bin: &Path) -> Option<Result<(), String>> {
        self.0.lock().unwrap().get(bin).cloned()
    }

    /// Proves `bin` on a copy of `db` made at `copy`, and keeps how. The copy
    /// is removed once proven, and kept, named in why not, where it fails.
    pub fn prove(&self, db: &Path, bin: &Path, copy: &Path) -> Result<(), String> {
        let proven = prove(db, bin, copy);
        self.0.lock().unwrap().insert(bin.to_path_buf(), proven.clone());
        proven
    }
}

fn prove(db: &Path, bin: &Path, copy: &Path) -> Result<(), String> {
    let dir = copy.parent().unwrap_or(Path::new("."));
    let copied = std::fs::create_dir_all(dir).map_err(|e| e.to_string()).and_then(|()| {
        let live = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY);
        live.and_then(|l| l.backup(rusqlite::MAIN_DB, copy, None::<fn(_)>)).map_err(|e| e.to_string())
    });
    if let Err(e) = copied {
        return Err(format!("cannot copy {} to {}: {e}", db.display(), copy.display()));
    }
    let env = [("PROOF_BIN", bin), ("PROOF_DB", copy)].map(|(k, v)| (k.to_string(), v.to_string_lossy().into()));
    let line = r#""$PROOF_BIN" --db "$PROOF_DB" doctor"#;
    let ran = run(dir, line, &env, LIMIT, Priority::Normal, &|_| {}, &|| false);
    if ran.ok {
        let _ = std::fs::remove_dir_all(dir);
        return Ok(());
    }
    let how = if ran.timed_out { format!("ran past {}s", LIMIT.as_secs()) } else { "failed".into() };
    let (bin, copy) = (bin.display(), copy.display());
    Err(format!("{bin} cannot open this host's database: doctor on its copy {copy} {how}:\n{}", ran.tail))
}
