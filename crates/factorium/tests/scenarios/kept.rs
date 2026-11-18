//! Kept worktrees when the proving tip cannot be read (SPEC §2.6): what a
//! worktree holds is unknown, so it is held. A start refuses and takes
//! nothing; a worker that ends keeps its worktree; start-over discards it,
//! keeping its branch.

use std::path::PathBuf;

use serde_json::json;

use crate::{CLAUDE, World, worktree};

/// Hides `web`'s remote, so its proving tip cannot be fetched, until `back`.
struct Unreachable {
    at: PathBuf,
    away: PathBuf,
}

impl Unreachable {
    fn web(w: &World) -> Unreachable {
        let at = w.dir.join("remotes").join("web.git");
        let away = w.dir.join("remotes").join("web.git.away");
        std::fs::rename(&at, &away).unwrap();
        Unreachable { at, away }
    }

    fn back(self) {
        std::fs::rename(&self.away, &self.at).unwrap();
    }
}

/// `web` with issue 1 parked, its worker's worktree holding `own` commits
/// of its own; answers that worktree.
async fn parked(w: &World, own: bool) -> PathBuf {
    let tree = worktree(&w.working(CLAUDE, "t").await);
    if own {
        w.commit(&tree, "kept.txt", "x");
    }
    w.app.issue_park("web", 1, "later").await.unwrap();
    tree
}

#[tokio::test]
async fn a_start_with_the_proving_tip_unread_takes_nothing_and_resumes_once_it_reads() {
    let w = World::new();
    let tree = parked(&w, true).await;
    w.app.issue_ready("web", 1).await.unwrap();
    let away = Unreachable::web(&w);
    let refused = w.app.tick().await.unwrap_err();
    assert_eq!(refused.code, "git", "{refused:?}");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["worker"], json!(null), "nothing taken");
    assert!(tree.exists(), "the kept worktree stays");

    away.back();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worker"], json!(factorium_domain::worker_id("web", 1, 2)), "no attempt was spent");
    assert_eq!(again["worktree"], json!(tree), "it resumes in the kept worktree");
}

#[tokio::test]
async fn a_start_that_failed_after_taking_the_issue_still_resumes_the_kept_worktree() {
    let w = World::new();
    let tree = parked(&w, true).await;
    w.app.issue_ready("web", 1).await.unwrap();
    *w.host.orca.refuse_open.lock().unwrap() = true;
    assert!(w.app.tick().await.is_err(), "the session does not open");
    *w.host.orca.refuse_open.lock().unwrap() = false;
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worktree"], json!(tree), "the attempt that never started kept nothing, the one before did");
}

#[tokio::test]
async fn a_worker_that_ends_with_the_proving_tip_unread_keeps_its_worktree() {
    let w = World::new();
    let tree = worktree(&w.working(CLAUDE, "t").await);
    let away = Unreachable::web(&w);
    w.app.issue_close("web", 1, "not needed").await.unwrap();
    assert!(tree.exists(), "it holds nothing of its own, but that cannot be read: it stays");
    away.back();
}

#[tokio::test]
async fn starting_over_with_the_proving_tip_unread_discards_the_worktree_and_keeps_its_branch() {
    let w = World::new();
    let tree = parked(&w, false).await;
    let away = Unreachable::web(&w);
    let over = w.app.issue_start_over("web", 1, "wrong approach").await.unwrap();
    assert_eq!(over["discarded"], json!(tree));
    assert_eq!(over["abandoned"], "factorium/web-1-1-abandoned");
    assert!(!tree.exists(), "the worktree is gone");
    away.back();

    w.app.issue_ready("web", 1).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worktree"], json!(w.dir.join("worktrees").join("web-1-2")), "a new worktree");
}
