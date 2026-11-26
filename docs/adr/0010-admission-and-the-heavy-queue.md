# 10. Admission and the heavy queue read the host, not a number

Status: accepted

## Context

How many workers a host can carry depends on what they run: one building
a large crate takes the machine, ten reading code take little. A
concurrency number is wrong both ways. The load that hurts comes from
heavy commands (builds, test suites, gates), and a time a gate compares
(slimfit's test time) is only true if every gate is measured alike.

## Decision

- Admission is whether one more worker may start now, from the host's readings alone: CPU busy share (less what runs in started heavy slots),
  free memory, the disk floor, factorium's own share caps, the accounts'
  windows, and the heavy queue keeping up (no heavy command waiting its
  turn past `queue_wait_bound`). No concurrency number.
- The resource `host` is the heavy queue: every gate run, a worker's
  `worker exclusive`, a program's `heavy run`. It starts each when the
  host has room, by the same readings as admission; it never starves the oldest (it starts at once when nothing runs), starts one beside what runs only on a reading taken `heavy_settle` after the last start, and ends one that holds others up past the hold bound.
- A run is ended whole, by one path whoever ends it: each step in a
  session of its own, TERM then KILL; an unqueued run's waiter answers `unqueued`.
- Gates first: a gate and a worker's `worker exclusive` run at
  factorium's own priority; heavy work whose time no gate compares runs
  at background priority, its whole process tree: a nice of 10, not Darwin's background band (efficiency cores and throttled disk even when idle), so the CPU goes to a gate before it when they compete, and the run has every core and full disk when the host has room.
- A line move (promotion, cut) past its bound goes ahead of workers' and programs' waiting entries, taking the next free slot; a running entry is never cut short for it. Behind it, an urgent issue's ([ADR 0015](0015-an-urgent-issue-halts-the-host.md)). Then a landing's fast gate (short, one per checkout), oldest first.
- The rules are the running daemon's: an entry waiting under replaced rules is passed over until its asker asks again under the daemon's, so it holds none up.
  An entry that leaves unstarted keeps its place for `heavy_keep`: the same worker (or program) asking the same command again within it takes that place, not the back.

## Consequences

- The queue holds the busy share near the limit, and the load it holds
  back shows only in its wait, which admission reads.
- A daemon restart or a client that asks again loses no turn.
