//! A checkout whose `slimfit.toml` holds the record of its mutants at its
//! base branch (`[check] mutants_target`, `slimfit help mutants-scope`): a
//! landing waits for its fast gate alone, and before a promotion's slow gate
//! the range landed since the base owes one record, picked, run and answered
//! by one worker on an issue filed for it. What is read here
//! is `slimfit.toml` and slimfit's listing of the range; what is decided is
//! whether the promotion goes, waits for that issue, or files it, and the
//! issue's words.

use serde_json::Value;

use crate::idle::Task;
use crate::neutral::landing::Awaited;

/// The task kind of a promotion's mutants issue: it starts ahead of the
/// ordinary ones ([`ahead`]), and one is open per checkout at a time.
pub const KIND: &str = "promotion-mutants";

/// Whether an issue filed for `task` starts ahead of the ordinary ones,
/// behind the urgent: a promotion's mutants issue, which the base waits for.
pub fn ahead(task: Option<&Task>) -> bool {
    task.is_some_and(|t| t.kind == KIND)
}

/// The branch `slimfit.toml` holds the record of mutants at
/// (`[check] mutants_target`), if it holds it at one.
pub fn held_at(slimfit_toml: &str) -> Option<String> {
    let v: toml::Value = slimfit_toml.parse().ok()?;
    let target = v.get("check")?.get("mutants_target")?.as_str()?.trim();
    (!target.is_empty()).then(|| target.to_string())
}

/// Whether the range slimfit's `listing` (`check --mutants --target`) is of
/// owes a record of its mutants: it adds source lines or removes tests, and
/// holds no record yet. A record names a picker other than its author, or
/// `slimfit ratchet` would not have written it.
pub fn owes(listing: &Value) -> bool {
    let landing = &listing["landing"];
    let records = landing["records"].as_array().is_some_and(|r| !r.is_empty());
    crate::pick::due(landing) && !records
}

/// Until when (ms) a promotion waits at `now` for its open mutants issue
/// `filed`: within `bound` of its filing, while a worker holds it or can
/// start it; `None`, it goes on without it, the issue staying open.
pub fn waits(filed: Awaited, startable: bool, now: i64, bound: i64) -> Option<i64> {
    let until = filed.at + bound;
    (startable && now < until).then_some(until)
}

/// The mutants issue of the promotion of `proving` at `tip`, owing the
/// record of the range `from..tip` against `target`: its title, body and
/// task.
pub fn issue(proving: &str, target: &str, from: &str, tip: &str) -> (String, String, Task) {
    let short = |sha: &str| sha.get(..10).unwrap_or(sha).to_string();
    let title = format!("Mutants of {}..{}", short(from), short(tip));
    let body = format!(
        "The promotion of `{proving}` at `{tip}` owes the record of the mutants of everything landed since its \
         base: the range `{from}..{tip}` (`git log {from}..{tip}`). This checkout holds that record at its base \
         (`slimfit.toml`: `[check] mutants_target = \"{target}\"`; `slimfit help mutants-scope`, \"Held at the base \
         branch\"), so no landing in it carried one. The promotion waits for this issue's landing at most \
         `mutants_bound` from its filing; past it, it goes on, and this issue stays open, for the same range.\n\n\
         Your worktree holds the range; what landed after it is in your listing too. Every command takes the range \
         by its base, `--target {from}`:\n\n\
         1. `factorium worker pick-mutants --target {from}`: a second agent picks over the range's diff and \
         candidates, and it answers the commands that run and record the picks.\n\
         2. Run the picks as it answers, through `factorium worker exclusive --background`.\n\
         3. For each survivor, add a fast test that kills it, or loosen it with its reason (`slimfit ratchet \
         --loosen survived_mutants --reason \"<why>\" --target {from}`); then the `slimfit ratchet --target {from}` \
         it answers.\n\
         4. Commit the tests and `.slimfit/`, and land them with `factorium worker done`, as any change (its fast \
         gate).\n"
    );
    (title, body, Task { kind: KIND.into(), files: Vec::new() })
}
