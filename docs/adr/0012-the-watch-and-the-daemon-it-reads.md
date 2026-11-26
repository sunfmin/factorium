# 12. One watch deals with every finding, and doctor says first whether a daemon runs

Status: accepted

## Context

Nobody watches the host: a finding nobody looks at stays until the owner
comes back. An agent looking at every finding all the time would spend
its model on what fixes itself, and a session briefed once keeps rules
the next cut may change. Everything else factorium does rests on its
daemon, so a host with none must say so to whoever touches it next.

## Decision

- One resident agent session, the watch, deals with every finding:
  escalations, failed, hot and silent workers, red promotions, red idle
  work, failed cuts, resource holds. A look (no model) every five
  minutes reads doctor's findings. Each condition is an **episode**, and an episode starts at most one **round** at a time: a brief, an hour (`round_bound`), and a report the watch files.
- No finding stays standing after its round with nobody looking at it: an episode still found `round_again` after its round ended starts another, whose brief carries the last report, up to `rounds_max`; past them doctor lists it under `unresolved` for the owner.
- `watch tell` to a worker that waits for the watch is `watch restart`: it ends the wait, so a word that sends the worker back to work never leaves it waiting, its `done` refused.
- The watch works by the rules of the factorium that runs: a look that
  finds its session briefed with other rules than the running binary's closes that session between rounds, never in one, and starts a fresh one.
- doctor's operator view first answers whether a daemon runs: its pid file names a live process that holds its lock, and that one ticked (`daemon.tick`) within five `tick_every`, never under two minutes.
  When none does, that is the first finding, and every other command but `run`, `stop` and `service` says so in one line on stderr.

## Consequences

- A worker the watch parks behind other issues (`issue block`) is no
  finding while any of them is unmet, and goes back to work by itself
  once all are met.
- The watch's look leaves the daemon finding out: the daemon runs the
  look.
