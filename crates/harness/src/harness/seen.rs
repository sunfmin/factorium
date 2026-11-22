//! What a look at a session file found, kept while the file is unchanged.
//!
//! A tick looks at every live session, and each look goes through every
//! file that may be its session: under Claude Code's one tree that is every
//! transcript on the host, gigabytes of them (factorium#221). What one look
//! found of a file for a session is kept here, by the file's size, its
//! modification time and its inode; a later look at the file unchanged
//! answers from it and reads nothing. A file that changed is read again.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::turn::Turn;

/// What one session file records for one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fact {
    /// The turn it records: `None` when it is not the session's or holds no
    /// turn.
    pub(super) turn: Option<Turn>,
    /// The tokens it says the session spent, once a look asked: `Some(None)`
    /// when it counts none for the session.
    pub(super) spent: Option<Option<(u64, u64)>>,
}

/// A file as it was when it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
    inode: u64,
}

/// What looks found of one file as it was: by the session asked about.
struct Seen {
    stamp: Stamp,
    facts: HashMap<u64, Fact>,
}

/// So many files kept before the ones gone are forgotten, and so many
/// sessions kept of one file before they are.
const FILES: usize = 16_384;
const SESSIONS: usize = 256;

fn kept() -> &'static Mutex<HashMap<PathBuf, Seen>> {
    static KEPT: OnceLock<Mutex<HashMap<PathBuf, Seen>>> = OnceLock::new();
    KEPT.get_or_init(Mutex::default)
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { len: meta.len(), modified: meta.modified().ok()?, inode: meta.ino() })
}

/// Which session of which harness is asked about.
pub(super) fn session(harness: &str, cwd: &str, brief: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (harness, cwd, brief).hash(&mut h);
    h.finish()
}

/// What `path` records for `session`: what an earlier look found when the
/// file is unchanged since and that look answered as much as is `wanted`,
/// else what `find` makes of it now, kept. `find` is given what was found
/// before of the file unchanged. `None`: the file is not there or `find`
/// could not read it.
pub(super) fn fact(
    path: &Path,
    session: u64,
    wanted: impl Fn(&Fact) -> bool,
    find: impl FnOnce(Option<Fact>) -> Option<Fact>,
) -> Option<Fact> {
    let stamp = stamp(path)?;
    let before = {
        let kept = kept().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        kept.get(path).filter(|seen| seen.stamp == stamp).and_then(|seen| seen.facts.get(&session).copied())
    };
    if let Some(fact) = before.filter(|f| wanted(f)) {
        return Some(fact);
    }
    let fact = find(before)?;
    let mut kept = kept().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if kept.len() >= FILES {
        kept.retain(|path, _| path.exists());
    }
    let seen = kept.entry(path.to_path_buf()).or_insert_with(|| Seen { stamp, facts: HashMap::new() });
    if seen.stamp != stamp || seen.facts.len() >= SESSIONS {
        *seen = Seen { stamp, facts: HashMap::new() };
    }
    seen.facts.insert(session, fact);
    Some(fact)
}
