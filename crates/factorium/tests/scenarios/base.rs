//! The checkout's local base follows origin's: fast-forwarded after a
//! promotion and on every tick, never further than origin's, and never
//! over anything of its own.

use std::path::Path;

use crate::World;
use crate::line::land;

/// The checkout's local `main`.
fn local(w: &World, web: &Path) -> String {
    w.git(web, &["rev-parse", "refs/heads/main"]).unwrap()
}

#[tokio::test]
async fn a_promotion_moves_the_checked_out_local_base_and_its_tree() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    // Idle once it lands: the same tick promotes.
    let landed = land(&w, 1, "a").await;
    assert_eq!(w.remote_tip("web", "main"), landed);
    assert_eq!(local(&w, &web), landed, "the local base follows");
    assert!(web.join("a").exists(), "and the tree it is checked out in");
    assert!(w.git(&web, &["status", "--porcelain"]).unwrap().is_empty());
    assert_eq!(w.finding("base:web").await, None);
}

#[tokio::test]
async fn a_promotion_by_hand_moves_a_local_base_not_checked_out() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    w.git(&web, &["checkout", "-q", "-b", "elsewhere"]).unwrap();
    // #2 keeps the checkout busy, so only the hand promotes.
    w.app.settings_set(Some("silent_bound"), Some(100_000)).await.unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "b", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    let landed = land(&w, 1, "a").await;
    let promoted = w.app.promote("web").await.unwrap();
    assert_eq!(promoted["tip"], landed.as_str(), "{promoted}");
    assert_eq!(promoted["local"]["fast_forwarded"], 2, "the commit and its landing: {promoted}");
    assert_eq!(local(&w, &web), landed);
    assert_eq!(w.git(&web, &["branch", "--show-current"]).unwrap(), "elsewhere", "what is checked out stays");
}

#[tokio::test]
async fn a_local_base_with_its_own_commits_is_left_and_is_a_finding() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    w.commit(&web, "mine", "x");
    let mine = local(&w, &web);
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let landed = land(&w, 1, "a").await;
    assert_eq!(w.remote_tip("web", "main"), landed, "origin's base is promoted");
    assert_eq!(local(&w, &web), mine, "the local base keeps its own commit");
    let what = w.finding("base:web").await.expect("a finding")["what"].to_string();
    assert!(what.contains("2 commits behind origin/main") && what.contains("1 commits of its own"), "{what}");
    // The owner drops its own commit: the next tick fast-forwards it.
    w.git(&web, &["reset", "-q", "--hard", "HEAD~1"]).unwrap();
    w.tick().await;
    assert_eq!(local(&w, &web), landed);
    assert_eq!(w.finding("base:web").await, None);
}

#[tokio::test]
async fn a_local_base_checked_out_over_changes_is_left_and_is_a_finding() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    let before = local(&w, &web);
    std::fs::write(web.join("README"), "edited\n").unwrap();
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let landed = land(&w, 1, "a").await;
    assert_eq!(local(&w, &web), before, "nothing moves under the changes");
    assert_eq!(std::fs::read_to_string(web.join("README")).unwrap(), "edited\n");
    let what = w.finding("base:web").await.expect("a finding")["what"].to_string();
    assert!(what.contains("2 commits behind") && what.contains("changes not committed"), "{what}");
    // The changes put aside: the next tick fast-forwards it.
    w.git(&web, &["checkout", "-q", "--", "README"]).unwrap();
    w.tick().await;
    assert_eq!(local(&w, &web), landed);
    assert_eq!(w.finding("base:web").await, None);
}

#[tokio::test]
async fn a_local_base_left_behind_catches_up_on_the_next_tick() {
    let w = World::new();
    let web = w.checkout("web").await;
    w.kit().await;
    let start = local(&w, &web);
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    w.tick().await;
    let landed = land(&w, 1, "a").await;
    // Left behind, as a base nothing moved was.
    w.git(&web, &["reset", "-q", "--hard", &start]).unwrap();
    assert_eq!(w.finding("base:web").await, None, "behind and nothing of its own: not a finding, it follows");
    let t = w.tick().await;
    assert!(t.lines.iter().any(|l| l["base"] == "web" && l["fast_forwarded"] == 2), "{:?}", t.lines);
    assert_eq!(local(&w, &web), landed);
    assert!(w.tick().await.lines.is_empty(), "converged: nothing more to do");
}
