//! A landing whose push took effect, the daemon never knowing (models/
//! Line.tla, factorium#188): the push's answer lost, or the daemon gone
//! between recording the landing and ending its worker. The next tick lands
//! it again: merged already, green already, pushed already.

use crate::fixture::{CLAUDE, World, worker};

#[tokio::test]
async fn a_landing_whose_push_answer_is_lost_lands_again_and_closes_its_issue() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    let start = w.remote_tip("web", "proving");
    let wk = worker(&w.app).await;
    w.lose_next_push("web");
    w.done(&wk, "a", "x").await;
    let t = w.tick().await;
    assert!(t.landed.iter().any(|l| l["retry"].is_string()), "the push answered an error: {:?}", t.landed);
    let pushed = w.remote_tip("web", "proving");
    assert_ne!(pushed, start, "and took effect");
    let t = w.tick().await;
    assert_eq!(t.landed[0]["landed"], "web#1", "{:?}", t.landed);
    assert_eq!(t.landed[0]["commit"], pushed.as_str(), "the commit it pushed, not another merge");
    assert_eq!(w.remote_tip("web", "proving"), pushed);
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], "closed");
    assert!(t.lines.iter().any(|l| l["promoted"] == "web"), "then promoted: {:?}", t.lines);
}

#[tokio::test]
async fn a_daemon_gone_after_recording_a_landing_lands_it_again_and_closes_its_issue() {
    let w = World::new();
    w.working(CLAUDE, "a").await;
    let wk = worker(&w.app).await;
    // The landing is recorded; the store takes nothing after it, as if the daemon died there.
    w.disk_refuses("SessionClosed", true);
    w.done(&wk, "a", "x").await;
    assert!(w.app.tick().await.is_err(), "the tick stops after recording the landing");
    w.disk_refuses("SessionClosed", false);
    let pushed = w.remote_tip("web", "proving");
    let t = w.app.tick().await.expect("the next tick lands it again, recorded already");
    assert_eq!(t.landed[0]["landed"], "web#1", "{:?}", t.landed);
    assert_eq!(t.landed[0]["commit"], pushed.as_str());
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["status"], "closed");
}
