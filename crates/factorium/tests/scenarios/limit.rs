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
    w.app.kit_set(&["agy=agy".into()]).await.unwrap();
    w.app.kit_pin("web", Some("agy")).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = crate::fixture::worker(&w.app).await;
    w.watch_session(false);
    let tree = worktree(&wk);
    w.commit(&tree, "RED", "x");
    w.app.worker_done(id(&wk)).await.unwrap();
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

    // Parked: no finding, no round, its reset in doctor; the pinned
    // checkout starts no new worker on the spent harness.
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    w.advance(170);
    let t = w.tick().await;
    assert!(t.started.is_none(), "{t:?}");
    assert!(t.held.as_deref().unwrap().contains("agy: at its limit until it resets at"), "{t:?}");
    let d = w.app.doctor().await.unwrap();
    let key = format!("worker:{}", wk["worker"].as_str().unwrap());
    assert!(!finding_keys(&d).contains(&key), "{d}");
    assert_eq!(
        (d["limited"][0]["worker"].clone(), d["limited"][0]["until"].clone()),
        (wk["worker"].clone(), json!(until))
    );
    assert_eq!(w.app.watch_next().await.unwrap()["round"], json!(null), "no round for it");

    // At the reset, back to work: the red first, then the word of the reset.
    w.advance(1);
    let t = w.tick().await;
    assert_eq!(t.resumed.len(), 1, "{t:?}");
    assert_eq!(t.resumed[0]["resumed"], true, "{t:?}");
    assert!(t.started.is_some(), "the harness is free again: {t:?}");
    assert_eq!(w.worker_on("web", 1).await["status"], "working");
    let typed = sent(&w, &wk);
    let [red, reset] = &typed[typed.len() - 2..] else { unreachable!() };
    assert!(red.contains("landing was red") && reset.contains("limit reset"), "{typed:?}");
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
    w.app.kit_set(&["agy=agy".into(), CLAUDE.into()]).await.unwrap();
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
