//! The repo's own gate on a crate's lines (SPEC §5, factorium#163): a
//! target is a trend shown by `./gate.sh`, never a red, and a crate that
//! keeps growing is a task its idle work files. Run on a copy of the repo's
//! `gate.sh`, `checks.sh` and `lines.sh`, in a repo of one crate, with
//! `cargo`, `slimfit`, `truefit`, `docfit` and the release scripts stubbed
//! green so only the lines are measured.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// A repo of one crate, `crates/web/src`, with a target of 10 lines, holding
/// the repo's gate scripts; and the stubs' directory.
fn repo() -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join("factorium-lines").join(uuid::Uuid::now_v7().to_string());
    let stubs = dir.join("stubs");
    std::fs::create_dir_all(dir.join("crates/web/src")).unwrap();
    std::fs::create_dir_all(&stubs).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for script in ["gate.sh", "checks.sh", "lines.sh"] {
        std::fs::copy(root.join(script), dir.join(script)).unwrap();
    }
    std::fs::create_dir_all(dir.join("packaging")).unwrap();
    for stub in [
        "stubs/cargo",
        "stubs/slimfit",
        "stubs/truefit",
        "stubs/docfit",
        "packaging/release.sh",
        "packaging/release-test.sh",
    ] {
        std::fs::write(dir.join(stub), "#!/bin/sh\nexit 0\n").unwrap();
    }
    std::fs::write(
        dir.join("SPEC.md"),
        "| Crate | Holds | Target (source lines) |\n|---|---|---|\n| `factorium-web` | The web. | 10 |\n",
    )
    .unwrap();
    sh(&dir, &stubs, "chmod +x stubs/* packaging/* && git init -q && git add -A && git commit -qm specs");
    (dir, stubs)
}

fn sh(dir: &Path, stubs: &Path, line: &str) -> Output {
    let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
    let out = Command::new("sh")
        .args(["-c", line])
        .current_dir(dir)
        .env("PATH", path)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "{line}: {}", String::from_utf8_lossy(&out.stderr));
    out
}

/// Lands one commit after another: each `(file, lines)` writes that many
/// lines to `file` under the crate's source.
fn land(dir: &Path, stubs: &Path, landings: &[(&str, usize)]) {
    for (file, n) in landings {
        std::fs::write(dir.join("crates/web/src").join(file), "x\n".repeat(*n)).unwrap();
        sh(dir, stubs, &format!("git add -A && git commit -qm '{file} at {n}'"));
    }
}

fn tasks(dir: &Path, stubs: &Path) -> Vec<Value> {
    let out = sh(dir, stubs, "./lines.sh --tasks");
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["by"], "lines.sh", "{answer}");
    answer["tasks"].as_array().unwrap().clone()
}

#[test]
fn a_crate_over_its_target_leaves_the_gate_green_and_shows_by_how_much() {
    let (dir, stubs) = repo();
    land(&dir, &stubs, &[("a.rs", 8), ("b.rs", 6)]);
    let out = sh(&dir, &stubs, "./gate.sh");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("web") && said.contains("14 /    10 lines  over by 4"), "{said}");

    let tasks = tasks(&dir, &stubs);
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0]["kind"], "crate-lines");
    assert_eq!(tasks[0]["summary"], "crates/web: 14 lines, over its target of 10 by 4");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_crate_that_grew_at_each_of_its_last_five_changes_is_a_task_naming_the_files_that_grew() {
    let (dir, stubs) = repo();
    // Under its target throughout: a fall, then rises, with a landing that
    // leaves it be among them.
    land(&dir, &stubs, &[("a.rs", 3), ("a.rs", 2), ("a.rs", 3), ("b.rs", 1), ("b.rs", 2), ("c.rs", 0)]);
    assert_eq!(tasks(&dir, &stubs), Vec::<Value>::new(), "three rises since its fall: no task");
    land(&dir, &stubs, &[("b.rs", 3), ("c.rs", 1)]);
    let tasks = tasks(&dir, &stubs);
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0]["summary"], "crates/web: rose at each of its last 5 landings that changed it, from 2");
    let files: Vec<&str> = tasks[0]["files"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(files, ["crates/web/src/b.rs", "crates/web/src/c.rs", "crates/web/src/a.rs"], "most grown first");
    let _ = std::fs::remove_dir_all(&dir);
}
