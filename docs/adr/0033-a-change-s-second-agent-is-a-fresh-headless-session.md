# 33. A change's second agent is a fresh headless session

Status: accepted

## Context

slimfit wants a change's mutants picked, and its growth judged, by an
agent other than the one that wrote it (`slimfit help picker`, `slimfit
help reviewer`). The worker's own session would judge its own work.

## Decision

- `worker pick-mutants` and `worker review-growth` each ask a fresh headless session, on the engine, model and account stablehand picks among the other engines that can be asked headless, else the worker's own, anew; within `pick_bound`, given only slimfit's goal, the diff from origin's proving and, for the picker, `slimfit check --mutants`.
- Claude Code, Codex, Grok and agy can be asked, each in a user directory of its own that holds only its login (agy's, its HOME, also an agent with no tools).
- One whose answer is no JSON object, or not one as asked, is asked once more in the same session (Claude Code and Grok by `--resume`, agy by `--conversation`; Codex's session is ephemeral, so it is not), told only what was wrong and the one form the answer takes; both asks' time and tokens count, and the record says it was "answered as asked when asked again".
- One that fails, or answers off the form twice, hands the question to the one stablehand picks among those left, the worker's own last, and the record names each that failed and why; when none is left the worker gets the last refusal, never a pick.
- When slimfit lists no candidates of a change its listing's `landing` says adds no source line and removes no test, no picker is asked: it answers `none`, "slimfit lists no candidates", and the issue keeps that line. Otherwise the gate still wants a record of none, so the picker is asked as ever and answers why none and the `ratchet --no-mutants` naming it.
- The reviewer answers `as_is` with why or `cut` with what, and the `slimfit ratchet` that writes the growth record naming it (`harness:session:model`) and the issue it serves.
- Each ask's time and tokens go on the issue as its own comment ([ADR 0029](0029-what-an-attempt-took-is-read-not-kept-twice.md)).

## Consequences

- A worker no second agent answers gets a refusal, never a pick of its own.
- Scenarios: `crates/factorium/tests/scenarios/pick.rs`, `crates/factorium/tests/scenarios/picked.rs`, `crates/factorium/tests/scenarios/review.rs`.
