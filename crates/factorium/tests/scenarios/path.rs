//! The issue's path: a ready issue gets a worker in its own worktree; its
//! `done` lands it on the proving branch through the fast gate; the issue
//! closes and the worker ends; the base is promoted and a version cut.

use factorium_domain::issue::Wait;
use serde_json::json;

use crate::fixture::{CLAUDE, World, comments, findings, id, refused, worker, worktree};

#[tokio::test]
async fn an_issue_goes_from_ready_to_landed_promoted_and_cut() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "Add a greeting", "Say hello.", false, &[]).await.unwrap();
    let base = w.remote_tip("web", "proving");

    let t = w.tick().await;
    let started = t.started.expect("a worker started");
    assert_eq!(started["issue"], "web#1");
    let wk = worker(&w.app).await;
    let tree = worktree(&wk);
    assert_eq!(w.git(&tree, &["rev-parse", "HEAD"]).unwrap(), base, "branched from the proving tip");
    let branch = w.git(&tree, &["branch", "--show-current"]).unwrap();
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["worker"], wk["worker"]);
    let term = w.terminal(wk["terminal"].as_str().unwrap());
    assert_eq!(term.worktree, tree);
    assert!(term.line.contains("claude --dangerously-skip-permissions"), "{}", term.line);
    let brief = std::fs::read_to_string(w.data().brief("web-1-1")).unwrap();
    assert!(
        brief.contains("Add a greeting") && brief.contains("Say hello.") && brief.contains("factorium worker done")
    );
    assert!(term.line.contains("web-1-1.md"), "the brief is handed at start: {}", term.line);

    // A second tick starts nothing more: the issue is held.
    assert!(w.tick().await.started.is_none());

    w.commit(&tree, "hello.txt", "hello\n");
    let gate = w.app.worker_gate(id(&wk)).await.unwrap();
    assert_eq!((gate["green"].clone(), gate["ran"].clone()), (json!(true), json!(null)));
    assert_eq!(w.app.worker_gate(id(&wk)).await.unwrap()["ran"], false, "a green tree is not gated twice");
    w.app.worker_done(id(&wk), None).await.unwrap();
    assert_eq!(worker(&w.app).await["status"], "landing");

    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1");
    let landed = t.landed[0]["commit"].as_str().unwrap().to_string();
    assert_eq!(w.remote_tip("web", "proving"), landed);
    let issue = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!((issue["status"].clone(), issue["worker"].clone()), (json!("closed"), json!(null)));
    assert!(issue["reason"].as_str().unwrap().contains(&landed));
    assert_eq!(w.app.workers(false).await.unwrap()["workers"], json!([]));
    assert!(crate::fixture::closed(&w.app, "term-1").await, "its session ends");
    assert!(!tree.exists(), "its worktree goes");
    let left = w.git(&web, &["branch", "--list", &branch]).unwrap();
    assert!(left.trim().is_empty(), "its branch goes: {branch}");

    // Idle, the same tick promoted and cut.
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"));
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut");
    assert_eq!(cut["version"], "v0.1.0");
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.0"]).is_ok());
    // The first cut installs it, Homebrew is asked what it has once done,
    // where its keg is, to check PATH links into it (it has no programs),
    // and where its factorium is, to prove it on the database (it has none).
    let installs =
        ["update --quiet", "list --formula web", "install web", "list --versions web", "--cellar web", "--prefix web"];
    assert_eq!(w.host.brewed(), installs);
}

#[tokio::test]
async fn done_takes_only_committed_work_of_its_own() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let tree = worktree(&wk);
    refused(w.app.worker_done(id(&wk), None).await, "nothing");
    std::fs::write(tree.join("loose"), "x").unwrap();
    refused(w.app.worker_done(id(&wk), None).await, "dirty");
    w.git(&tree, &["add", "-A"]).unwrap();
    w.git(&tree, &["commit", "-qm", "x"]).unwrap();
    // Landing takes its branch: on another, it is refused.
    w.git(&tree, &["checkout", "-qb", "elsewhere"]).unwrap();
    refused(w.app.worker_done(id(&wk), None).await, "off_branch");
    w.git(&tree, &["checkout", "-q", "-"]).unwrap();
    // Commits land: finishing without a landing refuses them, so no work is dropped.
    refused(w.app.worker_done(id(&wk), Some("published")).await, "has_commits");
    assert_eq!(worker(&w.app).await["status"], "working");
    w.app.worker_done(id(&wk), None).await.unwrap();
}

/// An issue whose work changes no code (a publish, a measurement) finishes
/// without a landing: its evidence closes it, and no gate runs.
#[tokio::test]
async fn done_without_landing_closes_an_issue_whose_work_changes_no_code() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let tree = worktree(&wk);
    let proving = w.remote_tip("web", "proving");
    let nothing = w.app.worker_done(id(&wk), None).await.unwrap_err();
    assert!(nothing.to_string().contains("--no-landing"), "the refusal names the way: {nothing}");
    refused(w.app.worker_done(id(&wk), Some(" ")).await, "no_evidence");

    let evidence = "published from proving's tip: 30 files, 0 wrong";
    w.app.worker_done(id(&wk), Some(evidence)).await.unwrap();
    assert_eq!(worker(&w.app).await["status"], "finishing");

    let t = w.tick().await;
    assert!(t.landed.is_empty(), "nothing lands: {:?}", t.landed);
    assert_eq!(t.finished[0]["finished"], "web#1");
    assert_eq!(w.remote_tip("web", "proving"), proving, "proving does not move");
    let issue = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!((issue["status"].clone(), issue["worker"].clone()), (json!("closed"), json!(null)));
    assert!(issue["reason"].as_str().unwrap().contains(evidence), "{issue}");
    assert_eq!(w.app.workers(false).await.unwrap()["workers"], json!([]));
    assert!(crate::fixture::closed(&w.app, "term-1").await, "its session ends");
    assert!(!tree.exists(), "its worktree goes");
}

#[tokio::test]
async fn a_red_landing_tells_the_worker_why_and_the_second_red_goes_to_the_watch() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let tree = worktree(&wk);
    let proving = w.remote_tip("web", "proving");

    w.commit(&tree, "RED", "x");
    w.app.worker_done(id(&wk), None).await.unwrap();
    let t = w.tick().await;
    assert!(t.landed[0]["red"].as_str().unwrap().contains("fast gate is red"));
    assert_eq!(t.landed[0]["status"], "working", "back to work");
    assert_eq!(w.remote_tip("web", "proving"), proving, "nothing red lands");
    let again = worker(&w.app).await;
    assert_eq!(again["status"], "working");
    let told = w.terminal(wk["terminal"].as_str().unwrap()).sent;
    assert!(told.last().unwrap().contains("landing was red"), "{told:?}");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], "working", "its worker is back on it");

    w.app.worker_done(id(&wk), None).await.unwrap();
    w.tick().await;
    let waiting = worker(&w.app).await;
    assert_eq!(waiting["status"], "waiting");
    assert!(waiting["why"].as_str().unwrap().starts_with("red twice"));
    let doctor = w.app.doctor().await.unwrap();
    assert!(findings(&doctor)[0]["what"].as_str().unwrap().contains("red twice"));
}

#[tokio::test]
async fn a_red_landing_then_a_fix_lands() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let tree = worktree(&wk);
    w.commit(&tree, "RED", "x");
    w.app.worker_done(id(&wk), None).await.unwrap();
    w.tick().await;
    w.git(&tree, &["rm", "-q", "RED"]).unwrap();
    w.git(&tree, &["commit", "-qm", "fix"]).unwrap();
    w.app.worker_done(id(&wk), None).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
}

/// Merges the proving tip into the worker's branch at `tree`, its own side
/// winning where they conflict: what a worker told to merge it does.
fn merge_proving(w: &World, tree: &std::path::Path) {
    w.git(tree, &["fetch", "-q", "origin"]).unwrap();
    w.git(tree, &["merge", "-q", "--no-edit", "-X", "ours", "origin/proving"]).unwrap();
}

/// A worker on `web` whose branch does not merge onto proving: it and the
/// proving tip both changed README. Its landing is red once.
async fn red_on_moved_proving(w: &World) -> serde_json::Value {
    let wk = w.working(CLAUDE, "t").await;
    let tree = worktree(&wk);
    w.commit(&tree, "README", "mine\n");
    w.push_by_hand("web", "proving", "README", "theirs 1\n");
    w.app.worker_done(id(&wk), None).await.unwrap();
    let t = w.tick().await;
    assert!(t.landed[0]["red"].as_str().unwrap().contains("does not merge"), "{:?}", t.landed);
    assert_eq!(worker(&w.app).await["status"], "working");
    wk
}

#[tokio::test]
async fn a_branch_that_merged_the_red_tip_and_conflicts_on_a_newer_one_is_told_again_not_sent_to_the_watch() {
    let w = World::new();
    let wk = red_on_moved_proving(&w).await;
    let tree = worktree(&wk);
    merge_proving(&w, &tree);
    // Another landing moves proving before this one's second try.
    w.push_by_hand("web", "proving", "README", "theirs 2\n");

    w.app.worker_done(id(&wk), None).await.unwrap();
    let t = w.tick().await;
    let red = t.landed[0]["red"].as_str().unwrap();
    assert!(red.contains("does not merge") && red.contains("Proving moved since"), "{red}");
    assert!(red.contains("write README") && red.contains("conflicts in: README"), "what landed, what conflicts: {red}");
    assert_eq!(worker(&w.app).await["status"], "working", "told as for a first red");
    let told = w.terminal(wk["terminal"].as_str().unwrap()).sent;
    assert!(told.last().unwrap().contains("Proving moved since"), "{told:?}");
    assert!(findings(&w.app.doctor().await.unwrap()).is_empty(), "no watch round");

    // It merges that tip too, and lands.
    merge_proving(&w, &tree);
    w.app.worker_done(id(&wk), None).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
}

#[tokio::test]
async fn a_second_merge_red_on_the_same_tip_or_without_the_red_tip_merged_goes_to_the_watch() {
    // The same tip: nothing merged, proving did not move.
    let w = World::new();
    let wk = red_on_moved_proving(&w).await;
    w.commit(&worktree(&wk), "other", "x");
    w.app.worker_done(id(&wk), None).await.unwrap();
    w.tick().await;
    let waiting = worker(&w.app).await;
    assert_eq!(waiting["status"], "waiting");
    assert!(waiting["why"].as_str().unwrap().starts_with("red twice"));

    // Proving moved, but the branch never merged the red's tip.
    let w = World::new();
    let wk = red_on_moved_proving(&w).await;
    w.push_by_hand("web", "proving", "README", "theirs 2\n");
    w.app.worker_done(id(&wk), None).await.unwrap();
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "waiting", "the branch does not hold the red's tip");

    // The red's tip merged, then the gate is red: it counts.
    let w = World::new();
    let wk = red_on_moved_proving(&w).await;
    let tree = worktree(&wk);
    merge_proving(&w, &tree);
    w.commit(&tree, "RED", "x");
    w.app.worker_done(id(&wk), None).await.unwrap();
    let t = w.tick().await;
    assert!(t.landed[0]["red"].as_str().unwrap().contains("fast gate is red"), "{:?}", t.landed);
    assert_eq!(worker(&w.app).await["status"], "waiting", "a gate red after a merge red");
}

#[tokio::test]
async fn past_moved_reds_max_of_them_in_a_row_it_goes_to_the_watch() {
    let w = World::new();
    w.app.settings_set(Some("moved_reds_max"), Some(2)).await.unwrap();
    let wk = red_on_moved_proving(&w).await;
    let tree = worktree(&wk);
    for n in 2..=4 {
        merge_proving(&w, &tree);
        w.push_by_hand("web", "proving", "README", &format!("theirs {n}\n"));
        w.app.worker_done(id(&wk), None).await.unwrap();
        w.tick().await;
        let status = worker(&w.app).await["status"].clone();
        assert_eq!(status, if n < 4 { "working" } else { "waiting" }, "red {n}");
    }
}

#[tokio::test]
async fn blocked_issues_and_ones_waiting_for_a_human_wait_and_the_oldest_ready_one_starts_first() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "first", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "second", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "for a human", "", true, &[]).await.unwrap();
    w.app.issue_block("web", 1, 2, Some("needs its change"), false).await.unwrap();

    assert_eq!(w.tick().await.started.unwrap()["issue"], "web#2", "#1 waits for #2");
    assert!(w.tick().await.started.is_none(), "#1 waits, #3 waits for a human");
    w.app.issue_close("web", 2, "done").await.unwrap();
    assert_eq!(w.tick().await.started.unwrap()["issue"], "web#1");
}

#[tokio::test]
async fn an_issue_added_with_its_blockers_waits_for_them_from_the_start() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "blocker", "", true, &[]).await.unwrap();
    let added = w
        .app
        .issue_add("web", "waits", "", false, &[Wait::new(1, "needs it"), Wait::new(1, "needs it")])
        .await
        .unwrap();
    assert_eq!((added["status"].clone(), added["blocked_by"].clone()), (json!("ready"), json!([1])));
    assert!(w.tick().await.started.is_none(), "#2 waits for #1, which waits for a human");

    refused(
        w.app.issue_add("web", "orphan", "", false, &[Wait::new(1, "needs it"), Wait::new(9, "needs it")]).await,
        "no_issue",
    );
    refused(w.app.issue_show("web", 3).await, "no_issue");
    assert!(w.tick().await.started.is_none(), "the refused add left no issue to start");

    w.app.issue_close("web", 1, "done").await.unwrap();
    assert_eq!(w.tick().await.started.unwrap()["issue"], "web#2");
}

#[tokio::test]
async fn closing_an_issue_or_leaving_it_to_a_human_ends_its_worker() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let ws = w.app.workers(false).await.unwrap()["workers"].take();
    let (a, b) = (ws[0].clone(), ws[1].clone());
    let b_tree = worktree(&b);
    w.app.issue_close("web", a["number"].as_u64().unwrap(), "not needed").await.unwrap();
    w.app.issue_wait_for_human("web", b["number"].as_u64().unwrap(), "later").await.unwrap();
    assert_eq!(w.app.workers(false).await.unwrap()["workers"], json!([]));
    assert!(crate::fixture::closed(&w.app, a["terminal"].as_str().unwrap()).await);
    assert!(crate::fixture::closed(&w.app, b["terminal"].as_str().unwrap()).await);
    assert!(b_tree.exists(), "the worktree of an issue waiting for a human stays");
    w.app.issue_ready("web", b["number"].as_u64().unwrap()).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worker"], json!(factorium_domain::worker_id("web", b["number"].as_u64().unwrap(), 2)));
    assert_ne!(again["worktree"], b["worktree"], "a worktree that holds nothing of its own is not resumed");
}

/// The brief a worker was handed.
fn brief_of(w: &World, name: &str) -> String {
    std::fs::read_to_string(w.data().brief(name)).unwrap()
}

#[tokio::test]
async fn readying_an_issue_waiting_for_a_human_resumes_in_the_worktree_it_kept() {
    let w = World::new();
    let first = w.working(CLAUDE, "t").await;
    let tree = worktree(&first);
    w.commit(&tree, "kept.txt", "x");
    std::fs::write(tree.join("wip.txt"), "half done").unwrap();
    w.app.issue_wait_for_human("web", 1, "a mutants run holds the CPU").await.unwrap();
    // Proving moves on while the issue waits for a human.
    w.app.issue_add("web", "other", "", false, &[]).await.unwrap();
    w.tick().await;
    let other = worker(&w.app).await;
    w.done(&other, "other.txt", "y").await;
    assert_eq!(w.tick().await.landed[0]["landed"], "web#2");

    w.app.issue_ready("web", 1).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worker"], json!(factorium_domain::worker_id("web", 1, 2)));
    assert_eq!(again["worktree"], first["worktree"], "it resumes in the kept worktree");
    assert_eq!(worker(&w.app).await["branch"], first["branch"], "on its branch");
    assert!(!w.dir.join("worktrees").join("web-1-2").exists(), "no second worktree");
    assert_eq!(std::fs::read_to_string(tree.join("wip.txt")).unwrap(), "half done");
    let brief = brief_of(&w, "web-1-2");
    assert!(brief.contains("## You resume"), "{brief}");
    assert!(brief.contains("write kept.txt") && brief.contains("- `wip.txt`"), "{brief}");
    assert!(brief.contains("kept from an earlier attempt"), "{brief}");

    // It brings the branch up to date itself, and lands it.
    w.git(&tree, &["merge", "--quiet", "--no-edit", "origin/proving"]).unwrap();
    w.git(&tree, &["add", "-A"]).unwrap();
    w.git(&tree, &["commit", "--quiet", "-m", "finish"]).unwrap();
    w.app.worker_done(id(&worker(&w.app).await), None).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
    assert!(!tree.exists(), "landed, the worktree goes");
}

#[tokio::test]
async fn starting_an_issue_waiting_for_a_human_over_discards_its_kept_worktree_and_keeps_its_branch() {
    let w = World::new();
    let repo = w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let first = worker(&w.app).await;
    let tree = worktree(&first);
    w.commit(&tree, "kept.txt", "x");
    std::fs::write(tree.join("wip.txt"), "half done").unwrap();

    let live = w.app.issue_start_over("web", 1, "wrong approach").await.unwrap_err();
    assert_eq!(live.code, "live_worker");
    assert!(live.message.contains("wait-for-human"), "{}", live.message);
    assert!(tree.exists(), "refused, nothing changes");

    w.app.issue_wait_for_human("web", 1, "wrong approach").await.unwrap();
    let over = w.app.issue_start_over("web", 1, "wrong approach").await.unwrap();
    assert_eq!(over["status"], "waiting_for_human", "its state stays");
    assert_eq!(over["abandoned"], "factorium/web-1-1-abandoned");
    assert!(!tree.exists(), "the kept worktree is gone");
    let last = comments(&over).last().unwrap().clone();
    assert_eq!(last["by"], "factorium", "{last}");
    let comment = last["text"].as_str().unwrap().to_string();
    assert!(comment.contains("`factorium/web-1-1-abandoned`") && comment.contains("wrong approach"), "{comment}");
    // Nothing is lost: the commits and what was not committed are on it.
    let old = "factorium/web-1-1-abandoned";
    assert_eq!(w.git(&repo, &["show", &format!("{old}:kept.txt")]).unwrap(), "x");
    assert_eq!(w.git(&repo, &["show", &format!("{old}:wip.txt")]).unwrap(), "half done");

    w.app.issue_ready("web", 1).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worktree"], json!(w.dir.join("worktrees").join("web-1-2")), "a new worktree");
    let fresh = worker(&w.app).await;
    assert_eq!(fresh["branch"], "factorium/web-1-2");
    let new = worktree(&again);
    assert!(!new.join("kept.txt").exists() && !new.join("wip.txt").exists(), "begun from proving");
    let brief = brief_of(&w, "web-1-2");
    assert!(!brief.contains("You resume") && brief.contains(&comment), "{brief}");
}

#[tokio::test]
async fn starting_over_an_issue_with_nothing_kept_changes_nothing() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", true, &[]).await.unwrap();
    let over = w.app.issue_start_over("web", 1, "why not").await.unwrap();
    assert_eq!(over["discarded"], false);
    assert!(over["said"].as_str().unwrap().contains("nothing"));
    assert_eq!(over["status"], "waiting_for_human");
    assert_eq!(over["comments"], json!([]));

    // An attempt that ended holding nothing of its own kept nothing either.
    w.app.issue_ready("web", 1).await.unwrap();
    w.tick().await;
    w.app.issue_wait_for_human("web", 1, "later").await.unwrap();
    let over = w.app.issue_start_over("web", 1, "why not").await.unwrap();
    assert_eq!(over["discarded"], false);
    assert_eq!(over["comments"], json!([]));
}

#[tokio::test]
async fn a_start_after_a_kept_worktree_was_removed_starts_fresh() {
    let w = World::new();
    let repo = w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let first = worker(&w.app).await;
    let tree = worktree(&first);
    w.commit(&tree, "kept.txt", "x");
    w.app.issue_wait_for_human("web", 1, "later").await.unwrap();
    w.git(&repo, &["worktree", "remove", "--force", &tree.to_string_lossy()]).unwrap();

    w.app.issue_ready("web", 1).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worktree"], json!(w.dir.join("worktrees").join("web-1-2")), "a new worktree");
    assert_eq!(worker(&w.app).await["branch"], "factorium/web-1-2");
    let brief = brief_of(&w, "web-1-2");
    assert!(!brief.contains("You resume") && brief.contains("made from `web`'s proving branch"), "{brief}");
}

#[tokio::test]
async fn held_landings_wait_until_released() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.commit(&worktree(&wk), "f", "x");
    w.app.hold("web", Some("the proving branch is being repaired")).await.unwrap();
    w.app.worker_done(id(&wk), None).await.unwrap();
    assert!(w.tick().await.landed.is_empty());
    w.app.hold("web", None).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
}

#[tokio::test]
async fn an_escalation_waits_for_the_watch() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    refused(w.app.worker_escalate(id(&wk), " ").await, "no_reason");
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    let waiting = worker(&w.app).await;
    assert_eq!(
        (waiting["status"].clone(), waiting["why"].clone()),
        (json!("waiting"), json!("escalated: which API version?"))
    );
    refused(w.app.worker_done(id(&wk), None).await, "nothing");
}

#[tokio::test]
async fn a_worker_answers_when_it_started_while_since_follows_its_status() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let start = w.app_now();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!((wk["started_at"].clone(), wk["since"].clone()), (json!(start), json!(start)));

    w.advance(5);
    w.done(&wk, "f", "x").await;
    let landing = worker(&w.app).await;
    assert_eq!(landing["status"], "landing");
    assert_eq!(landing["since"], json!(start + 5 * 60_000));
    assert_eq!(landing["started_at"], json!(start), "the start stays put");
}

#[tokio::test]
async fn the_tracker_rows_follow_each_event_and_doctor_names_a_waiting_worker_by_id() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let start = w.app_now();
    w.tick().await;
    let wk = worker(&w.app).await;
    let row = |v: &serde_json::Value| (v["status"].clone(), v["why"].clone(), v["since"].clone());
    assert_eq!(row(&wk), (json!("working"), json!(null), json!(start)));
    let issue = w.app.issue_list(None, false).await.unwrap()[0].clone();
    assert_eq!((issue["status"].clone(), issue["worker"].clone()), (json!("working"), wk["worker"].clone()));

    w.advance(2);
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    let waiting = worker(&w.app).await;
    let at = start + 2 * 60_000;
    assert_eq!(row(&waiting), (json!("waiting"), json!("escalated: which API version?"), json!(at)));
    let doctor = w.app.doctor().await.unwrap();
    let found = findings(&doctor).iter().find(|f| f["key"] == format!("worker:{}", id(&wk)));
    let found = found.expect("the waiting worker, by its id");
    assert!(found["what"].as_str().unwrap().contains("on web#1 waits: escalated"), "{found}");

    w.advance(1);
    w.app.issue_close("web", 1, "not needed").await.unwrap();
    let issue = w.app.issue_list(None, true).await.unwrap()[0].clone();
    assert_eq!(
        (issue["status"].clone(), issue["reason"].clone(), issue["worker"].clone()),
        (json!("closed"), json!("not needed"), json!(null))
    );
    assert_eq!(w.app.workers(false).await.unwrap()["workers"], json!([]));
    let ended = w.app.workers(true).await.unwrap()["workers"][0].clone();
    assert_eq!(row(&ended), (json!("ended"), json!("its issue closed: not needed"), json!(at + 60_000)));
    // A closed issue takes no more commands.
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["checkout"], "web");
    refused(w.app.issue_comment("web", 1, "one more thing").await, "ended");
}

#[tokio::test]
async fn an_issue_of_a_removed_checkout_starts_no_worker() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.app.checkout_remove("web").await.unwrap();
    let t = w.tick().await;
    assert_eq!((t.started, t.held), (None, None));
}

#[tokio::test]
async fn a_brief_carries_the_issue_s_comments_and_only_then_their_heading() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.app.issue_comment("web", 2, "use the v2 API").await.unwrap();
    w.tick().await;
    w.tick().await;
    let a = std::fs::read_to_string(w.data().brief("web-1-1")).unwrap();
    assert!(!a.contains("## Comments"), "{a}");
    let b = std::fs::read_to_string(w.data().brief("web-2-1")).unwrap();
    assert!(b.contains("## Comments\n\n- owner: use the v2 API\n"), "{b}");
}

#[tokio::test]
async fn stop_with_no_live_daemon_says_so_and_forgets_its_pid_file() {
    let w = World::new();
    assert_eq!(w.app.stop().await.unwrap()["why"], "no daemon runs");
    let pid = w.data().pid();
    // A pid no process has (macOS gives at most 99998).
    std::fs::write(&pid, "999999").unwrap();
    assert_eq!(w.app.stop().await.unwrap(), json!({ "stopped": false, "why": "no daemon runs", "watch_closed": null }));
    assert!(!pid.exists());
}

#[tokio::test]
async fn a_closed_issue_s_worktree_is_removed_through_orca_even_once_gone_from_disk() {
    let w = World::new();
    let tree = worktree(&w.working(CLAUDE, "a").await);
    std::fs::remove_dir_all(&tree).unwrap();
    w.app.issue_close("web", 1, "not needed").await.unwrap();
    assert!(w.host.orca.removed.lock().unwrap().contains(&tree), "Orca forgets it");
}

#[tokio::test]
async fn orca_refusing_to_remove_a_worktree_is_an_error_and_the_worktree_stays() {
    let w = World::new();
    let tree = worktree(&w.working(CLAUDE, "a").await);
    w.host.orca.fault("remove-refused", true);
    let refused = w.app.issue_close("web", 1, "not needed").await.unwrap_err();
    assert!(refused.message.contains("orca worktree rm: selector_not_found"), "{refused:?}");
    assert!(tree.exists());
}

/// A worker on `web` whose proving declares `rec/*.json` regenerated by its
/// fast gate, which writes the merge's parents to `rec/gen.json`; the branch
/// and proving both changed `rec/gen.json`, and proving also `also` to it.
async fn conflicting_on_records(w: &World, also: &str) -> serde_json::Value {
    w.checkout("web").await;
    w.kit().await;
    let toml =
        "[gate]\nfast = \"test ! -e RED && git log -1 --format=%P > rec/gen.json\"\nregenerated = [\"rec/*.json\"]\n";
    w.push_by_hand("web", "proving", "factorium.toml", toml);
    std::fs::create_dir_all(w.dir.join("by-hand/rec")).unwrap();
    w.push_by_hand("web", "proving", "rec/gen.json", "start\n");
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let tree = worktree(&wk);
    w.commit(&tree, "hello.txt", "hello\n");
    w.commit(&tree, "rec/gen.json", "mine\n");
    w.push_by_hand("web", "proving", "rec/gen.json", "theirs\n");
    if !also.is_empty() {
        w.commit(&tree, also, "mine\n");
        w.push_by_hand("web", "proving", also, "theirs\n");
    }
    w.app.worker_done(id(&wk), None).await.unwrap();
    wk
}

#[tokio::test]
async fn a_landing_whose_only_conflicts_are_regenerated_records_takes_provings_and_lands_them_as_the_gate_wrote_them() {
    let w = World::new();
    conflicting_on_records(&w, "").await;
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1", "{:?}", t.landed);
    let landed = t.landed[0]["commit"].as_str().unwrap().to_string();
    let remote = w.dir.join("remotes/web.git");
    let parents = w.git(&remote, &["log", "-1", "--format=%P", &landed]).unwrap();
    let record = w.git(&remote, &["show", &format!("{landed}:rec/gen.json")]).unwrap();
    assert_eq!(record.trim(), parents.trim(), "the record as the gate wrote it on the merge");
    assert_eq!(w.git(&remote, &["show", &format!("{landed}:hello.txt")]).unwrap().trim(), "hello");
    let said = w.git(&remote, &["log", "-1", "--format=%B", &landed]).unwrap();
    assert!(
        said.starts_with("Land web#1: t") && said.contains("Regenerated") && said.contains("rec/gen.json"),
        "{said}"
    );
}

#[tokio::test]
async fn a_landing_that_takes_provings_copy_of_a_regenerated_record_keeps_both_sides_kept_entries() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.push_by_hand("web", "proving", "factorium.toml", "[gate]\nregenerated = [\"rec/*.json\"]\n");
    std::fs::create_dir_all(w.dir.join("by-hand/rec")).unwrap();
    w.push_by_hand("web", "proving", "rec/gen.json", r#"{"kept": [], "measured": 0}"#);
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.commit(&worktree(&wk), "rec/gen.json", r#"{"kept": [{"test": "both"}, {"test": "mine"}], "measured": 1}"#);
    w.push_by_hand(
        "web",
        "proving",
        "rec/gen.json",
        r#"{"kept": [{"test": "theirs"}, {"test": "both"}], "measured": 2}"#,
    );
    w.app.worker_done(id(&wk), None).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1", "{:?}", t.landed);
    let landed = t.landed[0]["commit"].as_str().unwrap();
    let remote = w.dir.join("remotes/web.git");
    let record = w.git(&remote, &["show", &format!("{landed}:rec/gen.json")]).unwrap();
    let record: serde_json::Value = serde_json::from_str(&record).unwrap();
    let want = serde_json::json!({"kept": [{"test": "theirs"}, {"test": "both"}, {"test": "mine"}], "measured": 2});
    assert_eq!(record, want, "proving's copy, with the branch's kept verdicts");
}

#[tokio::test]
async fn a_landing_with_a_conflict_outside_the_regenerated_records_is_red_naming_it() {
    let w = World::new();
    conflicting_on_records(&w, "README").await;
    let t = w.tick().await;
    let red = t.landed[0]["red"].as_str().unwrap_or_else(|| panic!("red: {:?}", t.landed));
    assert!(red.contains("does not merge") && red.contains("README") && red.contains("rec/gen.json"), "{red}");
    assert_eq!(worker(&w.app).await["status"], "working");
}
