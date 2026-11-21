//! A file kept open across an exec: the image that comes after owns it only
//! if the descriptor it is handed is open on that very file.

use std::os::fd::IntoRawFd;

use factorium_host::procs::{inherited, keep_across_exec};

#[test]
fn a_descriptor_handed_through_an_exec_is_owned_only_on_its_own_file() {
    let dir = std::env::temp_dir().join(format!("factorium-exec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (pid, other) = (dir.join("daemon.pid"), dir.join("other"));
    std::fs::write(&other, "").unwrap();
    let f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&pid).unwrap();
    f.try_lock().unwrap();
    let fd = keep_across_exec(&f).unwrap();
    // As the next image finds it: open, its `File` gone with the old image.
    let fd2 = f.into_raw_fd();
    assert_eq!(fd, fd2);
    assert!(inherited(fd, &other).is_none(), "open on another file: not this one's");
    let owned = inherited(fd, &pid).expect("open on the pid file");
    // Its lock came with it, and it is relocked through it.
    assert!(owned.try_lock().is_ok());
    let again = std::fs::File::open(&pid).unwrap();
    assert!(matches!(again.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    drop(owned);
    assert!(again.try_lock().is_ok(), "closed: the lock goes");
    std::fs::remove_dir_all(&dir).unwrap();
}
