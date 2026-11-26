# 36. A landing waits for no review

Status: accepted

## Context

Workers finish at any hour, and a review before each landing would make
every landing wait for a reviewer. Verdicts and rework are afkconvoy's,
dropped from factorium (`docs/dropped.md`).

## Decision

- It lands on the fast gate alone and reaches only proving, which is
  promoted after the slow gate ([ADR 0012](0012-the-line-gate-landing-promotion-cut.md)).
- Review comes after landing, from outside: Guild's warden, and control's
  job on a proving branch, file what they find as new issues.

## Consequences

- What would reopen it: a landing that harms before review can catch it
  (secrets, data loss, something externally visible), and then a finding
  that holds promotion (`factorium hold`), not an approval on every
  landing; or measured evidence that findings after landing cost more than
  review before it.
