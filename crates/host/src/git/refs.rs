//! What git would answer, without running it, where that is certain: the
//! commit a branch or remote-tracking ref names, read from the repo's ref
//! files, and answers fixed by the object ids they name, kept once git gave
//! them. A tick asks every checkout for its base's lag and the files its
//! tips declare; each asked of git is one more process (factorium#366).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The commit the ref `name` (`refs/heads/…` or `refs/remotes/…`) names in
/// the repo whose work dir is `dir`, read as git's files backend keeps it:
/// a loose ref, else its line in `packed-refs`. `None` where git must be
/// asked: another kind of ref, a worktree's or a reftable repo, a symbolic
/// ref, or none of that name (git says why).
pub(super) fn read(dir: &Path, name: &str) -> Option<String> {
    if !(name.starts_with("refs/heads/") || name.starts_with("refs/remotes/")) || name.contains("..") {
        return None;
    }
    let git = dir.join(".git");
    if !git.is_dir() || git.join("reftable").exists() {
        return None;
    }
    match std::fs::read_to_string(git.join(name)) {
        Ok(loose) => object_id(loose.trim()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let packed = std::fs::read_to_string(git.join("packed-refs")).ok()?;
            let line = packed.lines().find(|l| l.split_once(' ').is_some_and(|(_, n)| n == name))?;
            object_id(line.split_once(' ')?.0)
        }
        Err(_) => None,
    }
}

/// `id`, if it is a full object id (sha1 or sha256).
pub(super) fn object_id(id: &str) -> Option<String> {
    let full = matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit());
    full.then(|| id.to_ascii_lowercase())
}

/// The most answers kept: past it they are dropped and asked again.
const KEPT: usize = 512;

type Key = (PathBuf, String);

fn kept() -> &'static Mutex<HashMap<Key, Result<String, String>>> {
    static KEPT_ANSWERS: OnceLock<Mutex<HashMap<Key, Result<String, String>>>> = OnceLock::new();
    KEPT_ANSWERS.get_or_init(Mutex::default)
}

/// What `ask` answers for `question` in `dir`, asked once: `question` is
/// fixed by the object ids it names, so its answer is too. An error is kept
/// only where `fixed` says it is one of those.
pub(super) fn once(
    dir: &Path,
    question: &str,
    fixed: impl Fn(&str) -> bool,
    ask: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    let key = (dir.to_path_buf(), question.to_string());
    if let Some(answer) = kept().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return answer.clone();
    }
    let answer = ask();
    if answer.as_ref().is_ok() || answer.as_ref().is_err_and(|e| fixed(e)) {
        let mut kept = kept().lock().unwrap_or_else(|e| e.into_inner());
        if kept.len() >= KEPT {
            kept.clear();
        }
        kept.insert(key, answer.clone());
    }
    answer
}
