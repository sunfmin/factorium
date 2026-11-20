//! What a scenario starts never outlives it: every fixture that holds runs
//! the one loop [`World::holds`] gives, which ends once its dir is gone or
//! after a time cap, not on its release file alone (factorium#70).

use std::time::{Duration, Instant};

use crate::fixture::World;

#[test]
fn a_held_fixture_whose_world_is_dropped_unreleased_ends_within_two_seconds() {
    let w = World::new();
    let dir = w.dir.join("held");
    let mut child = std::process::Command::new("sh").args(["-c", &w.holds(&dir)]).spawn().unwrap();
    while !dir.join("started").exists() {
        std::thread::sleep(crate::fixture::LOOK);
    }
    drop(w);
    let dropped = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if dropped.elapsed() > Duration::from_secs(2) {
            child.kill().unwrap();
            panic!("the fixture outlived its world");
        }
        std::thread::sleep(crate::fixture::LOOK);
    };
    assert_eq!(status.code(), Some(1), "it ends unreleased");
}

#[test]
fn no_scenario_starts_a_loop_but_the_one_world_holds_gives() {
    // Split so this file's own text is no match.
    let shell_loop = concat!("; d", "o ");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios");
    let (mut found, mut dirs) = (Vec::new(), vec![root.clone()]);
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let name = path.strip_prefix(&root).unwrap().to_string_lossy().into_owned();
                found.extend(std::iter::repeat_n(name, text.matches(shell_loop).count()));
            }
        }
    }
    assert_eq!(found, ["fixture/world.rs"], "a shell loop outside World::holds");
}
