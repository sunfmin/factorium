//! A host that cuts no version of factorium still upgrades it from the tap
//! (factorium#269): each `upgrade_every`, while no line move runs, beside
//! the ticks; the daemon swaps to what that links once it proves, and one
//! that fails the proof is a finding naming its version and why.

use serde_json::Value;

use crate::fixture::{LOOK, NEEDS_NEWER, STUCK, World};

/// Ticks until the upgrade from the tap that a tick started ends: what that tick answered of it.
async fn upgraded(w: &World) -> Value {
    let begun = std::time::Instant::now();
    loop {
        if let Some(u) = w.tick().await.upgrade.filter(|u| u.get("upgraded").is_some()) {
            return u;
        }
        assert!(begun.elapsed() < STUCK, "the upgrade ends");
        tokio::time::sleep(LOOK).await;
    }
}

/// Ticks until one asks for a restart, or the bound passes: the restart.
async fn restart(w: &World) -> Option<String> {
    let begun = std::time::Instant::now();
    while begun.elapsed() < std::time::Duration::from_secs(5) {
        if let Some(r) = w.tick().await.restart {
            return Some(r);
        }
        tokio::time::sleep(LOOK).await;
    }
    None
}

fn upgrades(w: &World) -> usize {
    w.host.brewed().iter().filter(|b| *b == "upgrade factorium").count()
}

#[tokio::test]
async fn a_host_that_does_not_cut_factorium_upgrades_it_from_the_tap_and_swaps_once_proven() {
    let w = World::new();
    // The host's one checkout cuts its own formula (`web`), not factorium.
    w.checkout("web").await;
    let link = w.running_as(&w.app, "factorium");
    w.host.brew.lock().unwrap().tap.insert("factorium".into(), "0.1.0".into());

    let u = upgraded(&w).await;
    assert_eq!(u["upgraded"], true, "{u}");
    assert_eq!(upgrades(&w), 1);
    assert_eq!(w.host.brew.lock().unwrap().installed["factorium"], "0.1.0");
    assert_eq!(restart(&w).await, Some(link.to_string_lossy().into_owned()), "it swaps by the existing rule");
    assert_eq!(w.finding("daemon:upgrade").await, None);

    // Not again before `upgrade_every` (an hour) since that try; again once it passed.
    w.advance(59);
    w.tick().await;
    assert_eq!(upgrades(&w), 1);
    w.advance(1);
    upgraded(&w).await;
    assert_eq!(upgrades(&w), 2);
}

#[tokio::test]
async fn a_version_from_the_tap_that_fails_its_proof_leaves_the_old_one_running_and_is_a_finding() {
    let w = World::new();
    w.running_as(&w.app, "factorium");
    // The tap's binary needs a newer database than this host's.
    w.next_binary("factorium", NEEDS_NEWER);
    w.host.brew.lock().unwrap().tap.insert("factorium".into(), "0.1.0".into());
    upgraded(&w).await;
    assert_eq!(restart(&w).await, None, "the old one runs on");
    let found = w.finding("daemon:refused:0.1.0").await.expect("the refusal is a finding");
    let what = found["what"].as_str().unwrap();
    assert!(what.contains("factorium 0.1.0") && what.contains("no such column: handed_back"), "{what}");
}

#[tokio::test]
async fn a_failed_upgrade_is_a_finding_until_one_works() {
    let w = World::new();
    w.running_as(&w.app, "factorium");
    w.host.brew.lock().unwrap().refuses = Some("Error: Failure while executing; `curl` exited with 6".into());
    assert_eq!(upgraded(&w).await["upgraded"], false);
    let found = w.finding("daemon:upgrade").await.expect("a finding");
    assert!(found["what"].as_str().unwrap().contains("curl"), "{found}");
    w.host.brew.lock().unwrap().refuses = None;
    w.advance(60);
    assert_eq!(upgraded(&w).await["upgraded"], true);
    assert_eq!(w.finding("daemon:upgrade").await, None);
}

#[tokio::test]
async fn a_host_that_cuts_the_daemons_formula_installs_it_only_at_the_cut() {
    let w = World::new();
    // The `web` checkout's base declares the cut of formula `web`: the daemon's own.
    w.checkout("web").await;
    w.running_as(&w.app, "web");
    for _ in 0..3 {
        assert_eq!(w.tick().await.upgrade, None);
        w.advance(61);
    }
    assert!(!w.host.brewed().iter().any(|b| b.starts_with("upgrade")), "{:?}", w.host.brewed());
}
