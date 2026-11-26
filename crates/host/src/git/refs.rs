//! What git would answer, without running it, where that is certain: the
//! commit a branch or remote-tracking ref names, read from the repo's ref
//! files, and answers fixed by the object ids they name, kept once git gave
//! them. A tick asks every checkout for its base's lag and the files its
//! tips declare; each asked of git is one more process (~6ms of CPU here).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The commit the ref `name` (`refs/heads/…` or `refs/remotes/…`) names in
/// the repo whose work dir is `dir`, read as git's files backend keeps it:
/// a loose ref, else its line in `packed-refs`. `None` where git must be
/// asked: another kind of ref, a symbolic ref, none of that name (git says
/// why), or a ref file that does not read, as in a worktree (its `.git` is a
/// file) or a reftable repo (its `refs/heads` is a file).
pub(super) fn read(dir: &Path, name: &str) -> Option<String> {
    if !(name.starts_with("refs/heads/") || name.starts_with("refs/remotes/")) || name.contains("..") {
        return None;
    }
    let git = dir.join(".git");
    match std::fs::read_to_string(git.join(name)) {
        Ok(loose) => object_id(loose.trim()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let packed = std::fs::read_to_string(git.join("packed-refs")).ok()?;
            let id = packed.lines().find_map(|l| l.split_once(' ').filter(|(_, n)| *n == name))?.0;
            object_id(id)
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

/// What `ask` answers for `question` in `dir`, asked once: `question` is
/// fixed by the object ids it names, so its answer is too. An error is kept
/// only where `fixed` says it is one of those.
pub(super) fn once(
    dir: &Path,
    question: &str,
    fixed: impl Fn(&str) -> bool,
    ask: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    type Answers = Mutex<HashMap<(PathBuf, String), Result<String, String>>>;
    static ANSWERS: OnceLock<Answers> = OnceLock::new();
    let answers = ANSWERS.get_or_init(Mutex::default);
    let key = (dir.to_path_buf(), question.to_string());
    if let Some(answer) = answers.lock().expect("nothing panics holding it").get(&key) {
        return answer.clone();
    }
    let answer = ask();
    if answer.as_ref().is_ok() || answer.as_ref().is_err_and(|e| fixed(e)) {
        let mut kept = answers.lock().expect("nothing panics holding it");
        if kept.len() >= KEPT {
            kept.clear();
        }
        kept.insert(key, answer.clone());
    }
    answer
}
