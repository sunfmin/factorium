# 20. A decision made without the owner is a Decided record in its issue, numbered once

Status: accepted

## Context

Everything but spending money is decided without the owner, who reads the
decisions later, in one place, and must be able to name one and undo it.
A record copied out of its issue goes stale; a number given twice, or
moved, points the owner at the wrong decision.

## Decision

- Each decision is kept once, in the issue it belongs to (its body when
  filed, a comment after), in the shape `Decided (<who>, for Felix):
  <what>`, then `Options:`, `Chose:` and `Undo:` lines.
- A worker writes one when it makes a choice its issue did not settle or
  does other than its issue says; the watch, when it decides in a round;
  a second agent (picker, reviewer) writes none, its answer's why being
  its record.
- A comment is a record only when its first line starts `Decided (`; a line inside a fenced code block quotes the shape and is none.
  Nothing copies a record out of its issue.
- Doctor answers `decided`: every record made since the owner last ran
  `decided --read`; `decided` answers those of the last 7 days, read or not.
- Each record has a number, `D<n>`: one sequence for the host, on a
  stream of its own that gives each number once, taken when the record
  enters its issue. An edit keeps the number of each record whose head
  (who and what) stays; a number whose record an edit removed is not
  given again, and no other moves.
- Records made before numbers were given take theirs once, oldest first by when they were made, before any new one takes the next: an open issue's as its own event, a closed one's placed by the sequence's stream.
