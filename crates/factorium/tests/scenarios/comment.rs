//! A comment on an issue reaches the worker on it exactly once: through its
//! session's outbox (typed, or in its brief: `outbox.rs`), or through the
//! watch; and is never lost.

use serde_json::json;

use crate::fixture::{CLAUDE, World, id, worktree};

#[tokio::test]
async fn a_comment_with_no_worker_on_the_issue_is_only_recorded() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let shown = w.app.issue_comment("web", 1, "use the v2 API").await.unwrap();
    assert_eq!(shown["delivery"], "none");
    assert_eq!(shown["undelivered"], json!(null), "nothing failed: no one to tell");
    assert_eq!(shown["comments"], json!([{ "text": "use the v2 API", "by": "owner", "at": 1_800_000_000_000_i64 }]));
}

#[tokio::test]
async fn a_comment_for_a_waiting_worker_goes_to_its_watch_episode_not_its_session() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    w.advance(1);
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    w.tick().await;
    let next = w.app.watch_next().await.unwrap();
    let key = format!("worker:{}", id(&wk));
    assert_eq!(next["episodes"][&key]["heard"], json!([]), "{next}");
    let before = w.terminal(handle).sent.len();

    w.advance(1);
    let shown = w.app.issue_comment("web", 1, "use the v2 API").await.unwrap();
    assert_eq!(shown["delivery"], "watch");
    assert_eq!(w.terminal(handle).sent.len(), before, "a waiting worker is not typed at");
    let looked = w.tick().await.watch;
    assert!(looked.is_some(), "the comment makes the look due");
    let next = w.app.watch_next().await.unwrap();
    assert_eq!(next["episodes"][&key]["heard"], json!(["use the v2 API"]), "{next}");

    // Sent back to work, the worker starts afresh: a later wait hears nothing old.
    w.app.watch_restart(id(&wk), "v2", false).await.unwrap();
    w.advance(1);
    w.app.worker_escalate(id(&wk), "again").await.unwrap();
    let state = w.app.watch_next().await.unwrap();
    assert_eq!(state["episodes"][&key]["heard"], json!(["use the v2 API"]), "until the next look");
    w.advance(1);
    w.tick().await;
    let next = w.app.watch_next().await.unwrap();
    assert_eq!(next["episodes"][&key]["heard"], json!([]), "{next}");
}

/// A waiting worker's episode carries what the owner said since it started
/// waiting, and nothing said before.
#[tokio::test]
async fn a_waiting_worker_hears_only_what_was_said_since_it_started_waiting() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.advance(1);
    w.app.issue_comment("web", 1, "said while working").await.unwrap();
    w.advance(1);
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    w.tick().await;
    let key = format!("worker:{}", id(&wk));
    let next = w.app.watch_next().await.unwrap();
    assert_eq!(next["episodes"][&key]["heard"], json!([]), "{next}");

    w.advance(1);
    assert_eq!(w.app.issue_comment("web", 1, "said while waiting").await.unwrap()["delivery"], "watch");
    assert!(w.tick().await.watch.is_some(), "the comment makes the look due");
    let next = w.app.watch_next().await.unwrap();
    assert_eq!(next["episodes"][&key]["heard"], json!(["said while waiting"]), "{next}");
}

/// How a comment is typed, or why not, is the outbox's (`outbox.rs`); the
/// issue keeps it either way.
#[tokio::test]
async fn a_comment_that_cannot_be_typed_is_kept_for_the_next_brief() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.host.orca.fault("send-refused", true);
    assert_eq!(w.app.issue_comment("web", 1, "use the v2 API").await.unwrap()["delivery"], "none");
    w.host.orca.fault("send-refused", false);
    w.kill_terminal(wk["terminal"].as_str().unwrap());
    assert_eq!(w.app.issue_comment("web", 1, "and log it").await.unwrap()["delivery"], "none");

    // The next worker's brief carries both.
    w.app.issue_wait_for_human("web", 1, "its session died").await.unwrap();
    w.app.issue_ready("web", 1).await.unwrap();
    w.tick().await;
    let brief = std::fs::read_to_string(w.data().brief("web-1-2")).unwrap();
    assert!(brief.contains("## Comments\n\n- owner: use the v2 API\n- owner: and log it\n"), "{brief}");
}

#[tokio::test]
async fn a_comment_factorium_writes_itself_is_typed_at_no_one() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let tree = worktree(&wk);
    w.commit(&tree, "kept.txt", "x");
    w.app.issue_wait_for_human("web", 1, "wrong approach").await.unwrap();
    w.app.issue_start_over("web", 1, "wrong approach").await.unwrap();
    let terminals = w.host.orca.terminals.lock().unwrap().clone();
    for t in terminals.values() {
        assert!(!t.sent.iter().any(|s| s.contains("a comment on your issue")), "{:?}", t.sent);
    }
}

#[tokio::test]
async fn an_escalation_is_kept_on_the_issue_and_not_typed_back_at_its_worker() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    let before = w.terminal(handle).sent.len();
    w.advance(1);
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    let by = format!("worker {}", id(&wk));
    let at = 1_800_000_000_000_i64 + 60_000;
    assert_eq!(w.comments("web", 1).await, [json!({ "text": "which API version?", "by": by, "at": at })]);
    assert_eq!(w.terminal(handle).sent.len(), before, "not typed back at it");
    w.tick().await;
    let next = w.app.watch_next().await.unwrap();
    assert_eq!(next["episodes"][format!("worker:{}", id(&wk))]["heard"], json!([]), "not heard twice: {next}");
}

/// A worker's record (a measurement) is its comment, and only that: the
/// worker goes on working, no one is typed at, and the watch has nothing.
#[tokio::test]
async fn a_worker_s_own_comment_is_kept_and_changes_nothing_else() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    let before = w.terminal(handle).sent.len();
    w.advance(1);
    let said_back = w.app.worker_comment(id(&wk), "before 12.3s, after 4.1s").await.unwrap();
    assert_eq!(said_back["commented"], true);
    let by = format!("worker {}", id(&wk));
    let at = 1_800_000_000_000_i64 + 60_000;
    assert_eq!(w.comments("web", 1).await, [json!({ "text": "before 12.3s, after 4.1s", "by": by, "at": at })]);
    assert_eq!(crate::fixture::worker(&w.app).await["status"], "working");
    assert!(w.app.worker_comment(id(&wk), "  ").await.is_err(), "an empty record is refused");
    w.advance(5);
    let t = w.tick().await;
    assert_eq!(w.terminal(handle).sent.len(), before, "not typed back at it");
    assert_eq!(t.watch.and_then(|l| l.get("round").cloned()).unwrap_or(json!(null)), json!(null), "no round");
    assert_eq!(w.app.watch_next().await.unwrap()["episodes"], json!({}), "no episode");
    assert_eq!(crate::fixture::worker(&w.app).await["status"], "working");
}

#[tokio::test]
async fn what_the_watch_tells_a_worker_is_kept_on_its_issue_and_typed_once() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    let before = w.terminal(handle).sent.len();
    w.app.watch_tell(id(&wk), "use the v2 API", false).await.unwrap();
    assert_eq!(w.comments("web", 1).await[0]["by"], "watch");
    assert_eq!(w.comments("web", 1).await[0]["text"], "use the v2 API");
    w.tick().await;
    assert_eq!(w.terminal(handle).sent[before..], ["factorium (the watch): use the v2 API".to_string()]);

    // Not delivered, it is kept all the same.
    w.host.orca.fault("send-refused", true);
    assert!(w.app.watch_tell(id(&wk), "and log it", false).await.is_err());
    assert_eq!(w.comments("web", 1).await[1]["text"], "and log it");
}

#[tokio::test]
async fn what_the_watch_restarts_a_worker_with_is_kept_on_its_issue_and_typed_once() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    let before = w.terminal(handle).sent.len();
    w.app.watch_restart(id(&wk), "use the v2 API.", false).await.unwrap();
    let said = w.comments("web", 1).await;
    assert_eq!((said[1]["by"].as_str(), said[1]["text"].as_str()), (Some("watch"), Some("use the v2 API.")));
    w.tick().await;
    let sent = &w.terminal(handle).sent[before..];
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].starts_with("factorium (the watch): use the v2 API. Then go on"), "{sent:?}");

    // The next worker's brief carries all of it, each with who said it.
    w.app.issue_wait_for_human("web", 1, "again").await.unwrap();
    w.app.issue_ready("web", 1).await.unwrap();
    w.tick().await;
    let brief = std::fs::read_to_string(w.data().brief("web-1-2")).unwrap();
    let want = format!("## Comments\n\n- worker {}: which API version?\n- watch: use the v2 API.\n", id(&wk));
    assert!(brief.contains(&want), "{brief}");
}

/// A comment kept before comments kept their author and time still reads:
/// the owner's, unless its words are factorium's start-over, and with no time.
#[tokio::test]
async fn a_comment_kept_before_authors_were_kept_still_reads() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.app.issue_comment("web", 1, "use the v2 API").await.unwrap();
    w.app.issue_comment("web", 1, "Started over: attempt 1's worktree was discarded").await.unwrap();
    w.comments_as_their_first_version_wrote_them();
    let shown = w.run().issue_show("web", 1).await.unwrap();
    assert_eq!(
        shown["comments"],
        json!([
            { "text": "use the v2 API", "by": "owner", "at": null },
            { "text": "Started over: attempt 1's worktree was discarded", "by": "factorium", "at": null },
        ])
    );
}

/// A log the code before factorium#161 wrote holds a copy of the comment on
/// the worker: it still reads, the copy is ignored, and the worker goes on.
#[tokio::test]
async fn a_copy_of_a_comment_older_code_kept_on_its_worker_is_ignored() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.advance(1);
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    w.advance(1);
    w.app.issue_comment("web", 1, "use the v2 API").await.unwrap();
    w.heard_as_code_before_161_kept_it("use the v2 API");
    w.tick().await;
    let next = w.app.watch_next().await.unwrap();
    let key = format!("worker:{}", id(&wk));
    assert_eq!(next["episodes"][&key]["heard"], json!(["use the v2 API"]), "heard once: {next}");
    w.app.watch_restart(id(&wk), "v2", false).await.unwrap();
    w.app.worker_escalate(id(&wk), "again").await.expect("its stream still folds and takes commands");
}

/// A worker that decides what its issue did not settle comments a Decided
/// record, and doctor's `decided` lists it.
#[tokio::test]
async fn a_worker_s_decided_record_shows_up_in_doctor_s_decided() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let record = "Decided (worker, for Felix): KEEP applies only to sampled survivors\n\
        Options: A) every survivor  B) sampled survivors only\n\
        Chose: B, because a survivor not sampled was never judged\n\
        Undo: revert the commit that reads KEEP";
    w.app.worker_comment(id(&wk), record).await.unwrap();
    let decided = w.doctor().await["decided"].clone();
    assert_eq!(decided.as_array().map(Vec::len), Some(1), "{decided}");
    assert_eq!(decided[0]["issue"], "web#1");
    assert_eq!(decided[0]["who"], "worker");
    assert_eq!(decided[0]["chose"], "B, because a survivor not sampled was never judged");
}
