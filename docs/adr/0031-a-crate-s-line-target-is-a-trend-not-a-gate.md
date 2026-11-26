# 31. A crate's line target is a trend, not a gate

Status: accepted

## Context

Each crate has a target for its source lines, so that one that only grows
is seen and deepened. A target that failed the gate would hold every
landing up on lines that a later landing could take away, and turn a
worker's change into one about the budget instead of its issue.

## Decision

- Each crate's target lives in `lines.sh`'s `targets`, one line per crate, and nowhere else.
- `./lines.sh`, which `checks.sh` runs in every gate, prints each crate's lines against its target, and an overage, and never fails.
- A crate over its target, or that grew at each of its last five landings that changed it, is a task: the idle tasks line runs `./lines.sh --tasks` after slimfit's `next`, and a `crate-lines` issue names the crate and the files that grew most (ADR 0014).
- Its worker deepens the crate; a target changes only with its reason in the issue that changes it.
- fmt and clippy stay bounds: they fail the gate.

## Consequences

- A landing over its crate's target is green; the crate's growth is
  filed once, by idle work, not argued at every landing.
- Scenarios: `crates/factorium/tests/scenarios/lines.rs`.
