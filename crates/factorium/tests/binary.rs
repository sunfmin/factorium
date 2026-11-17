//! The built binary: arguments parse, answers are JSON, refusals exit 1,
//! and the database is where the environment says.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let d = std::env::temp_dir()
            .join("factorium-binary")
            .join(uuid::Uuid::now_v7().to_string());
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the binary with `XDG_DATA_HOME` in `dir`; answers its exit success
/// and its printed JSON.
fn run(dir: &Dir, args: &[&str]) -> (bool, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_factorium"))
        .args(args)
        .env("XDG_DATA_HOME", dir.0.join("data"))
        .env_remove("FACTORIUM_DB")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let json = serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text:?}"));
    (out.status.success(), json)
}

#[test]
fn the_cli_records_in_the_xdg_database_and_answers_json() {
    let dir = Dir::new();
    let repo = dir.0.join("web");
    std::fs::create_dir_all(&repo).unwrap();

    let (ok, _) = run(&dir, &["checkout", "add", "web", repo.to_str().unwrap()]);
    assert!(ok);
    assert!(dir.0.join("data/factorium/factorium.db").exists());

    let (ok, issue) = run(&dir, &["issue", "add", "web", "Fix it", "--body", "how"]);
    assert!(ok);
    assert_eq!(
        (issue["number"].clone(), issue["body"].clone()),
        (json!(1), json!("how"))
    );

    let (ok, list) = run(&dir, &["issue", "list", "web"]);
    assert!(ok);
    assert_eq!(list[0]["title"], "Fix it");

    let (_, checkouts) = run(&dir, &["checkout", "list"]);
    assert_eq!(checkouts[0]["proving"], "proving");
}

#[test]
fn a_refusal_exits_one_with_its_code() {
    let dir = Dir::new();
    let (ok, answer) = run(&dir, &["issue", "show", "web", "1"]);
    assert!(!ok);
    assert_eq!(answer["error"]["code"], "no_issue");
}
