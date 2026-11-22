//! Kept worktrees when the proving tip cannot be read (SPEC §2.6): what a
//! worktree holds is unknown, so it is held. A start refuses and takes
//! nothing; a worker that ends keeps its worktree; start-over discards it,
//! keeping its branch.

use std::path::PathBuf;

use serde_json::json;

use crate::fixture::{CLAUDE, KeptTrace, World, comments, findings, id, keyed, worker, worktree};

/// The branch a comment of factorium's names, in backticks, which a git
/// branch of `web` must be.
fn named_branch(w: &World, text: &str) -> String {
    let branch = text.split('`').nth(1).unwrap_or_else(|| panic!("names no branch: {text}"));
    let repo = w.dir.join("repos").join("web");
    assert!(w.git(&repo, &["rev-parse", "--verify", &format!("refs/heads/{branch}")]).is_ok(), "{text}");
    branch.to_string()
}

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
    w.host.orca.fault("remove-refused", true);
    let refused = w.app.issue_close("web", 1, "not needed").await.unwrap_err();
    assert_eq!(refused.code, "orca", "{refused:?}");
    w.host.orca.fault("remove-refused", false);
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
    let last = comments(&closed).last().unwrap().clone();
    assert_eq!(last["by"], "factorium", "{last}");
    assert_eq!(named_branch(&w, last["text"].as_str().unwrap()), old, "{last}");
}

#[tokio::test]
async fn closing_an_issue_waiting_for_a_human_removes_the_worktree_it_kept() {
    let w = World::new();
    let tree = waiting_for_human(&w, false).await;
    assert!(tree.exists(), "waiting for a human, it is kept");
    let closed = w.app.issue_close("web", 1, "dropped").await.unwrap();
    assert!(!tree.exists(), "closed, it goes");
    assert_eq!(closed["abandoned"], "factorium/web-1-1-abandoned");
    assert_eq!(comments(&closed).len(), 1, "{closed}");
}

#[tokio::test]
async fn closing_an_issue_with_no_worktree_left_says_nothing_of_one() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let closed = w.app.issue_close("web", 1, "dropped").await.unwrap();
    assert_eq!((closed["abandoned"].clone(), closed["comments"].clone()), (json!(null), json!([])));

    // An issue of a checkout off the list closes all the same.
    w.app.issue_add("web", "u", "", true, &[]).await.unwrap();
    w.app.checkout_remove("web").await.unwrap();
    assert_eq!(w.app.issue_close("web", 2, "dropped").await.unwrap()["status"], "closed");
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
    let last = w.comments("web", 1).await.last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(last.contains("`factorium/web-1-1-abandoned`"), "{last}");
}

#[tokio::test]
async fn doctor_leaves_an_open_issue_s_worktrees_and_discard_refuses_them() {
    let w = World::new();
    let live = worktree(&w.working(CLAUDE, "t").await);
    let d = w.app.doctor().await.unwrap();
    let named = !keyed(&d, "worktree:").is_empty();
    assert!(!named, "{d}");
    let refused = w.app.discard_orphaned(&live).await.unwrap_err();
    assert_eq!(refused.code, "not_orphaned");
    assert!(live.exists());

    // An older attempt's worktree, and one of an issue this database does not hold, are no one's.
    let repo = w.dir.join("repos").join("web");
    let tip = w.remote_tip("web", "proving");
    for (name, why) in [
        ("web-1-0", "attempt 0 is neither web#1's last started (1) nor starting"),
        ("web-9-1", "web#9 is not an issue here"),
    ] {
        let at = w.dir.join("worktrees").join(name);
        let branch = format!("factorium/{name}");
        w.git(&repo, &["worktree", "add", "--quiet", "-b", &branch, &at.to_string_lossy(), &tip]).unwrap();
        let d = w.app.doctor().await.unwrap();
        let f = findings(&d).iter().find(|f| f["what"].as_str().unwrap().contains(&branch));
        assert!(f.expect("named")["what"].as_str().unwrap().contains(why), "{d}");
        let done = w.app.discard_orphaned(&at).await.unwrap();
        assert_eq!(done["abandoned"], format!("{branch}-abandoned"));
        assert!(!at.exists());
    }
    // The older attempt's issue is open: a comment names the branch.
    let comments = w.comments("web", 1).await;
    let said: Vec<&str> = comments.iter().map(|c| c["text"].as_str().unwrap()).collect();
    let older = said.iter().find(|t| t.contains("attempt 0")).expect("named");
    assert_eq!(named_branch(&w, older), "factorium/web-1-0-abandoned", "{said:?}");
}

#[tokio::test]
async fn a_kept_worktree_a_later_attempt_resumed_is_that_attempt_s_and_not_orphaned() {
    let w = World::new();
    let tree = waiting_for_human(&w, true).await;
    w.app.issue_ready("web", 1).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worker"], json!(factorium_domain::worker_id("web", 1, 2)));
    assert_eq!(again["worktree"], json!(tree), "attempt 2 goes on in attempt 1's worktree, on its branch");
    let d = w.app.doctor().await.unwrap();
    assert!(keyed(&d, "worktree:").is_empty(), "{d}");
    let refused = w.app.discard_orphaned(&tree).await.unwrap_err();
    assert_eq!(refused.code, "not_orphaned");
    assert!(tree.exists());
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
    let said = comments(&over).last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(said.starts_with("Started over: attempt 1's worktree"), "{said}");
    assert_eq!(named_branch(&w, &said), "factorium/web-1-1-abandoned");
    away.back();

    w.app.issue_ready("web", 1).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worktree"], json!(w.dir.join("worktrees").join("web-1-2")), "a new worktree");
}

#[tokio::test]
async fn discarding_an_older_attempt_s_worktree_names_that_attempt_and_its_kept_branch() {
    let w = World::new();
    // Attempt 1 held nothing of its own: attempt 2 starts afresh, and attempt 1's worktree is left behind.
    let old = waiting_for_human(&w, false).await;
    w.app.issue_ready("web", 1).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worker"], json!(factorium_domain::worker_id("web", 1, 2)));
    assert!(old.exists());
    w.app.discard_orphaned(&old).await.unwrap();
    let last = w.comments("web", 1).await.last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(last.starts_with("Discarded attempt 1's worktree"), "{last}");
    assert_eq!(named_branch(&w, &last), "factorium/web-1-1-abandoned");
}

#[tokio::test]
async fn a_start_undone_with_the_proving_tip_unread_leaves_its_worktree_to_doctor() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    // Its session does not open, and by then the proving tip cannot be read:
    // what its new worktree holds is unknown, so the undone start keeps it.
    let (at, away) = (w.dir.join("remotes").join("web.git"), w.dir.join("remotes").join("web.git.away"));
    let (from, to) = (at.clone(), away.clone());
    *w.host.orca.while_opening.lock().unwrap() = Some(Box::new(move |_: &str| {
        if from.exists() {
            std::fs::rename(&from, &to).unwrap()
        }
    }));
    *w.host.orca.refuse_open.lock().unwrap() = true;
    assert!(w.app.tick().await.is_err(), "the session does not open");
    *w.host.orca.while_opening.lock().unwrap() = None;
    std::fs::rename(&away, &at).unwrap();
    let tree = w.dir.join("worktrees").join("web-1-1");
    assert!(tree.exists(), "held");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["worker"], json!(null), "the issue is free");

    // No attempt started: doctor names it all the same (factorium#187), and discard fixes it.
    let real = tree.canonicalize().unwrap();
    let found = w.finding(&format!("worktree:{}", real.display())).await;
    let found = found.or(w.finding(&format!("worktree:{}", tree.display())).await).expect("doctor names it");
    assert!(
        found["what"].as_str().unwrap().contains("attempt 1 is neither web#1's last started (none) nor starting"),
        "{found}"
    );
    assert_eq!(w.app.discard_orphaned(&tree).await.unwrap()["abandoned"], "factorium/web-1-1-abandoned");
    assert!(!tree.exists());
}

/// Worker `wk` writes token `n` (`w<n>.txt`) in its worktree, not committed.
fn writes(wk: &serde_json::Value, n: u64) {
    std::fs::write(worktree(wk).join(format!("w{n}.txt")), "x").unwrap();
}

/// Worker `wk` commits what its worktree holds.
fn commits(w: &World, wk: &serde_json::Value) {
    w.git(&worktree(wk), &["add", "-A"]).unwrap();
    w.git(&worktree(wk), &["commit", "--quiet", "-m", "work"]).unwrap();
}

/// A worktree's life, each step recorded as the model's state
/// (models/Worktrees.tla; the gate checks the kept run,
/// models/worktrees-run.jsonl): attempt 1 works, ends kept; attempt 2
/// resumes it, ends; with the tip unread, start-over discards it, keeping
/// its branch; attempt 3 starts afresh and lands, and its worktree goes.
#[tokio::test]
async fn a_worktree_s_life_from_start_to_close_is_a_behaviour_of_its_model() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let mut t = KeptTrace::default();
    w.tick().await;
    t.step(&w, "start").await;
    let first = worker(&w.app).await;
    writes(&first, 1);
    t.step(&w, "edit").await;
    commits(&w, &first);
    t.step(&w, "commit").await;
    writes(&first, 2);
    t.step(&w, "edit").await;
    w.app.issue_wait_for_human("web", 1, "later").await.unwrap();
    t.step(&w, "ends").await;
    w.app.issue_ready("web", 1).await.unwrap();
    t.step(&w, "ready").await;
    let again = w.tick().await.started.unwrap();
    t.step(&w, "start").await;
    assert_eq!(again["worktree"], json!(worktree(&first)), "attempt 2 resumes attempt 1's worktree");
    w.app.issue_wait_for_human("web", 1, "later").await.unwrap();
    t.step(&w, "ends").await;
    let away = Unreachable::web(&w);
    t.step(&w, "tip").await;
    w.app.issue_start_over("web", 1, "wrong approach").await.unwrap();
    t.step(&w, "start-over").await;
    away.back();
    t.step(&w, "tip").await;
    w.app.issue_ready("web", 1).await.unwrap();
    t.step(&w, "ready").await;
    w.tick().await;
    t.step(&w, "start").await;
    let third = worker(&w.app).await;
    writes(&third, 3);
    t.step(&w, "edit").await;
    commits(&w, &third);
    t.step(&w, "commit").await;
    w.app.worker_done(id(&third)).await.unwrap();
    w.tick().await;
    t.step(&w, "land").await;
    assert!(!worktree(&third).exists(), "landed, it goes");
}

#[tokio::test]
async fn a_start_under_way_is_no_doctor_finding() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    // doctor, as another process, while the start opens its session: its worktree is made, its worker not yet recorded.
    let found = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (db, host, into) = (w.db.clone(), std::sync::Arc::downgrade(&w.host), found.clone());
    *w.host.orca.while_opening.lock().unwrap() = Some(Box::new(move |_: &str| {
        let (db, host) = (db.clone(), host.upgrade().unwrap());
        let doctor = std::thread::spawn(move || {
            let app = factorium::App::open(&db, "owner", host).unwrap();
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(app.doctor()).unwrap()
        });
        into.lock().unwrap().push(doctor.join().unwrap());
    }));
    w.tick().await.started.unwrap();
    *w.host.orca.while_opening.lock().unwrap() = None;
    let found = found.lock().unwrap();
    assert!(!found.is_empty(), "doctor ran during the start");
    for d in found.iter() {
        assert!(keyed(d, "worktree:").is_empty(), "{d}");
    }
}

#[tokio::test]
async fn a_later_attempt_s_start_is_no_finding_while_under_way_and_one_once_undone() {
    let w = World::new();
    worktree(&w.working(CLAUDE, "t").await);
    w.app.issue_wait_for_human("web", 1, "later").await.unwrap();
    w.app.issue_ready("web", 1).await.unwrap();
    // Attempt 2's session does not open: doctor runs, as another process, while
    // it opens; then the proving tip cannot be read, so its undone start keeps its worktree.
    let found = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (db, host, into) = (w.db.clone(), std::sync::Arc::downgrade(&w.host), found.clone());
    let (at, away) = (w.dir.join("remotes").join("web.git"), w.dir.join("remotes").join("web.git.away"));
    let (from, to) = (at.clone(), away.clone());
    *w.host.orca.while_opening.lock().unwrap() = Some(Box::new(move |_: &str| {
        let (db, host) = (db.clone(), host.upgrade().unwrap());
        let doctor = std::thread::spawn(move || {
            let app = factorium::App::open(&db, "owner", host).unwrap();
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(app.doctor()).unwrap()
        });
        into.lock().unwrap().push(doctor.join().unwrap());
        if from.exists() {
            std::fs::rename(&from, &to).unwrap();
        }
    }));
    *w.host.orca.refuse_open.lock().unwrap() = true;
    assert!(w.app.tick().await.is_err(), "the session does not open");
    *w.host.orca.while_opening.lock().unwrap() = None;
    std::fs::rename(&away, &at).unwrap();
    let second = w.dir.join("worktrees").join("web-1-2");
    assert!(second.exists(), "held");
    let named = |d: &serde_json::Value| keyed(d, "worktree:").into_iter().filter(|k| k.ends_with("web-1-2")).count();
    let during = found.lock().unwrap().clone();
    assert!(
        !during.is_empty() && during.iter().all(|d| named(d) == 0),
        "under way, attempt 2's is no finding: {during:?}"
    );
    let d = w.app.doctor().await.unwrap();
    let f = findings(&d).iter().find(|f| f["key"].as_str().unwrap().ends_with("web-1-2")).expect("undone, it is one");
    assert!(f["what"].as_str().unwrap().contains("attempt 2 is neither web#1's last started (1) nor starting"), "{f}");
}

/// Runs one tick of a daemon (its own App, on `w`'s database) that dies
/// between Orca's new worktree and the start's record: its fake Orca's
/// hook panics as its terminal opens, `then` run first.
fn a_daemon_dies_mid_start(w: &World, then: impl FnMut() + Send + 'static) {
    let mut then = then;
    *w.host.orca.while_opening.lock().unwrap() = Some(Box::new(move |_: &str| {
        then();
        panic!("the daemon dies");
    }));
    let (db, host) = (w.db.clone(), w.host.clone());
    let dying = std::thread::spawn(move || {
        let app = factorium::App::open(&db, "owner", host).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(app.tick())
    });
    assert!(dying.join().is_err(), "it died");
    *w.host.orca.while_opening.lock().unwrap() = None;
}

#[tokio::test]
async fn a_daemon_that_died_mid_start_frees_its_issue_when_it_starts_again() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    a_daemon_dies_mid_start(&w, || {});
    let tree = w.dir.join("worktrees").join("web-1-1");
    assert!(tree.exists(), "its worktree was made");
    assert_ne!(w.app.issue_show("web", 1).await.unwrap()["worker"], json!(null), "taken, never started");

    // The daemon starts again: the issue is free, its worktree (holding nothing) goes, and it starts again.
    let restarted = w.run();
    restarted.daemon_start().await.unwrap();
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["worker"], json!(null), "the issue is free");
    assert!(!tree.exists(), "it held nothing: it goes");
    let again = restarted.tick().await.unwrap().started.expect("it starts again");
    assert_eq!(again["worker"], json!(factorium_domain::worker_id("web", 1, 2)));
}

#[tokio::test]
async fn a_daemon_that_died_mid_start_leaves_its_held_worktree_to_doctor() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    // The worktree is made; then the proving tip cannot be read, and the daemon dies.
    let (at, away) = (w.dir.join("remotes").join("web.git"), w.dir.join("remotes").join("web.git.away"));
    let (from, to) = (at.clone(), away.clone());
    a_daemon_dies_mid_start(&w, move || std::fs::rename(&from, &to).unwrap());
    let restarted = w.run();
    restarted.daemon_start().await.unwrap();
    let tree = w.dir.join("worktrees").join("web-1-1");
    assert!(tree.exists(), "with the tip unread, held");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["worker"], json!(null), "the issue is free");
    let d = w.app.doctor().await.unwrap();
    let f = findings(&d).iter().find(|f| f["key"].as_str().unwrap().ends_with("web-1-1")).expect("a finding");
    assert!(
        f["what"].as_str().unwrap().contains("attempt 1 is neither web#1's last started (none) nor starting"),
        "{f}"
    );
    std::fs::rename(&away, &at).unwrap();
    assert!(restarted.tick().await.unwrap().started.is_some(), "it starts again");
}

/// factorium#189, behaviour 1: a tick, as another process, between
/// start-over's read (no worker, a kept worktree) and its act (the
/// worktree's removal). The claim start-over holds keeps the tick from
/// taking the issue, so no worker resumes the worktree it removes; the
/// next tick starts afresh, and the work is on the kept branch.
#[tokio::test]
async fn a_tick_during_a_start_over_starts_no_worker_in_the_worktree_it_removes() {
    let w = World::new();
    let tree = waiting_for_human(&w, true).await;
    w.app.issue_ready("web", 1).await.unwrap();
    let during = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (db, host, into) = (w.db.clone(), w.host.clone(), during.clone());
    *w.host.orca.while_removing.lock().unwrap() = Some(Box::new(move |_: &str| {
        let started =
            crate::fixture::another(&db, &host, |app| async move { app.tick().await.map(|t| t.started) }).unwrap();
        into.lock().unwrap().push(started.map_err(|r| r.to_string()));
    }));
    w.app.issue_start_over("web", 1, "wrong approach").await.unwrap();
    *w.host.orca.while_removing.lock().unwrap() = None;
    let during = during.lock().unwrap().clone();
    assert!(!during.is_empty(), "a tick ran during the start-over");
    assert!(during.iter().all(|t| matches!(t, Ok(None))), "it started no worker: {during:?}");
    assert!(!tree.exists(), "start-over removed it");
    let next = w.tick().await.started.expect("the next tick starts");
    assert_ne!(next["worktree"], json!(tree), "afresh, not in the removed worktree");
    assert!(PathBuf::from(next["worktree"].as_str().unwrap()).exists());
    let repo = w.dir.join("repos").join("web");
    assert!(w.git(&repo, &["show", "factorium/web-1-1-abandoned:kept.txt"]).is_ok(), "the work is kept");
}

/// factorium#189, behaviour 2: `issue close`, as another process, while a
/// tick's start opens its worker's session (its worktree made, its worker
/// not yet recorded). The start's claim refuses the close, so no worker is
/// left at work on a closed issue; once the start is done, the close runs.
#[tokio::test]
async fn a_close_during_a_start_is_refused_and_leaves_no_worker_on_a_closed_issue() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let during = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (db, host, into) = (w.db.clone(), w.host.clone(), during.clone());
    // Once: its worker's terminal, the first the start opens.
    *w.host.orca.while_opening.lock().unwrap() = Some(Box::new(move |_: &str| {
        if !into.lock().unwrap().is_empty() {
            return;
        }
        let closed =
            crate::fixture::another(&db, &host, |app| async move { app.issue_close("web", 1, "no").await }).unwrap();
        into.lock().unwrap().push(closed.map_err(|r| (r.code, r.message)));
    }));
    w.tick().await.started.expect("the start goes on");
    *w.host.orca.while_opening.lock().unwrap() = None;
    let during = during.lock().unwrap().clone();
    assert_eq!(during.len(), 1, "a close ran during the start");
    let (code, why) = during[0].as_ref().expect_err("refused");
    assert_eq!(code, "claimed");
    assert!(why.starts_with("start is under way on it (process "), "it names the start: {why}");
    let wk = worker(&w.app).await;
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], json!("ready"), "{wk}");
    w.app.issue_close("web", 1, "no").await.unwrap();
    assert!(w.app.workers(false).await.unwrap().as_array().unwrap().is_empty(), "closed, its worker ended");
    assert!(!worktree(&wk).exists(), "and its worktree is removed");
}

/// A start-over that dies between its read and its act leaves its claim:
/// while its process is alive (as far as the tick can tell) the issue
/// starts no worker; once it is gone, the next tick clears the claim and
/// resumes the worktree the start-over never removed.
#[tokio::test]
async fn a_claim_whose_process_died_is_cleared_by_the_next_tick() {
    let w = World::new();
    let tree = waiting_for_human(&w, true).await;
    w.app.issue_ready("web", 1).await.unwrap();
    *w.host.orca.while_removing.lock().unwrap() = Some(Box::new(|_: &str| panic!("the start-over's process dies")));
    let (db, host) = (w.db.clone(), w.host.clone());
    let died = crate::fixture::another(&db, &host, |app| async move { app.issue_start_over("web", 1, "x").await });
    assert!(died.is_err(), "it died");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["claim"]["by"], json!("start_over"));
    assert_eq!(w.tick().await.started, None, "claimed, it does not start");
    *w.host.gone.lock().unwrap() = true;
    let started = w.tick().await.started.expect("its claim cleared, it starts");
    *w.host.gone.lock().unwrap() = false;
    assert_eq!(started["worktree"], json!(tree), "in the worktree the start-over never removed");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["claim"], json!(null));
}

#[tokio::test]
async fn a_daemon_that_died_mid_start_whose_worktree_is_gone_frees_its_issue_with_none() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    a_daemon_dies_mid_start(&w, || {});
    let tree = w.dir.join("worktrees").join("web-1-1");
    w.git(&w.dir.join("repos").join("web"), &["worktree", "remove", "--force", &tree.to_string_lossy()]).unwrap();

    let restarted = w.run();
    let freed = restarted.daemon_start().await.unwrap()["freed"].clone();
    assert_eq!(freed[0]["issue"], "web#1", "{freed}");
    assert_eq!(freed[0]["worktree"], json!(null), "no worktree left to release: {freed}");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["worker"], json!(null), "the issue is free");
    assert!(restarted.tick().await.unwrap().started.is_some(), "it starts again");
}

/// `web` with attempt 1's worktree left behind by attempt 2 (as
/// [`discarding_an_older_attempt_s_worktree_names_that_attempt_and_its_kept_branch`]),
/// holding a git worktree of its own at `.slimfit/out/before`, as slimfit's
/// comparison makes: detached at the proving tip attempt 1 started from.
/// Answers attempt 1's worktree and the nested one.
async fn left_holding_a_comparison(w: &World) -> (PathBuf, PathBuf) {
    let old = waiting_for_human(w, false).await;
    let inner = old.join(".slimfit").join("out").join("before");
    std::fs::create_dir_all(inner.parent().unwrap()).unwrap();
    // The repo ignores slimfit's output, as this one's `.gitignore` does.
    let exclude = w.dir.join("repos").join("web").join(".git").join("info").join("exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, ".slimfit/out\n").unwrap();
    let at = w.git(&old, &["rev-parse", "HEAD"]).unwrap();
    w.git(&old, &["worktree", "add", "--quiet", "--detach", &inner.to_string_lossy(), at.trim()]).unwrap();
    w.app.issue_ready("web", 1).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worker"], json!(factorium_domain::worker_id("web", 1, 2)));
    (old, inner)
}

#[tokio::test]
async fn discard_removes_a_clean_detached_nested_worktree_at_a_proving_commit_in_one_call() {
    let w = World::new();
    let (old, inner) = left_holding_a_comparison(&w).await;
    let discarded = w.app.discard_orphaned(&old).await.unwrap();
    assert_eq!(discarded["abandoned"], "factorium/web-1-1-abandoned", "{discarded}");
    assert!(!old.exists() && !inner.exists(), "both go");
    let repo = w.dir.join("repos").join("web");
    let listed = w.git(&repo, &["worktree", "list", "--porcelain"]).unwrap();
    assert!(!listed.contains(".slimfit"), "git forgets the nested one: {listed}");
}

#[tokio::test]
async fn a_nested_worktree_with_changes_not_committed_is_kept_and_doctor_names_it() {
    let w = World::new();
    let (old, inner) = left_holding_a_comparison(&w).await;
    std::fs::write(inner.join("mine.txt"), "work").unwrap();
    let refused = w.app.discard_orphaned(&old).await.unwrap_err();
    assert_eq!(refused.code, "nested_held", "{refused:?}");
    assert!(refused.message.contains(".slimfit/out/before: it has changes not committed"), "{refused:?}");
    assert!(inner.join("mine.txt").exists() && old.exists(), "nothing is removed");
    let repo = w.dir.join("repos").join("web");
    assert!(
        w.git(&repo, &["rev-parse", "--verify", "refs/heads/factorium/web-1-1-abandoned"]).is_err(),
        "nothing changed"
    );

    let real = old.canonicalize().unwrap();
    let found = w.finding(&format!("worktree:{}", real.display())).await;
    let found = found.or(w.finding(&format!("worktree:{}", old.display())).await).expect("doctor names it");
    let what = found["what"].as_str().unwrap();
    assert!(what.contains(".slimfit/out/before: it has changes not committed"), "{found}");

    // Committed, it is work on no landed line; on a branch, that branch's.
    let landed = w.git(&inner, &["rev-parse", "HEAD"]).unwrap();
    w.commit(&inner, "mine.txt", "work");
    let refused = w.app.discard_orphaned(&old).await.unwrap_err();
    assert!(refused.message.contains("before: its HEAD is on neither proving nor main"), "{refused:?}");
    w.git(&inner, &["switch", "--quiet", "-c", "mine"]).unwrap();
    let refused = w.app.discard_orphaned(&old).await.unwrap_err();
    assert!(refused.message.contains("before: it is on branch `mine`, not detached"), "{refused:?}");

    // Back at the landed commit, detached and clean, discard goes through.
    w.git(&inner, &["switch", "--quiet", "--detach", landed.trim()]).unwrap();
    w.app.discard_orphaned(&old).await.unwrap();
    assert!(!old.exists());
}
