//! What a look at a session file found, kept while the file is unchanged.
//!
//! A tick looks at every live session, and each look goes through every
//! file that may be its session: under Claude Code's one tree that is every
//! transcript on the host, gigabytes of them (factorium#221). What one look
//! found of a file for a session is kept here, by the file's size, its
//! modification time and its inode; a later look at the file unchanged
//! answers from it and reads nothing. A file that changed is read again.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use crate::turn::Turn;

/// What one session file records for one session: its turn, `None` when it
/// is not the session's or holds no turn, and the tokens it says the session
/// spent, `None` when it counts none for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fact {
    pub(super) turn: Option<Turn>,
    pub(super) spent: Option<(u64, u64)>,
}

/// A file as it was when it was read: its size, modification time, inode.
type Stamp = (u64, SystemTime, u64);

/// What looks found of each file as it was, by the session asked about.
type Kept = HashMap<PathBuf, (Stamp, HashMap<u64, Fact>)>;
static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

/// So many files kept before the ones gone are forgotten, and so many
/// sessions kept of one file before they are.
const FILES: usize = 16_384;
const SESSIONS: usize = 256;

/// What `path` records for the session `asked` names (its harness, worktree
/// and brief): what an earlier look found when the file is unchanged since,
/// else what `find` makes of it now, kept. `None`: the file is not there or
/// `find` could not read it.
pub(super) fn fact(path: &Path, asked: (&str, &str, &str), find: impl FnOnce() -> Option<Fact>) -> Option<Fact> {
    static HASH: std::sync::OnceLock<RandomState> = std::sync::OnceLock::new();
    let session = HASH.get_or_init(RandomState::new).hash_one(asked);
    let meta = std::fs::metadata(path).ok()?;
    let stamp = (meta.len(), meta.modified().ok()?, meta.ino());
    let found = |kept: &Kept| {
        kept.get(path).filter(|(s, _)| *s == stamp).and_then(|(_, facts)| facts.get(&session).copied())
    };
    if let Some(fact) = KEPT.lock().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(found) {
        return Some(fact);
    }
    let fact = find()?;
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let kept = kept.get_or_insert_with(HashMap::new);
    if kept.len() >= FILES {
        kept.retain(|path, _| path.exists());
    }
    let (s, facts) = kept.entry(path.to_path_buf()).or_insert_with(|| (stamp, HashMap::new()));
    if *s != stamp || facts.len() >= SESSIONS {
        (*s, *facts) = (stamp, HashMap::new());
    }
    facts.insert(session, fact);
    Some(fact)
}
