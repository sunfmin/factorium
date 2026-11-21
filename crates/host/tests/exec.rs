//! A file kept open across an exec: open in the programs this process runs
//! while kept, and owned by the image after it only if the descriptor it is
//! handed is open on that very file.

use std::os::fd::IntoRawFd;

use factorium_host::procs::{inherited, kept_across_exec};

/// Whether a program this process runs has descriptor `fd` open.
fn open_in_a_child(fd: i32) -> bool {
    let probe = format!(": <&{fd}");
    std::process::Command::new("sh")
        .args(["-c", &probe])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn a_descriptor_handed_through_an_exec_is_owned_only_on_its_own_file() {
    let dir = std::env::temp_dir().join(format!("factorium-exec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (pid, other) = (dir.join("daemon.pid"), dir.join("other"));
    std::fs::write(&other, "").unwrap();
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&pid).unwrap();
    f.try_lock().unwrap();
    let fd = f.into_raw_fd();
    // As the next image finds it: open, its `File` gone with the old image.
    assert!(inherited(fd, &other).is_none(), "open on another file: not this one's");
    let owned = inherited(fd, &pid).expect("open on the pid file");
    assert!(!open_in_a_child(fd), "owned, it closes in programs this one runs");
    assert_eq!(kept_across_exec(&owned, true).unwrap(), fd);
    assert!(open_in_a_child(fd), "kept, it is open in them");
    // Its lock came with it.
    let again = std::fs::File::open(&pid).unwrap();
    assert!(matches!(again.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    drop(owned);
    assert!(again.try_lock().is_ok(), "closed: the lock goes");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[allow(unsafe_code)]
fn a_descriptor_not_open_is_neither_kept_nor_inherited() {
    use std::os::fd::FromRawFd;
    // Far past any this process opens.
    let fd = 1 << 20;
    // SAFETY: never read, written or closed: ManuallyDrop keeps it from closing.
    let none = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
    assert!(kept_across_exec(&none, true).is_err());
    assert!(inherited(fd, &std::env::temp_dir()).is_none());
}
