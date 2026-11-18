//! The stand-in session each harness writes where its turn is read.

use factorium_harness::{Harness, Turn};

/// A fresh, empty directory for one test.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("factorium-harness-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Kimi Code's index names each worktree's session: the stand-in replaces
/// this worktree's line and keeps the others, a line that is not JSON too,
/// and its turn is read back through it.
#[test]
fn a_kimi_stand_in_names_itself_in_the_index_and_keeps_the_other_lines() {
    let kimi = Harness::read("kimi").unwrap();
    let kept = scratch("kimi-index");
    let home = kimi.home(&kept, None, &kept);
    let other = r#"{"sessionId":"x","sessionDir":"sessions/x","workDir":"/other"}"#;
    let own = r#"{"sessionId":"y","sessionDir":"sessions/y","workDir":"/w/"}"#;
    std::fs::write(kept.join("session_index.jsonl"), format!("not json\n\n{own}\n{other}\n")).unwrap();

    kimi.write_session(&home, "/w", "brief", false, 1_800_000_000).unwrap();

    let index = std::fs::read_to_string(kept.join("session_index.jsonl")).unwrap();
    let lines: Vec<&str> = index.lines().collect();
    assert_eq!(lines[..2], ["not json", other], "{index}");
    assert_eq!(lines.len(), 3, "{index}");
    assert!(lines[2].contains(r#""workDir":"/w""#), "{index}");
    assert_eq!(
        kimi.turn(&home, "/w", "brief"),
        Some(Turn::Closed { since: 1_800_000_000, started: Some(1_800_000_000) })
    );
    let _ = std::fs::remove_dir_all(&kept);
}
