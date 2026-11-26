# 15. An owner's urgent issue goes first and halts every other start

Status: accepted (the owner asked for it, 2026-11)

## Context

The owner sometimes needs one issue done now: a fix the host itself
waits on. In the ordinary order it waits behind admission's load reading,
the heavy queue and its checkout's line, each fair to every issue and
none knowing this one matters more.

## Decision

- Urgent is the owner's mark on an issue, never a worker's: `worker file`
  cannot set it.
- Blocks still hold an urgent issue. Among urgent ones the ordinary order
  holds.
- Its start goes before every other ready issue, held only by the memory
  and disk floors, never by the load.
- Its worker's heavy commands go ahead of those not urgent, behind a line
  move past its bound, as its issue is marked at each ask.
- Its landing takes the next turn of its checkout's line, never cutting a
  running gate short.
- Once its worker starts, until the issue closes or is unmarked, nothing
  else starts, and what runs is cut short once it waits for it (the halt).

## Consequences

- What shows an issue is urgent, and how it is marked, is `issue add
  --help`'s and `issue edit --help`'s.
- An urgent issue the owner forgets to unmark halts the host until it
  closes.
