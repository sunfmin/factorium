//! A role checked before a worker starts with it (factorium#277): what
//! `rolesmith doctor --project <role>` says the role needs of this host is
//! there, a program it lacks installed first; what is still lacking holds
//! the role's issues and is handed to the watch as a doctor finding.

use factorium::host::Host;
use factorium_domain::issue::On;
use serde_json::json;

use crate::fixture::World;

/// A role directory under the world's home; with `record`, one rolesmith
/// checks (it holds `rolesmith.toml`).
fn role(w: &World, name: &str, record: bool) -> String {
    let dir = w.host.home().join("roles").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    if record {
        std::fs::write(dir.join("rolesmith.toml"), "[[role]]\nname = \"coder\"\n").unwrap();
    }
    dir.to_string_lossy().into_owned()
}

/// The rolesmith commands run, oldest first, from the `n`th program run on.
fn rolesmith_ran(w: &World, n: usize) -> Vec<String> {
    let ran = w.host.ran.lock().unwrap();
    ran[n..].iter().filter_map(|r| r.strip_prefix("rolesmith ")).map(String::from).collect()
}

fn ran_so_far(w: &World) -> usize {
    w.host.ran.lock().unwrap().len()
}

fn started(t: &factorium::Tick) -> Option<serde_json::Value> {
    t.started.as_ref().map(|s| s["issue"].clone())
}

#[tokio::test]
async fn a_role_with_all_it_needs_or_no_record_starts_and_one_lacking_a_program_starts_once_it_is_installed() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    let on = |r: &str| On { role: Some(r.into()), account: None };

    // No rolesmith.toml: nothing is checked, and it starts.
    let plain = role(&w, "plain", false);
    w.app.issue_add_on("web", "Plain", "", None, &[], on(&plain)).await.unwrap();
    let n = ran_so_far(&w);
    assert_eq!(started(&w.tick().await), Some(json!("web#1")));
    assert_eq!(rolesmith_ran(&w, n), Vec::<String>::new(), "a role with no record needs nothing");

    // All there: checked, and it starts.
    let ready = role(&w, "ready", true);
    w.app.issue_add_on("web", "Ready", "", None, &[], on(&ready)).await.unwrap();
    let n = ran_so_far(&w);
    assert_eq!(started(&w.tick().await), Some(json!("web#2")));
    assert_eq!(rolesmith_ran(&w, n), [format!("doctor --project {ready} --json")]);

    // A program it lacks that rolesmith can install: installed, checked again, and it starts.
    let ios = role(&w, "ios", true);
    w.host.rolesmith.lock().unwrap().lacks_program(&ios, "metal");
    w.app.issue_add_on("web", "Metal", "", None, &[], on(&ios)).await.unwrap();
    let n = ran_so_far(&w);
    assert_eq!(started(&w.tick().await), Some(json!("web#3")));
    let doctor = format!("doctor --project {ios} --json");
    assert_eq!(rolesmith_ran(&w, n), [doctor.clone(), format!("tool install --project {ios} --json"), doctor]);
    assert_eq!(w.finding(&format!("role:{ios}")).await, None, "nothing is lacking");
}

#[tokio::test]
async fn a_role_lacking_what_cannot_be_installed_holds_its_issues_and_is_the_watchs_checked_once_a_tick() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    // The checkout's default role, which its first two issues play.
    let ios = role(&w, "ios", true);
    w.app.checkout_edit("web", Some(Some(&ios)), None).await.unwrap();
    {
        let mut rolesmith = w.host.rolesmith.lock().unwrap();
        rolesmith.lacks_program(&ios, "metal");
        rolesmith.lacks_entry(&ios, "apple-id");
        rolesmith.uninstallable.push("metal".into());
    }
    w.app.issue_add("web", "One", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "Two", "", false, &[]).await.unwrap();
    let plain = role(&w, "plain", false);
    let on = On { role: Some(plain), account: None };
    w.app.issue_add_on("web", "Three", "", None, &[], on).await.unwrap();

    // Its two issues wait, the role checked once in the tick; the issue behind them starts.
    let n = ran_so_far(&w);
    assert_eq!(started(&w.tick().await), Some(json!("web#3")), "a role's lack holds only its own issues");
    let doctor = format!("doctor --project {ios} --json");
    let install = format!("tool install --project {ios} --json");
    assert_eq!(rolesmith_ran(&w, n), [doctor.clone(), install, doctor.clone()], "once a tick, not once an issue");

    // The watch is handed what it lacks, each with rolesmith's fix.
    let found = w.finding(&format!("role:{ios}")).await.expect("a finding for the watch");
    let what = found["what"].as_str().unwrap();
    assert!(what.contains("program `metal`") && what.contains("entry `apple-id` (no mytokens entry)"), "{what}");
    assert!(what.contains("`rolesmith tool install` failed: tool metal: its script exits 1"), "{what}");
    let fix = found["fix"].as_str().unwrap();
    assert!(fix.contains(&format!("`rolesmith tool install --project {ios}`")), "{fix}");
    assert!(fix.contains("`mytokens put apple-id --fields TOKEN`"), "{fix}");

    // The same lack is not installed again, tick by tick; the finding stands.
    let n = ran_so_far(&w);
    assert_eq!(started(&w.tick().await), None);
    assert_eq!(rolesmith_ran(&w, n), std::slice::from_ref(&doctor));
    let again = w.finding(&format!("role:{ios}")).await.expect("it stands");
    assert!(again["what"].as_str().unwrap().contains("`rolesmith tool install` failed"), "{again}");

    // Once all is there, the next tick starts its oldest issue, and the finding goes.
    w.host.rolesmith.lock().unwrap().lacks.clear();
    assert_eq!(started(&w.tick().await), Some(json!("web#1")));
    assert_eq!(w.finding(&format!("role:{ios}")).await, None);
}
