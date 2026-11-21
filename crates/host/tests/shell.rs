//! The real programs, as the daemon looks for the kit's at its start: a
//! file that may be run, found by its path or by its name on `PATH`.

use std::os::unix::fs::PermissionsExt;

use factorium_host::shell::{Priority, Programs, System, run};

#[test]
fn a_program_is_found_only_as_a_file_that_may_be_run() {
    let dir = std::env::temp_dir().join(format!("factorium-host-found-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("adir")).unwrap();
    let file = |name: &str, mode: u32| {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path.to_string_lossy().into_owned()
    };
    let (runs, kept) = (file("runs", 0o755), file("kept", 0o644));
    assert!(System.found(&runs), "by its path");
    assert!(!System.found(&kept), "not to be run");
    assert!(!System.found(&dir.join("adir").to_string_lossy()), "a directory");
    assert!(!System.found(&dir.join("absent").to_string_lossy()));
    assert!(System.found("sh"), "by its name, on PATH");
    assert!(!System.found("factorium-no-such-program"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A command whose directory is gone (a worktree removed under a step)
/// never starts: it fails, says why, and nothing ran.
#[test]
fn a_command_in_a_missing_directory_fails_and_says_it_could_not_start() {
    let gone = std::path::Path::new("/nonexistent/factorium-host-gone");
    let ran = run(gone, "true", &[], std::time::Duration::from_secs(5), Priority::Normal, &|_| {}, &|| false);
    assert!(!ran.ok && ran.code.is_none() && !ran.timed_out, "{ran:?}");
    assert!(ran.tail.starts_with("cannot start sh: "), "{}", ran.tail);
}

/// A run that ignores TERM is ended all the same: once the grace is over,
/// what is left of its session is KILLed.
#[test]
fn a_run_that_ignores_term_is_killed_after_the_grace() {
    let dir = std::env::temp_dir().join(format!("factorium-host-term-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let up = dir.join("up");
    let line = format!("trap '' TERM; sleep 30 & echo $! > {}; wait", up.display());
    let limit = std::time::Duration::from_secs(60);
    let ran = run(&dir, &line, &[], limit, Priority::Normal, &|_| {}, &|| up.exists());
    let sleep: u32 = std::fs::read_to_string(&up).unwrap().trim().parse().unwrap();
    assert!(!ran.ok && ran.took_ms >= 2000 && ran.took_ms < 30_000, "{ran:?}");
    assert!(!factorium_host::procs::alive(sleep), "the sleep that ignored TERM is gone");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Ending a session never reaches launchd's nor the caller's own.
#[test]
fn no_session_but_a_runs_own_is_signalled() {
    use factorium_host::procs::signal_session;
    // SAFETY: getsid(0) only reads this process's session.
    #[allow(unsafe_code)]
    let mine = u32::try_from(unsafe { libc::getsid(0) }).unwrap();
    assert_eq!(signal_session(1, 0), 0);
    assert_eq!(signal_session(mine, 0), 0);
}

/// Whether a process lives, asked of the kernel: this one does, so does one
/// of another user's that may not be signalled (launchd), and a child that
/// exited and was reaped does not.
#[test]
fn alive_answers_for_live_processes_and_another_user_s_and_not_for_a_reaped_one() {
    use factorium_host::procs::alive;
    assert!(alive(std::process::id()), "this process");
    assert!(alive(1), "pid 1, not ours to signal");
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!alive(pid), "a reaped child is gone");
}
