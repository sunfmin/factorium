# 28. An issue's comments are its one record, and reach its worker at once

Status: accepted

## Context

What the owner, a worker, the watch and factorium say about an issue was
scattered: escalations, tells and restarts each in their own place, the
next worker briefed with none of it. Word for a live worker is no use an
hour later; stopping its turn to give it loses the work of that turn.

## Decision

- An issue's comments are the one record of what is said about it, each
  with who wrote it (`owner`, `worker <id>`, `watch`, `factorium`) and when.
  A worker's `escalate`, and the text of `watch tell` and `watch restart`,
  become comments when they are typed, delivered or not.
- `issue comment` records the comment and reaches the live worker on the issue at once, through its outbox (ADR 0027), never stopping its running turn.
- Each reaches the worker at most once: the line `tell` or `restart` types is its delivery, nothing types it again, and a worker's own escalation is typed back at no one. Comments factorium writes itself (start-over's) are typed at no one.
- The next brief carries every comment.

## Consequences

- Comments made before authors and times were kept read as the owner's
  (start-over's as factorium's), with no time.
