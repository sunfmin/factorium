# 16. A checkout runs at most `workers_per_checkout` workers

Status: accepted (the owner asked for it, 2026-11-26: 「都开吧，也加急」)

## Context

Admission bounds the host's load, not one checkout's share of it
([ADR 0010](0010-admission-and-the-heavy-queue.md)). On 2026-11-26 the
host ran 23 live workers, 19 of them factorium's (15 in landing). Their
branches conflicted: each landing moved proving and the ones behind it
went red on the merge. They also waited in the heavy queue far longer than
their commands ran. More workers on one checkout did not land more.

## Decision

- The host setting `workers_per_checkout` (a count): a checkout with that
  many live workers, those in landing among them, starts no other. The
  next ready issue of another checkout starts in its place.
- It holds no urgent issue ([ADR 0015](0015-an-urgent-issue-halts-the-host.md)),
  as the load does not.
- It holds no promotion's mutants issue
  ([ADR 0006](0006-mutants-are-owed-at-promotion.md)), which the promotion
  waits for only `mutants_bound` (the owner, 2026-11-26: 「开吧」).
  Admission's load still holds it; its worker counts as live.
- It is no part of admission: admission still reads the host alone, and
  the cap decides which ready issue may start, beside the order of ADR 0020.
- Its default is 6, from the measurement below.

## Measurement

From this host's event log, 2026-11-17 to 2026-11-26, read-only: for
factorium, each hour's average of live workers (from a worker's start to
its landing or end) beside that hour's landings and red landings (a red
gate or a branch that no longer merges). 215 hours, 393 workers, 349
landings. Per landed worker, the median minutes from start to landing
and from asking to land to landing, by the average live workers over its
life.

| live workers | hours | landed/h | red/h | red per landing | start→landed (min) | asked→landed (min) |
|---|---|---|---|---|---|---|
| under 2 | 88 | 1.6 | 0.4 | 0.26 | 26 | 0 |
| 2–4 | 33 | 2.7 | 1.0 | 0.38 | 20 | 0 |
| 4–6 | 11 | 2.9 | 1.5 | 0.50 | 23 | 1 |
| 6–8 | 6 | 7.8 | 3.8 | 0.49 | 73 | 10 |
| 8–11 | 2 | 3.0 | 0.0 | 0.00 | 41 | 13 |
| 11–15 | 2 | 5.5 | 5.5 | 1.00 | 31 | 14 |
| 15 and over | 5 | 3.4 | 1.6 | 0.47 | 228 | 55 |

Landings per hour are highest at 6–8 live workers. Past 8 they fall,
while a worker's time to land and its wait in the line grow, and at
11–15 every landing had a red one beside it. Hours past 6 are few (15), so
the default sits at the start of the best band: 6.

## Consequences

- Workers past the cap wait as ready issues, not as sessions: no
  worktree, no heavy entry, no branch going stale behind proving.
- doctor's `capped` names each checkout at its cap and the ready issues
  waiting for it, never an urgent one or one that starts ahead.
- The number is the host's: `factorium settings workers_per_checkout <n>`.
  Measuring it again after the cap ran shows whether 6 holds.
