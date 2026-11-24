//! Sessions as their harnesses write them: the files a harness's own
//! stand-in writes where its reader looks, saying whether its turn is open.

use factorium::host::Host;
use factorium_harness::Harness;

use super::World;

/// Writes the session files a harness's stand-in describes for `cwd`, in
/// the home of its sessions on `account`.
pub fn write_session(w: &World, h: Harness, account: Option<&str>, cwd: &str, open: bool, at: u64) {
    // The brief its session was handed: the watch's, in its home; a worker's,
    // named as its worktree.
    let path = std::path::Path::new(cwd);
    let brief = if path == w.data().watch() {
        w.data().watch_brief()
    } else {
        w.data().brief(&path.file_name().unwrap().to_string_lossy())
    };
    write_session_in(w, h, account, cwd, &brief.to_string_lossy(), open, at);
}

/// The stand-in session of `h` run in `dir` and handed `brief`, on
/// `account` (`None`: the one named after its engine, where the world
/// keeps one, `World::kit_of`): on a bound login, in the home this host
/// keeps for it.
pub fn write_session_in(w: &World, h: Harness, account: Option<&str>, dir: &str, brief: &str, open: bool, at: u64) {
    let kept = w.data().kept(h.key());
    let books = w.host.stablehand.lock().unwrap();
    let account = account.or(Some(h.key()));
    let bound = account.filter(|a| books.accounts.get(*a).is_some_and(|x| x.home.is_some()));
    let home = bound.map(|a| w.data().kept_on(h.key(), a));
    drop(books);
    h.write_session(&h.home(&w.host.home(), home.as_deref(), &kept), dir, brief, open, at).unwrap();
}

impl World {
    /// The directory this host keeps for sessions of the engine `key` on
    /// the world's own account of it (`World::kit_of`): its bound login's,
    /// or Claude Code's, portable, the engine's own kept directory.
    pub fn kept_home(&self, key: &str) -> std::path::PathBuf {
        match key {
            "claude-code" => self.data().kept(key),
            _ => self.data().kept_on(key, key),
        }
    }

    /// The Claude Code session run in `cwd` writes its turn, `open` or
    /// closed, at `at` (seconds).
    pub fn claude_session(&self, cwd: &str, open: bool, at: u64) {
        write_session(self, Harness::ClaudeCode, None, cwd, open, at);
    }

    /// The watch's session writes its turn: open, or closed.
    pub fn watch_session(&self, open: bool) {
        let home = self.data().watch();
        self.claude_session(&home.to_string_lossy(), open, (self.app_now() / 1000) as u64);
    }

    /// Its Claude Code session is in a turn (`busy`) or not, which its
    /// session file says; `unobserved` as for [`super::Terminal`].
    pub fn set_busy(&self, handle: &str, busy: bool, unobserved: bool) {
        let cwd = {
            let mut t = self.host.orca.terminals.lock().unwrap();
            let term = t.get_mut(handle).unwrap();
            (term.busy, term.unobserved) = (busy, unobserved);
            term.worktree.to_string_lossy().into_owned()
        };
        self.claude_session(&cwd, busy, (self.app_now() / 1000) as u64);
    }
}
