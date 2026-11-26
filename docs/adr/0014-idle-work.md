# 14. Idle work runs only on an idle host, and files one task of each kind

Status: accepted

## Context

Some checks are too slow for any gate (a long soak, a load test), and the
fits (slimfit, truefit, docfit) find work nobody filed. Run beside
workers, either would take the room their gates need. Filed all at once,
a fit's tasks would flood the tracker and conflict with each other.

## Decision

- A checkout's `[idle]` steps run in a work dir of their own ([ADR 0016](0016-the-line-s-jobs-run-in-work-dirs.md)) at the base's tip while the host is idle, at most once per `every` (from the last run's start) and within `limit` for all the steps.
  It takes a heavy slot after the lines' moves and the tick's start, so it never goes ahead of a promotion, a cut, a worker's start or a waiting heavy command; the first tick that finds the host no longer idle ends it, unrecorded.
- Its tasks line runs after the steps. The first task of each kind is
  filed as an issue, unless one of that kind is open, or one for the same
  task closed by a landing the tip read does not hold yet. So each kind has at most one open issue, no kind waits on another, and a verdict to leave a task is the tool's keep in the repo.
- A host may stay busy for days: once a checkout's last run started more than `every` plus `tasks_bound` ago, its tasks line runs without an idle host, alone, at background priority.
- By hand, `factorium idle <checkout>` runs it now at background priority: a tick that finds the host busy does not end it, and `unqueue` ends it whole.
  Its tasks are filed by the same rule, so it never files a second open issue of a kind.
  It is refused when the base declares no `[idle]`, and while idle work of any checkout runs or waits on the host, naming that entry and `factorium unqueue <id>`.
- `[idle]` lives in the repo, so every host with a checkout of it would
  file the same tasks: `checkout edit --no-idle` turns it off on this host
  alone.

## Consequences

- A worker that wants room waits at most a tick for idle work.
- A red run is a doctor finding until a later run is green; an answer of
  no tasks is red.
