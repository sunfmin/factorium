//! A fit configured at a repo's root and never run by the gate factorium
//! runs for it checks nothing: doctor finds it, `checkout add` warns of it,
//! and `checkout show` says, for each fit, where its gate runs it or why the
//! repo leaves it out.

use serde_json::{Value, json};

use crate::fixture::World;

/// Commits each of `files` (path, text) to `web`'s base by hand.
fn base_holds(w: &World, files: &[(&str, &str)]) {
    for (file, text) in files {
        w.push_by_hand("web", "main", file, text);
    }
}

/// Checkout `web` on the list, its base holding `files` as last fetched.
async fn web_holding(w: &World, files: &[(&str, &str)]) {
    let web = w.checkout("web").await;
    base_holds(w, files);
    w.git(&web, &["fetch", "-q", "origin"]).unwrap();
}

const GATED_BY_SCRIPT: &str = "[gate]\nfast = \"./gate.sh\"\n";

/// `checkout show`'s fit named `fit`.
async fn shown(w: &World, fit: &str) -> Value {
    let show = w.app.checkout_show("web").await.unwrap();
    let found = show["fits"].as_array().unwrap().iter().find(|f| f["fit"] == fit).cloned();
    found.unwrap_or_else(|| panic!("no {fit} in {show}"))
}

#[tokio::test]
async fn a_truefit_config_its_gate_never_runs_is_a_finding_naming_the_checkout_the_fit_and_the_file() {
    let w = World::new();
    let gate = "#!/bin/sh\n# truefit contract is run elsewhere, they say\ncargo test\n";
    web_holding(&w, &[("truefit.toml", "[contract]\n"), ("gate.sh", gate), ("factorium.toml", GATED_BY_SCRIPT)]).await;
    let found = w.finding("unrun:web:truefit").await.expect("a finding for truefit");
    let (what, fix) = (found["what"].as_str().unwrap(), found["fix"].as_str().unwrap());
    assert!(what.contains("web") && what.contains("truefit.toml") && what.contains("`truefit contract`"), "{what}");
    assert!(fix.contains("add `truefit contract` to gate.sh") && fix.contains("[gate.left_out]"), "{fix}");
    assert!(!w.finding_keys().await.iter().any(|k| k.starts_with("unrun:web:") && k != "unrun:web:truefit"));
    assert_eq!(shown(&w, "truefit").await["state"], "unrun");
}

#[tokio::test]
async fn a_fit_whose_check_a_script_the_gate_runs_calls_is_no_finding() {
    let w = World::new();
    let gate = "#!/bin/sh\nset -eu\ncargo test\ntruefit contract\n";
    let toml = "[gate]\nfast = \"cargo fmt --check\"\nslow = \"sh ./gate.sh\"\n";
    web_holding(&w, &[("truefit.toml", "[contract]\n"), ("gate.sh", gate), ("factorium.toml", toml)]).await;
    assert_eq!(w.finding("unrun:web:truefit").await, None);
    assert_eq!(
        shown(&w, "truefit").await,
        json!({ "fit": "truefit", "config": "truefit.toml", "call": "truefit contract", "state": "run", "in": "gate.sh" })
    );
}

#[tokio::test]
async fn a_fit_left_out_with_its_reason_is_no_finding_and_checkout_show_gives_the_reason() {
    let w = World::new();
    let toml = format!("{GATED_BY_SCRIPT}\n[gate.left_out]\nslimfit = \"the suite runs on the build host\"\n");
    web_holding(&w, &[("slimfit.toml", "[check]\n"), ("gate.sh", "cargo test\n"), ("factorium.toml", &toml)]).await;
    assert_eq!(w.finding("unrun:web:slimfit").await, None);
    let slimfit = shown(&w, "slimfit").await;
    assert_eq!(
        (slimfit["state"].clone(), slimfit["why"].clone()),
        (json!("left_out"), json!("the suite runs on the build host"))
    );
}

#[tokio::test]
async fn left_out_naming_no_fit_is_refused_naming_the_fits() {
    let w = World::new();
    let toml = format!("{GATED_BY_SCRIPT}\n[gate.left_out]\ntrufit = \"typo\"\n");
    web_holding(&w, &[("factorium.toml", &toml)]).await;
    let refused = w.app.checkout_show("web").await.unwrap_err();
    assert_eq!(refused.code, "bad_declaration");
    assert!(refused.message.contains("`trufit`") && refused.message.contains("slimfit, truefit, docfit"), "{refused}");
}

#[tokio::test]
async fn checkout_add_warns_of_a_fit_its_gate_never_runs_as_doctor_finds_it() {
    let w = World::new();
    let path = w.repo("web");
    base_holds(&w, &[("docfit.toml", "[budget]\n"), ("factorium.toml", "[gate]\nfast = \"cargo test\"\n")]);
    let added = w.app.checkout_add("web", &path, "main", "proving", None).await.unwrap();
    let warnings = added["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{added}");
    assert_eq!(warnings[0], json!(w.finding("unrun:web:docfit").await.unwrap()));
    assert!(warnings[0]["fix"].as_str().unwrap().contains("to factorium.toml's [gate]"), "{added}");
}
