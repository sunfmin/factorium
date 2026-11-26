//! git's answers read without running it: a branch's commit as the repo's
//! ref files hold it, loose or packed, and what git answers of fixed
//! commits, are what git itself answers.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use factorium_host::{Git, Lag};

/// A fresh directory for `name`, and git there as a host whose home it holds.
fn scratch(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("factorium-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    (dir.join("home"), dir)
}

fn head(git: &Git) -> String {
    git.run(&["rev-parse", "HEAD"]).unwrap()
}

fn commit(git: &Git, file: &str, text: &str) -> String {
    std::fs::write(git.dir.join(file), text).unwrap();
    git.run(&["add", file]).unwrap();
    git.run(&["commit", "--quiet", "-m", file]).unwrap();
    head(git)
}

/// The git commands `f` ran as the host whose home is `home`, by name, as
/// git's own trace of each tells them.
fn ran(home: &Path, f: impl FnOnce()) -> Vec<String> {
    let trace = home.join("trace");
    let _ = std::fs::remove_file(&trace);
    std::fs::write(home.join(".gitconfig"), format!("[trace2]\n\tnormalTarget = {}\n", trace.display())).unwrap();
    f();
    std::fs::remove_file(home.join(".gitconfig")).unwrap();
    let traced = std::fs::read_to_string(&trace).unwrap_or_default();
    traced
        .lines()
        .filter_map(|l| l.split(" cmd_name ").nth(1))
        .map(|c| c.split(' ').next().unwrap().to_string())
        .collect()
}

#[test]
fn a_branch_is_read_as_git_reads_it_loose_over_packed_and_in_a_worktree_from_git() {
    let (home, dir) = scratch("refs");
    let git = Git::new(&home, dir.join("repo"));
    git.init("one").unwrap();
    let one = head(&git);
    git.run(&["branch", "kept"]).unwrap();
    git.run(&["pack-refs", "--all"]).unwrap();
    let read = ran(&home, || assert_eq!(git.sha("refs/heads/kept").unwrap(), one, "packed"));
    assert!(read.is_empty(), "read from its packed line, not asked of git: {read:?}");
    let two = commit(&git, "f", "at two\n");
    let branch = format!("refs/heads/{}", git.run(&["symbolic-ref", "--short", "HEAD"]).unwrap());
    assert_eq!(git.sha(&branch).unwrap(), two, "loose, over its packed line");
    assert!(git.sha("refs/heads/none").is_err(), "no such branch");
    // A name that climbs out of the refs is no ref, whatever file it reaches.
    git.run(&["update-ref", "ORIG_HEAD", &one]).unwrap();
    assert!(git.sha("refs/heads/../../ORIG_HEAD").is_err());
    // A loose ref that does not read is never answered by its packed line.
    let loose = git.dir.join(".git").join(&branch);
    git.run(&["pack-refs", "--all"]).unwrap();
    let three = commit(&git, "f", "at three\n");
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_ne!(git.sha(&branch).ok(), Some(two.clone()), "not the packed line");
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(git.sha(&branch).unwrap(), three);
    // A worktree's `.git` is a file: git is asked, and answers the same.
    let tree = dir.join("tree");
    git.run(&["worktree", "add", "--quiet", "--detach", &tree.to_string_lossy(), &one]).unwrap();
    assert_eq!(git.at(&tree).sha("refs/heads/kept").unwrap(), one);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn what_git_answers_of_fixed_commits_is_asked_once() {
    let (home, dir) = scratch("once");
    let git = Git::new(&home, dir.join("repo"));
    git.init("one").unwrap();
    let one = head(&git);
    git.run(&["branch", "kept"]).unwrap();
    let two = commit(&git, "f", "at two\n");
    let branch = format!("refs/heads/{}", git.run(&["symbolic-ref", "--short", "HEAD"]).unwrap());
    // `kept` is one commit behind the branch, and none ahead of it.
    assert_eq!(git.lag("kept", &branch).unwrap(), Lag::Behind(1));
    assert_eq!(git.lag("none", &two).unwrap(), Lag::Even, "no such local branch");
    assert!(git.lag("kept", "refs/heads/none").is_err(), "no such tip");
    assert_eq!(git.shown(&branch, "f").as_deref(), Some("at two"));
    assert_eq!(git.shown(&one, "f"), None, "the file is not at one");
    // Asked again, git is not, but for where the branch behind is checked
    // out, which may change.
    let again = ran(&home, || {
        assert_eq!(git.lag("kept", &branch).unwrap(), Lag::Behind(1));
        assert_eq!(git.shown(&two, "f").as_deref(), Some("at two"));
        assert_eq!(git.shown(&one, "f"), None);
    });
    assert_eq!(again, ["worktree"]);
    // A branch whose name is as long as an object id is still a branch: it moves.
    let long = "g".repeat(40);
    git.run(&["branch", &long, &one]).unwrap();
    assert_eq!(git.shown(&long, "f"), None);
    git.run(&["branch", "--force", &long, &two]).unwrap();
    assert_eq!(git.shown(&long, "f").as_deref(), Some("at two"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fetch_with_tags_refused_fetches_the_branch_without_them() {
    let (home, dir) = scratch("fetch");
    let origin = Git::new(&home, dir.join("origin"));
    origin.init("one").unwrap();
    let one = head(&origin);
    origin.run(&["tag", "v1"]).unwrap();
    let clone = |name: &str| {
        let at: &Path = &dir;
        Git::new(&home, at).run(&["clone", "--quiet", &origin.dir.to_string_lossy(), name]).unwrap();
        Git::new(&home, at.join(name))
    };
    let here = clone("here");
    let branch = origin.run(&["symbolic-ref", "--short", "HEAD"]).unwrap();
    // Origin moves its tag: fetching tags would clobber this one's.
    let two = commit(&origin, "f", "at two\n");
    origin.run(&["tag", "--force", "v1"]).unwrap();
    assert_eq!(here.fetch(&branch).unwrap(), two);
    assert_eq!(here.sha("refs/tags/v1").unwrap(), one, "its tag left as it was");
    let fetched = ran(&home, || assert!(here.fetch("none").is_err(), "origin has no such branch"));
    assert_eq!(fetched.iter().filter(|c| *c == "fetch").count(), 1, "not again without tags: {fetched:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
