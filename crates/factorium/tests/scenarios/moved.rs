//! `issue move` (SPEC §4, §7): one issue handed to another host's
//! factorium over ssh. Checked there first, nothing changed here until that
//! passes; then its worker ends here, its worktree kept; its work goes to a
//! branch of its own on origin; it is filed there, and closed here.

use serde_json::json;

use crate::fixture::{World, comments, refused, worker, worktree};

const THERE: &str = "felix@there";
const BRANCH: &str = "factorium/move/web-1";

/// `web`'s issue 1, with a body and a comment of the owner's, a quote
/// in it, as ssh's shells must keep it; a worker at work on it if `started`.
async fn issue(w: &World, started: bool) {
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "Make the screen tests run", "Run `scripts/screen-tests.sh`.", false, &[]).await.unwrap();
    w.app.issue_comment("web", 1, "Use the small screen: it's quicker.").await.unwrap();
    if started {
        w.tick().await;
    }
}

/// What `web`'s issue 1 shows here, but its `spent` (which moves with the clock).
async fn shown(w: &World) -> serde_json::Value {
    let mut shown = w.app.issue_show("web", 1).await.unwrap();
    shown.as_object_mut().unwrap().remove("spent");
    shown
}

#[tokio::test]
async fn a_worker_moves_with_its_commits_and_its_changes_and_goes_on_there() {
    let (w, there) = (World::new(), World::new());
    issue(&w, true).await;
    let tree = worktree(&worker(&w.app).await);
    w.commit(&tree, "done.txt", "committed here");
    std::fs::write(tree.join("wip.txt"), "not committed").unwrap();
    there.checkout_of(&w, "web").await;
    there.reached_by(&w, THERE);

    let moved = w.app.issue_move("web", 1, THERE).await.unwrap();
    assert_eq!(moved["moved_to"], json!("felix@there web#1"), "{moved:#}");
    assert_eq!(moved["branch"], json!(BRANCH));

    let tip = w.remote_tip("web", BRANCH);
    assert_eq!(moved["commit"], json!(tip), "the branch on origin is the move's commit");
    let remote = w.dir.join("remotes").join("web.git");
    let at = |file: &str| w.git(&remote, &["show", &format!("{tip}:{file}")]);
    assert_eq!(at("done.txt").as_deref(), Ok("committed here"), "it holds the commits");
    assert_eq!(at("wip.txt").as_deref(), Ok("not committed"), "and the changes not committed");

    assert_eq!(w.app.workers(false).await.unwrap(), json!([]), "its worker ended");
    assert!(tree.join("wip.txt").exists(), "its worktree is kept");
    let status = w.git(&tree, &["status", "--porcelain"]).unwrap();
    assert_eq!(status, "?? wip.txt", "as it was: nothing committed in it");

    let here = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!(here["status"], json!("closed"));
    let said = comments(&here).last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(said.contains("`felix@there web#1`") && said.contains(BRANCH), "{said}");

    let filed = there.app.issue_show("web", 1).await.unwrap();
    assert_eq!(filed["status"], json!("ready"), "{filed:#}");
    assert_eq!(filed["title"], json!("Make the screen tests run"));
    let body = filed["body"].as_str().unwrap();
    for part in [
        "Run `scripts/screen-tests.sh`.",
        " web#1` by `factorium issue move`",
        &format!("`{BRANCH}`, at {tip}: merge it into your branch first"),
        "owner, ",
        "> Use the small screen: it's quicker.",
    ] {
        assert!(body.contains(part), "the body carries {part:?}: {body}");
    }
}

#[tokio::test]
async fn an_issue_no_worktree_holds_moves_with_no_branch() {
    let (w, there) = (World::new(), World::new());
    issue(&w, false).await;
    there.checkout_of(&w, "web").await;
    there.reached_by(&w, THERE);

    let moved = w.app.issue_move("web", 1, THERE).await.unwrap();
    assert_eq!(moved["branch"], json!(null), "{moved:#}");
    let remote = w.dir.join("remotes").join("web.git");
    assert!(w.git(&remote, &["rev-parse", "--verify", BRANCH]).is_err(), "no branch is pushed");
    let body = there.app.issue_show("web", 1).await.unwrap()["body"].as_str().unwrap().to_string();
    assert!(body.contains("It carried no work"), "{body}");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], json!("closed"));
}

#[tokio::test]
async fn a_move_the_other_host_cannot_take_is_refused_with_nothing_changed_here() {
    // There: no checkout `web`; a `web` of another origin; no host at all.
    let (none, other) = (World::new(), World::new());
    other.checkout("web").await;
    for (there, code) in [(Some(&none), "no_checkout_there"), (Some(&other), "other_origin"), (None, "ssh")] {
        let w = World::new();
        issue(&w, true).await;
        let tree = worktree(&worker(&w.app).await);
        w.commit(&tree, "done.txt", "committed here");
        if let Some(there) = there {
            there.reached_by(&w, THERE);
        }
        let before = shown(&w).await;
        refused(w.app.issue_move("web", 1, THERE).await, code);
        assert_eq!(shown(&w).await, before, "{code}: nothing changed here");
        assert_ne!(before["worker"], json!(null), "{code}: its worker works on");
        let remote = w.dir.join("remotes").join("web.git");
        assert!(w.git(&remote, &["rev-parse", "--verify", BRANCH]).is_err(), "{code}: no branch is pushed");
    }
    assert_eq!(other.app.issue_list(None, true).await.unwrap(), json!([]), "nothing was filed there");
}
