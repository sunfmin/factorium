# 18. A checkout's landing, promotion and cut run apart

Status: accepted. Replaces the rule of [ADR 0012](0012-the-line-gate-landing-promotion-cut.md)'s
Consequences that while a move of a checkout runs beside the ticks, none of
its other moves is carried.

## Context

The owner, 2026-11-26: "landing promotion 和 cut 为什么要用一个队伍呢？CPU
够用的话，不是应该不阻碍任何队伍？" (why do landing, promotion and cut share one
line? With CPU to spare, should none hold another?)

A checkout carried one move at a time, so a promotion's slow gate and a
cut's steps held its landings. Measured that day on factorium: ten landings
one every 3.2 min, then 17 min with none (a promotion, then two cuts) while
six workers asked to land; most of a cut is the wait for the tap's remote
build, the host's CPU idle.

The three moves write different refs: a landing pushes proving, a
promotion fast-forwards the base to a tip it read, a cut tags and publishes
the base commit it read. Each runs in a work dir of its own (ADR 0017), and
each push requires the tip it read.

## Decision

- A checkout's landing, promotion and cut run independently, each kind one
  at a time: no landing is carried while one of its landings runs beside
  the ticks, no promotion while a promotion runs, no cut while a cut runs
  or its installed binary is proven.
- Each goes through the heavy queue as before: the queue's room alone
  decides whether their heavy steps run together.
- A promotion's gate is on the tip it read, and the base fast-forwards only
  to that tip, however far proving moved meanwhile. A cut cuts the base
  commit it read.
- When each is due (the promotion bound's busy rule, the cut bound's
  unsettled rule) is unchanged.

## Consequences

- Landings go on while a promotion's slow gate or a cut's remote wait runs.
- The daemon still restarts onto a new binary only once no move of any
  checkout runs beside the ticks.
