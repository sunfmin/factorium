# 35. No Flux: scenarios and mutation hold the rules

Status: accepted

## Context

afkconvoy checked its pure crates with Flux refinement types. Its fast
gate, which runs on every landing, spent 65 of its 138 seconds in Flux
(measured on afkconvoy's gate when factorium was specified, 2026-11-17).
factorium's rules are already held by its scenarios, run on the real
binary, and by each change's mutants ([ADR 0032](0032-tests-only-at-the-outermost-layer.md)).

## Decision

- factorium's crates carry no Flux specs, and its gate runs no Flux.
- A rule is held by a scenario in `crates/factorium/tests/scenarios/`
  and by the mutants of the change that adds it ([ADR 0006](0006-mutants-are-owed-at-promotion.md)).

## Consequences

- The fast gate spends nothing on refinement checking.
- A rule no scenario exercises is held by nothing; slimfit's coverage and
  mutants find it.
