//! An engine at its limit (factorium#223): agy's quota screen parks its
//! worker until the reset its screen gives, what is for it held, with no
//! finding and no round meanwhile; at the reset the first tick sends it back
//! with what was held. No worker starts on the spent harness before then.

use serde_json::{Value, json};

use crate::fixture::{CLAUDE, World, finding_keys, id, worktree};

/// agy's quota, as it shows it, with its error's id `n`.
fn quota(n: u32) -> [String; 3] {
    [
        "⚠ Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 2h50m4s."
            .into(),
        format!("Error ID: baec577a-{n}"),
        String::new(),
    ]
}

/// How long agy's quota above says it has to its reset, ms.
const RESETS_IN: i64 = (2 * 3600 + 50 * 60 + 4) * 1000;

/// Shows agy's quota on the worker's screen.
fn spent(w: &World, wk: &Value, n: u32) {
    let lines = quota(n);
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    w.set_screen(wk["terminal"].as_str().unwrap(), &lines);
}

/// The messages typed at the worker's session.
fn sent(w: &World, wk: &Value) -> Vec<String> {
    w.terminal(wk["terminal"].as_str().unwrap()).sent
}

#[tokio::test]
async fn an_agy_worker_at_its_quota_is_parked_until_the_reset_and_then_sent_back_with_what_was_held() {
    let w = World::new();
    w.checkout("web").await;
    w.kit_of(&["agy=agy"]).await;
    w.app.kit_pin("web", Some("agy")).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = crate::fixture::worker(&w.app).await;
    w.watch_session(false);
    let tree = worktree(&wk);
    w.commit(&tree, "RED", "x");
    w.app.worker_done(id(&wk), None).await.unwrap();
    let typed = sent(&w, &wk).len();
    spent(&w, &wk, 1);

    let until = w.app_now() + RESETS_IN;
    let t = w.tick().await;
    assert!(t.landed[0]["red"].as_str().unwrap().contains("fast gate is red"), "{t:?}");
    assert_eq!(t.seen, [json!({ "worker": wk["worker"], "limited": until, "said": quota(1)[..2].join(" ") })]);
    assert_eq!(sent(&w, &wk).len(), typed, "nothing is typed at a spent engine");
    let outbox = w.app.outbox_of(Some(id(&wk))).await.unwrap();
    let red = outbox["messages"].as_array().unwrap().last().unwrap().clone();
    assert!(red["text"].as_str().unwrap().contains("landing was red"), "{red}");
    assert_eq!((red["outcome"]["outcome"].clone(), red["outcome"]["until"].clone()), (json!("held"), json!(until)));
    let row = crate::fixture::worker(&w.app).await;
    assert_eq!(row["status"], "waiting");
    crate::fixture::refused(w.app.worker_waiting(id(&wk), "a build", 60_000).await, "waiting");

    // Parked: no finding, no round, its reset in doctor; the pinned
    // checkout starts no new worker on the spent harness.
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    w.advance(170);
    let t = w.tick().await;
    assert!(t.started.is_none(), "{t:?}");
    assert!(t.held.as_deref().unwrap().contains("stablehand picks none until"), "{t:?}");
    let d = w.app.doctor().await.unwrap();
    let key = format!("worker:{}", wk["worker"].as_str().unwrap());
    assert!(!finding_keys(&d).contains(&key), "{d}");
    assert_eq!(
        (d["limited"][0]["worker"].clone(), d["limited"][0]["until"].clone()),
        (wk["worker"].clone(), json!(until))
    );
    assert_eq!(w.app.watch_next().await.unwrap()["round"], json!(null), "no round for it");

    // A word for it meanwhile is held too, and a comment on its issue.
    let told = w.app.watch_tell(id(&wk), "go on", false).await.unwrap();
    assert_eq!((told["delivery"].clone(), sent(&w, &wk).len()), (json!("held"), typed), "{told}");
    assert!(told["missed"].as_str().unwrap().contains("at its limit"), "{told}");
    let shown = w.app.issue_comment("web", 1, "use v2").await.unwrap();
    assert_eq!(shown["delivery"], "held", "{shown}");

    // At the reset, to the ms, back to work: what was held first, then the
    // word of the reset.
    w.clock.advance(until - w.app_now());
    let t = w.tick().await;
    assert_eq!(t.resumed.len(), 1, "{t:?}");
    assert_eq!(t.resumed[0]["resumed"], true, "{t:?}");
    assert!(t.started.is_some(), "the harness is free again: {t:?}");
    assert_eq!(w.worker_on("web", 1).await["status"], "working");
    let now_typed = sent(&w, &wk);
    let [red, word, comment, reset] = &now_typed[now_typed.len() - 4..] else { unreachable!() };
    assert!(red.contains("landing was red") && word.contains("go on"), "{now_typed:?}");
    assert!(comment.ends_with("use v2"), "{now_typed:?}");
    assert!(reset.contains("limit reset"), "{now_typed:?}");
    assert_eq!(w.app.doctor().await.unwrap()["limited"], json!([]));

    // Its old quota, still on its screen, is no new limit; a new one is.
    let t = w.tick().await;
    assert!(t.seen.is_empty(), "{t:?}");
    assert_eq!(w.worker_on("web", 1).await["status"], "working");
    spent(&w, &wk, 2);
    let t = w.tick().await;
    assert_eq!(t.seen[0]["limited"], w.app_now() + RESETS_IN, "{t:?}");
}

#[tokio::test]
async fn a_checkout_not_pinned_starts_its_next_worker_on_another_harness_while_agy_is_spent() {
    let w = World::new();
    w.checkout("web").await;
    w.kit_of(&["agy=agy", CLAUDE]).await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = crate::fixture::worker(&w.app).await;
    assert!(w.terminal(wk["terminal"].as_str().unwrap()).line.contains("agy"));
    spent(&w, &wk, 1);
    assert!(w.tick().await.seen[0]["limited"].is_i64());
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    let t = w.tick().await;
    assert_eq!(t.started.unwrap()["harness"], "claude-code");
}

#[tokio::test]
async fn a_worker_its_reset_does_not_reach_is_the_watch_s() {
    let w = World::new();
    let wk = w.working("agy=agy", "t").await;
    spent(&w, &wk, 1);
    w.tick().await;
    let key = format!("worker:{}", wk["worker"].as_str().unwrap());
    assert!(!finding_keys(&w.app.doctor().await.unwrap()).contains(&key));
    w.host.orca.fault("send-refused", true);
    w.advance(171);
    let t = w.tick().await;
    assert!(t.resumed[0]["refused"].is_string(), "{t:?}");
    assert_eq!(w.worker_on("web", 1).await["status"], "waiting");
    assert!(finding_keys(&w.app.doctor().await.unwrap()).contains(&key), "no longer parked: the watch's");
}

/// agy takes a message, starts a turn and ends it at once on its quota
/// (factorium#223, round 115): `submitted` is no proof it read it. What it
/// took just before its limit is first seen is held again, and typed at the
/// reset; what it took long before is not.
#[tokio::test]
async fn a_word_submitted_just_before_the_quota_showed_is_typed_again_at_the_reset() {
    let w = World::new();
    let wk = w.working("agy=agy", "t").await;
    let tell = |text: &'static str| w.app.watch_tell(id(&wk), text, false);
    assert_eq!(tell("old word").await.unwrap()["delivery"], "submitted");
    w.advance(9);
    assert_eq!(tell("new word").await.unwrap()["delivery"], "submitted");
    // Seen five minutes after it took the new word, fourteen after the old.
    w.advance(5);
    spent(&w, &wk, 1);
    w.tick().await;
    let outbox = w.app.outbox_of(Some(id(&wk))).await.unwrap();
    let outcomes: Vec<Value> =
        outbox["messages"].as_array().unwrap().iter().map(|m| m["outcome"]["outcome"].clone()).collect();
    assert_eq!(outcomes[outcomes.len() - 2..], [json!("submitted"), json!("held")], "{outbox}");
    w.advance(171);
    assert_eq!(w.tick().await.resumed[0]["resumed"], true);
    let typed = sent(&w, &wk);
    let [again, reset] = &typed[typed.len() - 2..] else { unreachable!() };
    assert!(again.ends_with("new word") && reset.contains("limit reset"), "{typed:?}");
    assert_eq!(typed.iter().filter(|t| t.ends_with("old word")).count(), 1, "{typed:?}");
}

/// A harness that stops on its own words, not a limit (Codex's model at
/// capacity), sends its worker to the watch, its words said.
#[tokio::test]
async fn a_worker_whose_harness_stopped_waits_for_the_watch() {
    let w = World::new();
    let wk = w.working("codex=codex", "t").await;
    w.set_screen(wk["terminal"].as_str().unwrap(), &["Selected model is at capacity. Please try a different model."]);
    let t = w.tick().await;
    assert!(t.seen[0]["waits"].as_str().unwrap().starts_with("its harness stopped: Selected model"), "{t:?}");
}

/// agy's quota as five horizon-labs workers' screens showed it
/// (factorium#232), wrapped at the terminal's width, its reset given or not.
fn wrapped_quota(resets: bool) -> Vec<&'static str> {
    let limits = if resets { "limits. Resets in 2h50m4s." } else { "limits." };
    vec![
        "⚠ Individual quota reached. Please upgrade your subscription to increase your",
        limits,
        "Error ID: baec577a-f31f-4b6e-8719-29962abbe30f-121",
        "────────────────────────────────────────",
        ">",
        "? for shortcuts                                     auto · Gemini 4 Neon · high",
    ]
}

/// A worker already waiting for the watch when its screen shows agy's
/// quota (factorium#232: hung, or a word that did not reach it, both the
/// spent engine's doing) is parked until the reset all the same: limited in
/// doctor, no finding, and back at work on the first tick past the reset.
#[tokio::test]
async fn a_worker_waiting_for_the_watch_at_agy_s_quota_is_parked_until_the_reset() {
    let w = World::new();
    let wk = w.working("agy=agy", "t").await;
    w.app.worker_escalate(id(&wk), "hung").await.unwrap();
    let key = format!("worker:{}", wk["worker"].as_str().unwrap());
    assert!(finding_keys(&w.app.doctor().await.unwrap()).contains(&key));
    w.set_screen(wk["terminal"].as_str().unwrap(), &wrapped_quota(true));

    let until = w.app_now() + RESETS_IN;
    let t = w.tick().await;
    assert_eq!(t.seen[0]["limited"], json!(until), "{t:?}");
    let d = w.app.doctor().await.unwrap();
    assert!(!finding_keys(&d).contains(&key), "{d}");
    assert_eq!(
        (d["limited"][0]["worker"].clone(), d["limited"][0]["until"].clone()),
        (wk["worker"].clone(), json!(until))
    );

    w.clock.advance(until - w.app_now() - 1);
    assert!(w.tick().await.resumed.is_empty(), "not before the reset");
    w.clock.advance(1);
    assert_eq!(w.tick().await.resumed[0]["resumed"], true);
    assert_eq!(w.worker_on("web", 1).await["status"], "working");
    assert_eq!(w.app.doctor().await.unwrap()["limited"], json!([]));
}

/// agy's quota with no reset says its account is used up (SPEC §1.7): the
/// worker ends, its issue to start again, and agy on the host's own login
/// starts no worker for an hour; no finding meanwhile.
#[tokio::test]
async fn agy_s_quota_with_no_reset_ends_its_worker_and_rests_its_account_until_stablehand_reads_it() {
    let w = World::new();
    let wk = w.working("agy=agy", "t").await;
    w.set_screen(wk["terminal"].as_str().unwrap(), &wrapped_quota(false));
    let t = w.tick().await;
    assert_eq!(t.seen[0]["ended"], "quota", "{t:?}");
    assert!(t.started.is_none(), "agy rests: {t:?}");
    let key = format!("worker:{}", wk["worker"].as_str().unwrap());
    assert!(!finding_keys(&w.app.doctor().await.unwrap()).contains(&key));
    w.advance(60);
    assert!(w.tick().await.started.is_none(), "until stablehand reads it again");
    w.host.stablehand.lock().unwrap().read("agy");
    assert_eq!(w.tick().await.started.unwrap()["issue"], "web#1");
}
