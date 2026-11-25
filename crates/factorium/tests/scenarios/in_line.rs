//! A landing waiting its turn is checked for its merge onto the proving tip
//! as soon as `done` asks, and again on every move of the tip; one that no
//! longer merges is told at once and leaves the line, without waiting its
//! turn (factorium#307).

use crate::fixture::{World, worktree};

/// Three workers at work on `web`, on #1 to #3.
async fn three_at_work(w: &World) {
    w.checkout("web").await;
    w.kit().await;
    w.app.settings_set(Some("silent_bound"), Some(1_000_000)).await.unwrap();
    for title in ["a", "b", "c"] {
        w.app.issue_add("web", title, "", false, &[]).await.unwrap();
    }
    for _ in 0..3 {
        w.tick().await;
    }
}

/// What the tick said of the worker on `web`#`n`'s landing, if it was red.
async fn red_of(w: &World, landed: &[serde_json::Value], n: u64) -> Option<String> {
    let wk = w.worker_on("web", n).await;
    landed.iter().find(|l| l["worker"] == wk["worker"]).and_then(|l| l["red"].as_str().map(str::to_string))
}

#[tokio::test]
async fn a_landing_that_no_longer_merges_once_another_lands_is_told_on_that_tick_ahead_of_its_turn() {
    let w = World::new();
    three_at_work(&w).await;
    // #1 and #3 change the same line; #2 asks before #3.
    for (n, file, text) in [(1, "README", "one\n"), (2, "two.txt", "two\n"), (3, "README", "three\n")] {
        w.done(&w.worker_on("web", n).await, file, text).await;
        w.advance(1);
    }
    let t = w.tick_alone().await;
    assert!(t.landed.iter().all(|l| l.get("red").is_none()), "all merge onto the tip: {:?}", t.landed);
    w.app.moves_ended().await;

    let t = w.tick_alone().await;
    assert!(t.landed.iter().any(|l| l["landed"] == "web#1"), "{:?}", t.landed);
    let red = red_of(&w, &t.landed, 3).await.unwrap_or_else(|| panic!("#3 told now: {:?}", t.landed));
    assert!(red.contains("does not merge") && red.contains("conflicts in: README"), "{red}");
    assert_eq!(w.worker_on("web", 3).await["status"], "working", "it left the line");
    assert_eq!(w.worker_on("web", 2).await["status"], "landing", "#2 merges and keeps its place");
    let wk = w.worker_on("web", 3).await;
    let told = w.terminal(wk["terminal"].as_str().unwrap()).sent;
    assert!(told.last().unwrap().contains("does not merge"), "{told:?}");
    assert_eq!(w.tick().await.landed[0]["landed"], "web#2");
}

#[tokio::test]
async fn done_on_a_branch_that_conflicts_with_the_tip_is_red_on_its_tick_while_another_landing_gates() {
    let w = World::new();
    w.two_at_work().await;
    w.push_by_hand("web", "proving", "README", "theirs\n");
    w.done(&w.worker_on("web", 1).await, "one.txt", "x").await;
    let t = w.tick_alone().await;
    assert!(t.landed.is_empty() && w.app.moves_run(), "#1's fast gate runs beside: {:?}", t.landed);

    w.done(&w.worker_on("web", 2).await, "README", "mine\n").await;
    let t = w.tick_alone().await;
    let red = red_of(&w, &t.landed, 2).await.unwrap_or_else(|| panic!("#2 red now: {:?}", t.landed));
    assert!(red.contains("does not merge") && red.contains("conflicts in: README"), "no gate ran: {red}");
    assert_eq!(w.worker_on("web", 2).await["status"], "working");
    let mut landed = t.landed;
    landed.extend(w.tick().await.landed);
    assert!(landed.iter().any(|l| l["landed"] == "web#1"), "{landed:?}");
}

#[tokio::test]
async fn a_conflict_only_in_a_regenerated_record_is_not_red_and_keeps_its_place() {
    let w = World::new();
    w.two_at_work().await;
    let toml =
        "[gate]\nfast = \"test ! -e RED && git log -1 --format=%P > rec/gen.json\"\nregenerated = [\"rec/*.json\"]\n";
    w.push_by_hand("web", "proving", "factorium.toml", toml);
    std::fs::create_dir_all(w.dir.join("by-hand/rec")).unwrap();
    w.push_by_hand("web", "proving", "rec/gen.json", "start\n");
    w.done(&w.worker_on("web", 2).await, "two.txt", "x").await;
    w.advance(1);
    let wk = w.worker_on("web", 1).await;
    std::fs::create_dir_all(worktree(&wk).join("rec")).unwrap();
    w.done(&wk, "rec/gen.json", "mine\n").await;

    let t = w.tick_alone().await;
    assert!(red_of(&w, &t.landed, 1).await.is_none(), "{:?}", t.landed);
    assert_eq!(w.worker_on("web", 1).await["status"], "landing", "it keeps its place");
    let mut t = w.tick().await;
    t.landed.extend(w.tick().await.landed);
    let landed: Vec<_> = t.landed.iter().filter_map(|l| l["landed"].as_str()).collect();
    assert_eq!(landed, ["web#2", "web#1"], "{:?}", t.landed);
}

#[tokio::test]
async fn a_merge_check_git_refuses_is_red_in_gits_words() {
    let w = World::new();
    w.two_at_work().await;
    let wk = w.worker_on("web", 1).await;
    let (tree, branch) = (worktree(&wk), wk["branch"].as_str().unwrap().to_string());
    // A branch of a history of its own: git refuses to merge it at all.
    w.git(&tree, &["checkout", "-q", "--orphan", "alone"]).unwrap();
    w.git(&tree, &["commit", "-qm", "alone"]).unwrap();
    w.git(&tree, &["branch", "-qf", &branch, "alone"]).unwrap();
    w.git(&tree, &["checkout", "-q", &branch]).unwrap();
    w.app.worker_done(crate::fixture::id(&wk), None).await.unwrap();
    let t = w.tick_alone().await;
    let red = red_of(&w, &t.landed, 1).await.unwrap_or_else(|| panic!("red: {:?}", t.landed));
    assert!(red.contains("does not merge") && red.contains("unrelated histories"), "{red}");
}
