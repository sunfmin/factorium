//! `issue move` (SPEC §4, §7): one issue handed to another host's
//! factorium over ssh. Checked there first, nothing changed here until that
//! passes; then its worker ends here, its worktree kept; its work goes to a
//! branch of its own on origin; it is filed there, and closed here.

use factorium::host::Host;
use serde_json::json;

use crate::fixture::{World, comments, worker, worktree};

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
    // A move tried before left its branch on origin: this one replaces it.
    let repo = w.dir.join("repos").join("web");
    w.git(&repo, &["push", "--quiet", "origin", &format!("HEAD:refs/heads/{BRANCH}")]).unwrap();

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
    let out = std::process::Command::new("hostname").arg("-s").output().unwrap();
    let here = String::from_utf8_lossy(&out.stdout).trim().to_string();
    for part in [
        "Run `scripts/screen-tests.sh`.",
        &format!("Moved here from `{here} web#1` by `factorium issue move`"),
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
    w.app.issue_add("web", "first", "", false, &[]).await.unwrap();
    w.app.issue_block("web", 1, factorium_domain::issue::Blocker::Here(2), false).await.unwrap();
    there.checkout_of(&w, "web").await;
    there.reached_by(&w, THERE);

    let moved = w.app.issue_move("web", 1, THERE).await.unwrap();
    assert_eq!(moved["branch"], json!(null), "{moved:#}");
    let remote = w.dir.join("remotes").join("web.git");
    assert!(w.git(&remote, &["rev-parse", "--verify", BRANCH]).is_err(), "no branch is pushed");
    let body = there.app.issue_show("web", 1).await.unwrap()["body"].as_str().unwrap().to_string();
    assert!(body.contains("It carried no work"), "{body}");
    let stayed = "none of which travelled (its paths and numbers are that host's): blocked by #2.";
    assert!(body.contains(stayed), "{body}");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], json!("closed"));
}

#[tokio::test]
async fn a_move_the_other_host_cannot_take_is_refused_with_nothing_changed_here() {
    // There: no checkout `web`; a `web` of another origin; no host at all;
    // no ssh target, or an option in its stead.
    let (none, other) = (World::new(), World::new());
    other.checkout("web").await;
    let cases = [
        (Some(&none), THERE, "no_checkout_there", "felix@there's factorium has no checkout web (it lists: none)"),
        (Some(&other), THERE, "other_origin", "felix@there's web ("),
        (None, THERE, "ssh", "Could not resolve hostname felix@there"),
        (None, "", "bad_target", "names no ssh target"),
        (None, "-oProxyCommand=true", "bad_target", "names no ssh target"),
    ];
    for (there, target, code, said) in cases {
        let w = World::new();
        issue(&w, true).await;
        let tree = worktree(&worker(&w.app).await);
        w.commit(&tree, "done.txt", "committed here");
        if let Some(there) = there {
            there.reached_by(&w, target);
        }
        let before = shown(&w).await;
        let r = w.app.issue_move("web", 1, target).await.unwrap_err();
        assert!(r.code == code && r.message.contains(said), "{code}, {said:?}: {r}");
        assert_eq!(shown(&w).await, before, "{code}: nothing changed here");
        assert_ne!(before["worker"], json!(null), "{code}: its worker works on");
        let remote = w.dir.join("remotes").join("web.git");
        assert!(w.git(&remote, &["rev-parse", "--verify", BRANCH]).is_err(), "{code}: no branch is pushed");
    }
    assert_eq!(other.app.issue_list(None, true).await.unwrap(), json!([]), "nothing was filed there");
}

#[tokio::test]
async fn origins_that_name_one_repo_in_other_forms_are_one_and_others_are_not() {
    // Here's origin as stored, the `insteadOf` this host's ~/.gitconfig
    // rewrites it with, and there's: no issue is worked, so nothing is
    // pushed and no URL is reached.
    let rewrite = "[url \"ssh://git@example.com/\"]\n\tinsteadOf = https://example.com/\n";
    let cases = [
        ("https://example.com/felix/web.git", Some(rewrite), "https://example.com/felix/web.git", None),
        ("git@example.com:felix/web", None, "https://example.com/felix/web/", None),
        ("ssh://git@Example.com/felix/web.git", None, "git@example.com:felix/web.git", None),
        // A local path whose colon comes after a slash is no scp-like `host:path`.
        ("/srv/a:b/web.git", None, "file:///srv/a:b/web.git", None),
        ("https://example.com/felix/web.git", None, "https://example.org/felix/web.git", Some("other_origin")),
        ("https://example.com/felix/web.git", Some(rewrite), "https://example.com/ann/web.git", Some("other_origin")),
    ];
    for (here, config, theirs, refused) in cases {
        let (w, there) = (World::new(), World::new());
        issue(&w, false).await;
        there.checkout_of(&w, "web").await;
        there.reached_by(&w, THERE);
        w.git(&w.dir.join("repos").join("web"), &["remote", "set-url", "origin", here]).unwrap();
        there.git(&there.dir.join("repos").join("web"), &["remote", "set-url", "origin", theirs]).unwrap();
        if let Some(config) = config {
            let mut file = std::fs::OpenOptions::new().append(true).open(w.host.home().join(".gitconfig")).unwrap();
            std::io::Write::write_all(&mut file, config.as_bytes()).unwrap();
        }
        let moved = w.app.issue_move("web", 1, THERE).await;
        match refused {
            None => assert_eq!(moved.unwrap()["moved_to"], json!("felix@there web#1"), "{here} = {theirs}"),
            Some(code) => assert_eq!(moved.unwrap_err().code, code, "{here} ≠ {theirs}"),
        }
    }
}

#[tokio::test]
async fn an_issue_that_waits_for_a_person_here_waits_for_one_there_and_a_ready_one_arrives_ready() {
    let (w, there) = (World::new(), World::new());
    issue(&w, false).await;
    w.app.issue_add("web", "Ready to go", "", false, &[]).await.unwrap();
    w.app.issue_wait_for_human("web", 1, "slimfit found it too big: split it?").await.unwrap();
    there.checkout_of(&w, "web").await;
    there.reached_by(&w, THERE);

    w.app.issue_move("web", 1, THERE).await.unwrap();
    let filed = there.app.issue_show("web", 1).await.unwrap();
    assert_eq!(filed["status"], json!("waiting_for_human"), "{filed:#}");
    let out = std::process::Command::new("hostname").arg("-s").output().unwrap();
    let here = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let why = format!("it waited for a person at `{here} web#1`: slimfit found it too big: split it?");
    assert_eq!(filed["reason"], json!(why), "{filed:#}");
    let asks = there.app.doctor().await.unwrap()["asks"].clone();
    assert_eq!(asks[0]["issue"], json!("web#1"), "{asks:#}");
    assert_eq!(asks[0]["why"], json!(why), "{asks:#}");

    w.app.issue_move("web", 2, THERE).await.unwrap();
    assert_eq!(there.app.issue_show("web", 2).await.unwrap()["status"], json!("ready"));
}
