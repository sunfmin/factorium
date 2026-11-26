# 34. Every checkout has a proving branch, and factorium alone moves its base

Status: accepted

## Context

A base branch is what every host and every release builds from. Two paths
to it, one through proving and one straight to the base for repos with
nothing slow to gate, would be two sets of rules; and a base that something
else moves is a base no gate has passed.

## Decision

- Every checkout has a proving branch: one path for every repo. A repo with
  no slow steps has one too; its promotion runs no slow steps.
- factorium is the one writer of a managed repo's base: the base is moved
  only by promotion, and anything else that moves the base turns promotion red.

## Consequences

- Landing, promotion and cut are the one path to a release ([ADR 0012](0012-the-line-gate-landing-promotion-cut.md)).
- Scenario: `a_base_moved_outside_factorium_turns_promotion_red_and_stays` in `crates/factorium/tests/scenarios/line.rs`.
