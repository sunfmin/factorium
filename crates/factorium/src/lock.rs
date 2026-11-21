//! A lock held on a file for as long as its holder needs it: an exclusive
//! `flock`, held by the file it is taken through and released when that
//! closes or its process ends, however it ends.

use std::path::Path;

/// Takes the lock on `path` and writes `text` in it, unless it is held:
/// then answers what the file says (empty if it says nothing). A file
/// removed while it was being taken is taken again at its path, so two
/// holders never hold two files at one path.
pub(crate) fn take(path: &Path, text: &str) -> Result<std::fs::File, Taken> {
    use std::io::{Read, Seek, Write};
    use std::os::unix::fs::MetadataExt;
    loop {
        let opened = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path));
        let mut f = opened.map_err(|e| Taken::Fails(format!("its lock {} does not open: {e}", path.display())))?;
        match f.try_lock() {
            Ok(()) => {
                let ours = f.metadata().map(|m| (m.dev(), m.ino())).ok();
                if ours != std::fs::metadata(path).map(|m| (m.dev(), m.ino())).ok() {
                    continue;
                }
                let _ = f.set_len(0).and_then(|()| f.rewind());
                let _ = write!(f, "{text}");
                return Ok(f);
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let mut held = String::new();
                let _ = f.read_to_string(&mut held);
                return Err(Taken::Held(held));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(Taken::Fails(format!("its lock {} does not lock: {e}", path.display())));
            }
        }
    }
}

/// Whether another holds the lock on `path`. A shared lock is tried and let
/// go at once, never taken over a holder; a file not there is not held.
pub(crate) fn held(path: &Path) -> bool {
    let Ok(f) = std::fs::File::open(path) else { return false };
    matches!(f.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock))
}

/// Why a lock was not taken.
#[derive(Debug)]
pub(crate) enum Taken {
    /// Another holds it; its file says this.
    Held(String),
    /// The file does not open or lock.
    Fails(String),
}
