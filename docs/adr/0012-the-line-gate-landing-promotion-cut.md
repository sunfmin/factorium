# 12. A change reaches the base by landing, promotion and cut, each gated

Status: accepted

## Context

Workers finish at any hour, many at once, and nobody reviews their merge.
The base branch is what every host and every release builds from, so
nothing may reach it that a gate has not passed on that very tree, and a
slow gate cannot run on every landing.

## Decision

- Each landing, promotion and cut runs in a work dir of its own ([ADR 0017](0017-the-line-s-jobs-run-in-work-dirs.md)).
- A gate is the repo's own fast and slow commands, each with its limit. A green run is recorded against the tree it ran on, so the same tree is never gated twice.
- Landing: a worker's `done` merges its branch onto the proving tip, runs
  the fast gate on the merge, and moves proving by a push that requires the
  tip it read; one per checkout at a time. A merge whose only conflicts are records the gate regenerates (`[gate] regenerated` in `factorium.toml`) is not red: it takes proving's copy of each.
  Any other conflict is red, naming every conflicting file.
- Each tick checks the merge of every branch waiting in line onto the
  proving tip (`git merge-tree`). One that does not merge is red on that tick, in the words of the red at its turn (the conflicting files, and what landed since), and leaves the line; one that merges keeps its place.
- Promotion: its gate on the proving tip (the slow steps alone on a green landing's commit, else the fast gate first), then the base
  fast-forwarded to it. It starts when the host is idle, or once the
  oldest unpromoted landing has waited the promotion bound less the hold
  bound, ahead of waiting heavy commands, so a heavy command running then does not push the promotion past its bound.
  The local base follows, on every tick to origin's base as last fetched, never further.
  One with commits of its own, or checked out over changes not committed, is never reset: it is a doctor finding.
- Cut: a version made from the base, when the checkout is idle or its
  oldest uncut landing is past the cut bound less the hold bound: set,
  push its own branch, gate, tag, publish, install, then the installed
  binary's proof. A promotion by hand is left to a cut by hand until the
  bound: the daemon cuts none in the gap. A failed cut holds every later cut of its checkout until one of three: its tag and install are read back as holding, a cut by hand, or a promotion moves the base to another commit.
- One cut of a checkout runs at a time, across processes: it holds the
  checkout's cut lock from before it reads its line until it is recorded;
  a cut finding it held does not start, and says who holds it. Two cuts
  at once in one clone tagged the tip without its version commit.

## Consequences

- While a move of a checkout runs beside the ticks, no other move of its
  kind is carried (the other kinds go on: [ADR 0018](0018-a-checkout-s-moves-run-apart.md)),
  and the daemon does not restart onto a new binary.
