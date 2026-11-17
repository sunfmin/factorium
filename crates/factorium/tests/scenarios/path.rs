//! The issue's path: a ready issue gets a worker in its own worktree; its
//! `done` lands it on the proving branch through the fast gate; the issue
//! closes and the worker ends; the base is promoted and a version cut.

use std::path::PathBuf;

use serde_json::json;

use crate::{World, id, refused, worker};

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
    let tree = PathBuf::from(wk["worktree"].as_str().unwrap());
    assert_eq!(w.git(&tree, &["rev-parse", "HEAD"]).unwrap(), base, "branched from the proving tip");
    let branch = w.git(&tree, &["branch", "--show-current"]).unwrap();
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["worker"], wk["worker"]);
    let term = w.terminal(wk["terminal"].as_str().unwrap());
    assert_eq!(term.worktree, tree);
    assert!(term.line.contains("claude --dangerously-skip-permissions"), "{}", term.line);
    let brief = std::fs::read_to_string(w.dir.join("data/briefs/web-1-1.md")).unwrap();
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
    w.app.worker_done(id(&wk)).await.unwrap();
    assert_eq!(worker(&w.app).await["status"], "landing");

    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1");
    let landed = t.landed[0]["commit"].as_str().unwrap().to_string();
    assert_eq!(w.remote_tip("web", "proving"), landed);
    let issue = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!((issue["status"].clone(), issue["worker"].clone()), (json!("closed"), json!(null)));
    assert!(issue["reason"].as_str().unwrap().contains(&landed));
    assert_eq!(w.app.workers(false).await.unwrap(), json!([]));
    assert!(crate::closed(&w.app, "term-1").await, "its session ends");
    assert!(!tree.exists(), "its worktree goes");
    let left = w.git(&web, &["branch", "--list", &branch]).unwrap();
    assert!(left.trim().is_empty(), "its branch goes: {branch}");

    // Idle, the same tick promoted and cut.
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"));
    let cut = t.lines.iter().find(|l| l["cut"] == "web").expect("a cut");
    assert_eq!(cut["version"], "v0.1.0");
    assert!(w.git(&w.dir.join("remotes/web.git"), &["rev-parse", "v0.1.0"]).is_ok());
    // The first cut installs it, and Homebrew is asked what it has once done.
    let installs = ["update --quiet", "list --formula web", "install web", "list --versions web"];
    assert_eq!(w.host.brewed(), installs);
}

#[tokio::test]
async fn done_takes_only_committed_work_of_its_own() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let tree = PathBuf::from(wk["worktree"].as_str().unwrap());
    refused(w.app.worker_done(id(&wk)).await, "nothing");
    std::fs::write(tree.join("loose"), "x").unwrap();
    refused(w.app.worker_done(id(&wk)).await, "dirty");
    w.git(&tree, &["add", "-A"]).unwrap();
    w.git(&tree, &["commit", "-qm", "x"]).unwrap();
    w.app.worker_done(id(&wk)).await.unwrap();
}

#[tokio::test]
async fn a_red_landing_tells_the_worker_why_and_the_second_red_goes_to_the_watch() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let tree = PathBuf::from(wk["worktree"].as_str().unwrap());
    let proving = w.remote_tip("web", "proving");

    w.commit(&tree, "RED", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert!(t.landed[0]["red"].as_str().unwrap().contains("fast gate is red"));
    assert_eq!(t.landed[0]["status"], "working", "back to work");
    assert_eq!(w.remote_tip("web", "proving"), proving, "nothing red lands");
    let again = worker(&w.app).await;
    assert_eq!(again["status"], "working");
    let told = w.terminal(wk["terminal"].as_str().unwrap()).sent;
    assert!(told.last().unwrap().contains("landing was red"), "{told:?}");
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], "ready");

    w.app.worker_done(id(&wk)).await.unwrap();
    w.tick().await;
    let waiting = worker(&w.app).await;
    assert_eq!(waiting["status"], "waiting");
    assert!(waiting["why"].as_str().unwrap().starts_with("red twice"));
    let doctor = w.app.doctor().await.unwrap();
    assert!(doctor["findings"][0]["what"].as_str().unwrap().contains("red twice"));
}

#[tokio::test]
async fn a_red_landing_then_a_fix_lands() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let tree = PathBuf::from(wk["worktree"].as_str().unwrap());
    w.commit(&tree, "RED", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
    w.tick().await;
    w.git(&tree, &["rm", "-q", "RED"]).unwrap();
    w.git(&tree, &["commit", "-qm", "fix"]).unwrap();
    w.app.worker_done(id(&wk)).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
}

#[tokio::test]
async fn a_branch_that_does_not_merge_is_red() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let tree = PathBuf::from(wk["worktree"].as_str().unwrap());
    w.commit(&tree, "README", "mine\n");
    // The proving branch moves under it, the same file changed.
    let other = w.dir.join("other");
    let remote = w.dir.join("remotes/web.git");
    w.git(&w.dir, &["clone", "-q", "-b", "proving", &remote.to_string_lossy(), &other.to_string_lossy()]).unwrap();
    w.commit(&other, "README", "theirs\n");
    w.git(&other, &["push", "-q", "origin", "HEAD:proving"]).unwrap();

    w.app.worker_done(id(&wk)).await.unwrap();
    let t = w.tick().await;
    assert!(t.landed[0]["red"].as_str().unwrap().contains("does not merge"), "{:?}", t.landed);
}

#[tokio::test]
async fn blocked_and_parked_issues_wait_and_the_oldest_ready_one_starts_first() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "first", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "second", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "parked", "", true, &[]).await.unwrap();
    w.app.issue_block("web", 1, 2, false).await.unwrap();

    assert_eq!(w.tick().await.started.unwrap()["issue"], "web#2", "#1 waits for #2");
    assert!(w.tick().await.started.is_none(), "#1 waits, #3 is parked");
    w.app.issue_close("web", 2, "done").await.unwrap();
    assert_eq!(w.tick().await.started.unwrap()["issue"], "web#1");
}

#[tokio::test]
async fn an_issue_added_with_its_blockers_waits_for_them_from_the_start() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "blocker", "", true, &[]).await.unwrap();
    let added = w.app.issue_add("web", "waits", "", false, &[1, 1]).await.unwrap();
    assert_eq!((added["status"].clone(), added["blocked_by"].clone()), (json!("ready"), json!([1])));
    assert!(w.tick().await.started.is_none(), "#2 waits for #1, which is parked");

    refused(w.app.issue_add("web", "orphan", "", false, &[1, 9]).await, "no_issue");
    refused(w.app.issue_show("web", 3).await, "no_issue");
    assert!(w.tick().await.started.is_none(), "the refused add left no issue to start");

    w.app.issue_close("web", 1, "done").await.unwrap();
    assert_eq!(w.tick().await.started.unwrap()["issue"], "web#2");
}

#[tokio::test]
async fn closing_or_parking_an_issue_ends_its_worker() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let ws = w.app.workers(false).await.unwrap();
    let (a, b) = (ws[0].clone(), ws[1].clone());
    let b_tree = PathBuf::from(b["worktree"].as_str().unwrap());
    w.app.issue_close("web", a["number"].as_u64().unwrap(), "not needed").await.unwrap();
    w.app.issue_park("web", b["number"].as_u64().unwrap(), "later").await.unwrap();
    assert_eq!(w.app.workers(false).await.unwrap(), json!([]));
    assert!(crate::closed(&w.app, a["terminal"].as_str().unwrap()).await);
    assert!(crate::closed(&w.app, b["terminal"].as_str().unwrap()).await);
    assert!(b_tree.exists(), "a parked issue's worktree stays");
    w.app.issue_ready("web", b["number"].as_u64().unwrap()).await.unwrap();
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["worker"], json!(factorium_domain::worker_id("web", b["number"].as_u64().unwrap(), 2)));
    assert_ne!(again["worktree"], b["worktree"], "a worktree that holds nothing of its own is not resumed");
}

/// The brief a worker was handed.
fn brief_of(w: &World, name: &str) -> String {
    std::fs::read_to_string(w.dir.join("data").join("briefs").join(format!("{name}.md"))).unwrap()
}

#[tokio::test]
async fn readying_a_parked_issue_resumes_in_the_worktree_it_kept() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let first = worker(&w.app).await;
    let tree = PathBuf::from(first["worktree"].as_str().unwrap());
    w.commit(&tree, "kept.txt", "x");
    std::fs::write(tree.join("wip.txt"), "half done").unwrap();
    w.app.issue_park("web", 1, "a mutants run holds the CPU").await.unwrap();
    // Proving moves on while the issue is parked.
    w.app.issue_add("web", "other", "", false, &[]).await.unwrap();
    w.tick().await;
    let other = worker(&w.app).await;
    w.commit(&PathBuf::from(other["worktree"].as_str().unwrap()), "other.txt", "y");
    w.app.worker_done(id(&other)).await.unwrap();
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
    w.app.worker_done(id(&worker(&w.app).await)).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
    assert!(!tree.exists(), "landed, the worktree goes");
}

#[tokio::test]
async fn starting_a_parked_issue_over_discards_its_kept_worktree_and_keeps_its_branch() {
    let w = World::new();
    let repo = w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let first = worker(&w.app).await;
    let tree = PathBuf::from(first["worktree"].as_str().unwrap());
    w.commit(&tree, "kept.txt", "x");
    std::fs::write(tree.join("wip.txt"), "half done").unwrap();

    let live = w.app.issue_start_over("web", 1, "wrong approach").await.unwrap_err();
    assert_eq!(live.code, "live_worker");
    assert!(live.message.contains("park"), "{}", live.message);
    assert!(tree.exists(), "refused, nothing changes");

    w.app.issue_park("web", 1, "wrong approach").await.unwrap();
    let over = w.app.issue_start_over("web", 1, "wrong approach").await.unwrap();
    assert_eq!(over["status"], "parked", "its state stays");
    assert_eq!(over["abandoned"], "factorium/web-1-1-abandoned");
    assert!(!tree.exists(), "the kept worktree is gone");
    let last = over["comments"].as_array().unwrap().last().unwrap().clone();
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
    let new = PathBuf::from(again["worktree"].as_str().unwrap());
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
    assert_eq!(over["status"], "parked");
    assert_eq!(over["comments"], json!([]));

    // An attempt that ended holding nothing of its own kept nothing either.
    w.app.issue_ready("web", 1).await.unwrap();
    w.tick().await;
    w.app.issue_park("web", 1, "later").await.unwrap();
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
    let tree = PathBuf::from(first["worktree"].as_str().unwrap());
    w.commit(&tree, "kept.txt", "x");
    w.app.issue_park("web", 1, "later").await.unwrap();
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
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "f", "x");
    w.app.hold("web", Some("the proving branch is being repaired")).await.unwrap();
    w.app.worker_done(id(&wk)).await.unwrap();
    assert!(w.tick().await.landed.is_empty());
    w.app.hold("web", None).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
}

#[tokio::test]
async fn an_escalation_waits_for_the_watch() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    refused(w.app.worker_escalate(id(&wk), " ").await, "no_reason");
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    let waiting = worker(&w.app).await;
    assert_eq!(
        (waiting["status"].clone(), waiting["why"].clone()),
        (json!("waiting"), json!("escalated: which API version?"))
    );
    refused(w.app.worker_done(id(&wk)).await, "nothing");
}

#[tokio::test]
async fn a_worker_answers_when_it_started_while_since_follows_its_status() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let start = factum::Clock::now(&*w.clock).0;
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!((wk["started_at"].clone(), wk["since"].clone()), (json!(start), json!(start)));

    w.advance(5);
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "f", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
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
    let start = factum::Clock::now(&*w.clock).0;
    w.tick().await;
    let wk = worker(&w.app).await;
    let row = |v: &serde_json::Value| (v["status"].clone(), v["why"].clone(), v["since"].clone());
    assert_eq!(row(&wk), (json!("working"), json!(null), json!(start)));
    let issue = w.app.issue_list(None, false).await.unwrap()[0].clone();
    assert_eq!((issue["status"].clone(), issue["worker"].clone()), (json!("ready"), wk["worker"].clone()));

    w.advance(2);
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    let waiting = worker(&w.app).await;
    let at = start + 2 * 60_000;
    assert_eq!(row(&waiting), (json!("waiting"), json!("escalated: which API version?"), json!(at)));
    let doctor = w.app.doctor().await.unwrap();
    let found = doctor["findings"].as_array().unwrap().iter().find(|f| f["key"] == format!("worker:{}", id(&wk)));
    let found = found.expect("the waiting worker, by its id");
    assert!(found["what"].as_str().unwrap().contains("on web#1 waits: escalated"), "{found}");

    w.advance(1);
    w.app.issue_close("web", 1, "not needed").await.unwrap();
    let issue = w.app.issue_list(None, true).await.unwrap()[0].clone();
    assert_eq!(
        (issue["status"].clone(), issue["reason"].clone(), issue["worker"].clone()),
        (json!("closed"), json!("not needed"), json!(null))
    );
    assert_eq!(w.app.workers(false).await.unwrap(), json!([]));
    let ended = w.app.workers(true).await.unwrap()[0].clone();
    assert_eq!(row(&ended), (json!("ended"), json!("its issue closed: not needed"), json!(at + 60_000)));
}

#[tokio::test]
async fn a_closed_issue_takes_no_more_commands() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["checkout"], "web");
    w.app.issue_close("web", 1, "done elsewhere").await.unwrap();
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
    let a = std::fs::read_to_string(w.dir.join("data/briefs/web-1-1.md")).unwrap();
    assert!(!a.contains("## Comments"), "{a}");
    let b = std::fs::read_to_string(w.dir.join("data/briefs/web-2-1.md")).unwrap();
    assert!(b.contains("## Comments\n\n- owner: use the v2 API\n"), "{b}");
}

#[tokio::test]
async fn stop_with_no_live_daemon_says_so_and_forgets_its_pid_file() {
    let w = World::new();
    assert_eq!(w.app.stop().await.unwrap()["why"], "no daemon runs");
    let pid = w.dir.join("data/daemon.pid");
    // A pid no process has (macOS gives at most 99998).
    std::fs::write(&pid, "999999").unwrap();
    assert_eq!(w.app.stop().await.unwrap(), json!({ "stopped": false, "why": "no daemon runs", "watch_closed": null }));
    assert!(!pid.exists());
}

#[tokio::test]
async fn a_repo_that_declares_no_fast_gate_lands_on_none() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.commit(&PathBuf::from(wk["worktree"].as_str().unwrap()), "factorium.toml", "[gate]\nslow = \"true\"\n");
    w.app.worker_done(id(&wk)).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
}

/// `web` with one worker at work, no commit of its own; answers its worktree.
async fn one_worker(w: &World) -> PathBuf {
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    PathBuf::from(worker(&w.app).await["worktree"].as_str().unwrap())
}

#[tokio::test]
async fn a_closed_issue_s_worktree_is_removed_through_orca_even_once_gone_from_disk() {
    let w = World::new();
    let tree = one_worker(&w).await;
    std::fs::remove_dir_all(&tree).unwrap();
    w.app.issue_close("web", 1, "not needed").await.unwrap();
    assert!(w.host.orca.removed.lock().unwrap().contains(&tree), "Orca forgets it");
}

#[tokio::test]
async fn orca_refusing_to_remove_a_worktree_is_an_error_and_the_worktree_stays() {
    let w = World::new();
    let tree = one_worker(&w).await;
    *w.host.orca.refuse_rm.lock().unwrap() = true;
    let refused = w.app.issue_close("web", 1, "not needed").await.unwrap_err();
    assert!(refused.message.contains("orca worktree rm: worktree_remove_failed"), "{refused:?}");
    assert!(tree.exists());
}
