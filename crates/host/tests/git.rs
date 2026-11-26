//! git's answers read without running it: a branch's commit as the repo's
//! ref files hold it, loose or packed, and what git answers of fixed
//! commits, are what git itself answers.

use factorium_host::{Git, Lag};

#[test]
fn a_branch_is_read_as_git_reads_it_loose_over_packed_and_in_a_worktree_from_git() {
    let dir = std::env::temp_dir().join(format!("factorium-host-refs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (home, repo) = (dir.join("home"), dir.join("repo"));
    std::fs::create_dir_all(&home).unwrap();
    let git = Git::new(&home, &repo);
    git.init("one").unwrap();
    let one = git.run(&["rev-parse", "HEAD"]).unwrap();
    git.run(&["branch", "kept"]).unwrap();
    git.run(&["pack-refs", "--all"]).unwrap();
    assert_eq!(git.sha("refs/heads/kept").unwrap(), one, "packed");
    git.run(&["commit", "--quiet", "--allow-empty", "-m", "two"]).unwrap();
    let two = git.run(&["rev-parse", "HEAD"]).unwrap();
    let branch = git.run(&["symbolic-ref", "--short", "HEAD"]).unwrap();
    assert_eq!(git.sha(&format!("refs/heads/{branch}")).unwrap(), two, "loose, over its packed line");
    assert!(git.sha("refs/heads/none").is_err(), "no such branch");
    // The base `kept` is one commit behind the branch, and none ahead of it.
    assert_eq!(git.lag("kept", &format!("refs/heads/{branch}")).unwrap(), Lag::Behind(1));
    assert_eq!(git.lag("kept", &format!("refs/heads/{branch}")).unwrap(), Lag::Behind(1), "asked again");
    assert_eq!(git.lag("none", &two).unwrap(), Lag::Even, "no such local branch");
    std::fs::write(repo.join("f"), "at two\n").unwrap();
    git.run(&["add", "f"]).unwrap();
    git.run(&["commit", "--quiet", "-m", "three"]).unwrap();
    assert_eq!(git.shown(&format!("refs/heads/{branch}"), "f").as_deref(), Some("at two"));
    assert_eq!(git.shown(&two, "f"), None, "the file is not at two");
    assert_eq!(git.shown(&two, "f"), None, "nor is it, asked again");
    // A worktree's `.git` is a file: git is asked, and answers the same.
    let tree = dir.join("tree");
    git.run(&["worktree", "add", "--quiet", "--detach", &tree.to_string_lossy(), &one]).unwrap();
    assert_eq!(git.at(&tree).sha("refs/heads/kept").unwrap(), one);
    std::fs::remove_dir_all(&dir).unwrap();
}
