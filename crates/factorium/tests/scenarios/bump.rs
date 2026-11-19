//! A checkout that pins another checkout by git rev (factorium#127): once
//! that one's base moves past the pin, the tick files one issue in the
//! dependent to bump it, and no second while it is open.

use std::path::Path;

use crate::fixture::World;

/// Commits `Cargo.toml` pinning `url` at `rev` (and, sorted first, a repo
/// no checkout is) in `app`, and pushes it to app's proving branch.
fn pin(w: &World, app: &Path, url: &str, rev: &str) {
    let toml = format!(
        "[workspace.dependencies]\naaa = {{ git = \"https://example.com/aaa\", rev = \"0123abc\" }}\n\
         lib = {{ git = \"{url}\", rev = \"{rev}\" }}\nlib-core = {{ git = \"{url}\", rev = \"{rev}\" }}\n"
    );
    w.commit(app, "Cargo.toml", &toml);
    w.git(app, &["push", "-q", "origin", "HEAD:refs/heads/proving"]).unwrap();
    w.git(app, &["fetch", "-q", "origin", "proving"]).unwrap();
}

/// Commits `file` on lib's base and pushes it, as a promotion would; answers the new tip.
fn moves(w: &World, lib: &Path, file: &str) -> String {
    w.commit(lib, file, file);
    w.git(lib, &["push", "-q", "origin", "HEAD:refs/heads/main"]).unwrap();
    w.git(lib, &["fetch", "-q", "origin", "main"]).unwrap();
    w.remote_tip("lib", "main")
}

#[tokio::test]
async fn a_pin_behind_its_checkouts_base_gets_one_bump_issue() {
    let w = World::new();
    let lib = w.checkout("lib").await;
    let app = w.checkout("app").await;
    // Cargo's URL of lib's remote, written otherwise than git's: a scheme, a user, no `.git`, a `/`.
    let remote = w.git(&lib, &["remote", "get-url", "origin"]).unwrap();
    let url = format!("ssh://me@{}/", remote.strip_suffix(".git").unwrap());
    // lib pins only a repo no checkout is: app, another checkout, is not it.
    w.commit(&lib, "Cargo.toml", "[dependencies]\nx = { git = \"https://example.com/x\", rev = \"0123abc\" }\n");
    for branch in ["main", "proving"] {
        w.git(&lib, &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")]).unwrap();
        w.git(&lib, &["fetch", "-q", "origin", branch]).unwrap();
    }
    let first = w.remote_tip("lib", "main");

    pin(&w, &app, &url, &first);
    let t = w.tick().await;
    assert!(t.bumped.is_empty(), "pinned at lib's tip: {:?}", t.bumped);

    // Pinned at a commit off lib's base, which moves: not behind it.
    w.git(&lib, &["checkout", "-q", "-b", "side"]).unwrap();
    w.commit(&lib, "side", "x");
    let side = w.git(&lib, &["rev-parse", "HEAD"]).unwrap();
    w.git(&lib, &["checkout", "-q", "main"]).unwrap();
    let second = moves(&w, &lib, "a");
    pin(&w, &app, &url, &side);
    assert!(w.tick().await.bumped.is_empty(), "diverged from lib's base");

    pin(&w, &app, &url, &first);
    let t = w.tick().await;
    assert_eq!(t.bumped.len(), 1, "{:?}", t.bumped);
    let issue = &t.bumped[0];
    assert_eq!(issue["checkout"], "app");
    assert_eq!(issue["title"], format!("Bump lib to its base tip ({second})"));
    let body = issue["body"].as_str().unwrap();
    assert!(body.contains("write a") && body.contains("1 landing(s)") && body.contains(&first), "{body}");

    moves(&w, &lib, "b");
    assert!(w.tick().await.bumped.is_empty(), "one is open");
    let open = w.app.issue_list(Some("app"), false).await.unwrap();
    assert_eq!(open.as_array().unwrap().len(), 1, "{open}");
}
