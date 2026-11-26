//! What Guild depends on (`factorium_domain::neutral`): the domain's neutral parts — admission, the
//! queue's start rule, the hot rule, landing onto a proving branch — name
//! no issue, worker, watch or checkout, and depend on nothing else of
//! factorium. This reads their source from outside and fails on each leak.
//!
//! `FACTORIUM_NEUTRAL` (a list of files) reads other files instead: how the
//! leaks of the parts before they were split were shown.

use std::path::{Path, PathBuf};

/// The neutral parts: each must be there, so the check cannot pass empty.
const PARTS: [&str; 5] = ["mod.rs", "admission.rs", "queue.rs", "hot.rs", "landing.rs"];

/// What they may name: only factorium knows these.
const WORDS: [&str; 4] = ["issue", "worker", "watch", "checkout"];

/// What they may depend on: the language, serde, ids and factum's core.
const ROOTS: [&str; 7] = ["std", "core", "serde", "uuid", "factum_core", "super", "self"];

/// The macros they may call: the language's own.
const MACROS: [&str; 9] =
    ["format", "vec", "matches", "assert", "debug_assert", "write", "unreachable", "panic", "derive"];

fn files() -> Vec<PathBuf> {
    if let Ok(list) = std::env::var("FACTORIUM_NEUTRAL") {
        return std::env::split_paths(&list).collect();
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../domain/src/neutral");
    PARTS.iter().map(|p| dir.join(p)).collect()
}

/// The leaks of `text`, the file at `name`.
fn leaks(name: &str, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let at = format!("{name}:{}", n + 1);
        let lower = line.to_lowercase();
        for w in WORDS {
            if lower.contains(w) {
                found.push(format!("{at}: names `{w}`: {}", line.trim()));
            }
        }
        let code = line.split("//").next().unwrap_or("");
        let mut rest = code;
        while let Some(i) = rest.find("crate::") {
            let after = &rest[i + "crate::".len()..];
            if !after.starts_with("neutral") {
                found.push(format!("{at}: depends on factorium outside the neutral parts: {}", line.trim()));
            }
            rest = after;
        }
        if code.contains("super::super") {
            found.push(format!("{at}: reaches out of the neutral parts: {}", line.trim()));
        }
        if let Some(path) =
            code.trim_start().strip_prefix("pub use ").or_else(|| code.trim_start().strip_prefix("use "))
        {
            let root: String = path.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !ROOTS.contains(&root.as_str()) && root != "crate" {
                found.push(format!("{at}: depends on `{root}`: {}", line.trim()));
            }
        }
        for (i, _) in code.match_indices('!') {
            let name: String = code[..i].chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let name: String = name.chars().rev().collect();
            let call = code[i + 1..].starts_with(['(', '[', '{']);
            if call && !name.is_empty() && !MACROS.contains(&name.as_str()) {
                found.push(format!("{at}: calls the macro `{name}!`: {}", line.trim()));
            }
        }
    }
    found
}

#[test]
fn the_neutral_parts_name_nothing_of_factorium() {
    let mut found = Vec::new();
    for f in files() {
        let name = f.display().to_string();
        match std::fs::read_to_string(&f) {
            Ok(text) => found.extend(leaks(&name, &text)),
            Err(e) => found.push(format!("{name}: missing: {e}")),
        }
    }
    assert!(found.is_empty(), "the neutral parts leak ({}):\n{}", found.len(), found.join("\n"));
}

#[test]
fn each_kind_of_leak_is_found() {
    let leaky = "use crate::worker::Status;\n\
                 pub fn f(c: &Checkout) {}\n\
                 crate::no::<()>(\"x\", \"y\");\n\
                 use factorium_host::Git;\n\
                 command!(X, \"x\");\n\
                 use super::super::line;\n";
    let found = leaks("leaky.rs", leaky);
    for (line, what) in [(1, "names `worker`"), (1, "outside"), (2, "names `checkout`"), (3, "outside")] {
        assert!(found.iter().any(|l| l.starts_with(&format!("leaky.rs:{line}:")) && l.contains(what)), "{found:?}");
    }
    for (line, what) in [(4, "depends on `factorium_host`"), (5, "macro `command!`"), (6, "reaches out")] {
        assert!(found.iter().any(|l| l.starts_with(&format!("leaky.rs:{line}:")) && l.contains(what)), "{found:?}");
    }
    let clean = "use std::fmt;\nuse crate::neutral::queue::Queued;\npub fn f() -> String { format!(\"x\") }\n";
    assert_eq!(leaks("clean.rs", clean), Vec::<String>::new());
}
