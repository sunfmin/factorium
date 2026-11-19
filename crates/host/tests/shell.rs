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
