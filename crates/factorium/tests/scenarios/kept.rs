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

/// `web` with issue 1 waiting for a human, its worker's worktree holding `own` commits
/// of its own; answers that worktree.
async fn waiting_for_human(w: &World, own: bool) -> PathBuf {
    let tree = worktree(&w.working(CLAUDE, "t").await);
    if own {
        w.commit(&tree, "kept.txt", "x");
    }
    w.app.issue_wait_for_human("web", 1, "later").await.unwrap();
    tree
}

#[tokio::test]
async fn a_start_with_the_proving_tip_unread_takes_nothing_and_resumes_once_it_reads() {
    let w = World::new();
    let tree = waiting_for_human(&w, true).await;
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
    let tree = waiting_for_human(&w, true).await;
    w.app.issue_ready("web", 1).await.unwrap();
    *w.host.orca.refuse_open.lock().unwrap() = true;
    assert!(w.app.tick().await.is_err(), "the session does not open");
    *w.host.orca.refuse_open.lock().unwrap() = false;
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worktree"], json!(tree), "the attempt that never started kept nothing, the one before did");
}

#[tokio::test]
async fn closing_with_the_proving_tip_unread_removes_the_worktree_and_keeps_its_branch() {
    let w = World::new();
    let tree = worktree(&w.working(CLAUDE, "t").await);
    let away = Unreachable::web(&w);
    let closed = w.app.issue_close("web", 1, "not needed").await.unwrap();
    away.back();
    // It holds nothing of its own, but that cannot be read: its branch is kept.
    assert_eq!(closed["abandoned"], "factorium/web-1-1-abandoned");
    assert!(!tree.exists(), "the worktree is gone");
}

/// A closed issue's worktree that Orca would not remove; answers the
/// worktree, holding a commit and a change of its own.
async fn left_by_a_close(w: &World) -> PathBuf {
    let tree = worktree(&w.working(CLAUDE, "t").await);
    w.commit(&tree, "kept.txt", "x");
    std::fs::write(tree.join("wip.txt"), "half done").unwrap();
    *w.host.orca.refuse_rm.lock().unwrap() = true;
    let refused = w.app.issue_close("web", 1, "not needed").await.unwrap_err();
    assert_eq!(refused.code, "orca", "{refused:?}");
    *w.host.orca.refuse_rm.lock().unwrap() = false;
    assert!(tree.exists());
    tree
}

#[tokio::test]
async fn closing_an_issue_removes_its_kept_worktree_and_keeps_its_branch() {
    let w = World::new();
    let tree = worktree(&w.working(CLAUDE, "t").await);
    w.commit(&tree, "kept.txt", "x");
    std::fs::write(tree.join("wip.txt"), "half done").unwrap();
    let closed = w.app.issue_close("web", 1, "superseded").await.unwrap();
    assert_eq!(closed["status"], "closed");
    assert_eq!(closed["discarded"], json!(tree));
    assert!(!tree.exists(), "the worktree is gone");
    let old = "factorium/web-1-1-abandoned";
    assert_eq!(closed["abandoned"], old);
    let repo = w.dir.join("repos").join("web");
    assert_eq!(w.git(&repo, &["show", &format!("{old}:kept.txt")]).unwrap(), "x");
    assert_eq!(w.git(&repo, &["show", &format!("{old}:wip.txt")]).unwrap(), "half done");
    assert!(w.git(&repo, &["rev-parse", "--verify", "factorium/web-1-1"]).is_err(), "only the abandoned one");
    let last = closed["comments"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["by"], "factorium", "{last}");
    assert!(last["text"].as_str().unwrap().contains(&format!("`{old}`")), "{last}");
}

#[tokio::test]
async fn closing_an_issue_waiting_for_a_human_removes_the_worktree_it_kept() {
    let w = World::new();
    let tree = waiting_for_human(&w, false).await;
    assert!(tree.exists(), "waiting for a human, it is kept");
    let closed = w.app.issue_close("web", 1, "dropped").await.unwrap();
    assert!(!tree.exists(), "closed, it goes");
    assert_eq!(closed["abandoned"], "factorium/web-1-1-abandoned");
    assert_eq!(closed["comments"].as_array().unwrap().len(), 1, "{closed}");
}

#[tokio::test]
async fn closing_an_issue_with_no_worktree_left_says_nothing_of_one() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let closed = w.app.issue_close("web", 1, "dropped").await.unwrap();
    assert_eq!((closed["abandoned"].clone(), closed["comments"].clone()), (json!(null), json!([])));
}

#[tokio::test]
async fn doctor_finds_a_worktree_no_open_issue_owns_and_discard_fixes_it() {
    let w = World::new();
    let tree = left_by_a_close(&w).await;
    let key = |p: &std::path::Path| format!("worktree:{}", p.display());
    let real = tree.canonicalize().unwrap();
    let found = w.finding(&key(&tree)).await.or(w.finding(&key(&real)).await).expect("doctor names it");
    assert!(found["what"].as_str().unwrap().contains("web#1 is closed"), "{found}");
    assert!(found["fix"].as_str().unwrap().contains("factorium discard"), "{found}");

    let done = w.app.discard_orphaned(&tree).await.unwrap();
    assert_eq!(done["abandoned"], "factorium/web-1-1-abandoned");
    assert!(!tree.exists());
    let repo = w.dir.join("repos").join("web");
    assert_eq!(w.git(&repo, &["show", "factorium/web-1-1-abandoned:wip.txt"]).unwrap(), "half done");
    assert!(w.finding(&key(&tree)).await.is_none() && w.finding(&key(&real)).await.is_none(), "fixed");
    assert!(done["why"].as_str().unwrap().contains("web#1 is closed"), "{done}");
    // The close named the branch before it closed; a closed issue takes no more comments.
    let comments = w.app.issue_show("web", 1).await.unwrap()["comments"].clone();
    let last = comments.as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(last.contains("`factorium/web-1-1-abandoned`"), "{last}");
}

#[tokio::test]
async fn doctor_leaves_an_open_issue_s_worktrees_and_discard_refuses_them() {
    let w = World::new();
    let live = worktree(&w.working(CLAUDE, "t").await);
    let d = w.app.doctor().await.unwrap();
    let named = d["findings"].as_array().unwrap().iter().any(|f| f["key"].as_str().unwrap().starts_with("worktree:"));
    assert!(!named, "{d}");
    let refused = w.app.discard_orphaned(&live).await.unwrap_err();
    assert_eq!(refused.code, "not_orphaned");
    assert!(live.exists());

    // An older attempt's worktree, and one of an issue this database does not hold, are no one's.
    let repo = w.dir.join("repos").join("web");
    let tip = w.remote_tip("web", "proving");
    for (name, why) in [("web-1-0", "older than web#1's last, 1"), ("web-9-1", "web#9 is not an issue here")] {
        let at = w.dir.join("worktrees").join(name);
        let branch = format!("factorium/{name}");
        w.git(&repo, &["worktree", "add", "--quiet", "-b", &branch, &at.to_string_lossy(), &tip]).unwrap();
        let d = w.app.doctor().await.unwrap();
        let f = d["findings"].as_array().unwrap().iter().find(|f| f["what"].as_str().unwrap().contains(&branch));
        assert!(f.expect("named")["what"].as_str().unwrap().contains(why), "{d}");
        let done = w.app.discard_orphaned(&at).await.unwrap();
        assert_eq!(done["abandoned"], format!("{branch}-abandoned"));
        assert!(!at.exists());
    }
    // The older attempt's issue is open: a comment names the branch.
    let comments = w.app.issue_show("web", 1).await.unwrap()["comments"].clone();
    let said: Vec<&str> = comments.as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap()).collect();
    assert!(said.iter().any(|t| t.contains("`factorium/web-1-0-abandoned`")), "{said:?}");
}

#[tokio::test]
async fn starting_over_with_the_proving_tip_unread_discards_the_worktree_and_keeps_its_branch() {
    let w = World::new();
    let tree = waiting_for_human(&w, false).await;
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
