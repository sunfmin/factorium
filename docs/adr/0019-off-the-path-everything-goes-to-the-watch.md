# 19. Off the path, everything goes to the watch, and a stopped line is a finding

Status: accepted

## Context

Nobody watches the host. A worker that dies, hangs or goes quiet, or a
line that stops moving, tells nobody unless a check makes it a finding;
one found while it only waits for what it was told to wait for spends
the watch on nothing.

## Decision

- Off the path, everything goes to the watch: `worker escalate`, a worker whose session died, never came up (five minutes after it started, a fixed bound), went silent past its bound (not while its issue waits for anything unmet, nor within a wait it declared on a long command of its own, held to two hours), or hung past it (its turn open, its processes next to idle, its screen unchanged), a hot worker, a red promotion, red idle work, a failed cut, admission held for one reason past the host's span.
- A finding past a declared wait (`worker waiting`) names what the worker
  said.
- So does a line that stopped: whatever it owes is done within its bound or is a finding — a heavy queue that starts nothing past `stall_bound`, a landing no gate runs for on a line that has not moved for `stall_bound`, the oldest unpromoted landing past `promotion_bound` with no promotion running, the oldest uncut promotion past `cut_bound` with no cut running.
- A line has moved when a landing, promotion, cut or line job started on
  its checkout in the last `stall_bound`, or its own ask did: one that waits
  its turn behind landings that move is no stall.
- A worker whose engine is at its limit, its screen giving the reset, is
  parked until then, no finding: what is passed to it is held, and its
  account starts no worker before the reset. A used-up host login rests
  its harness the hour, a fixed bound, not a setting.
- The watch fixes, restarts, re-briefs, leaves an issue to a person or
  closes, and reports. Only spending money is left to the owner; doctor's `asks` answers every issue of every checkout that waits for a person.
