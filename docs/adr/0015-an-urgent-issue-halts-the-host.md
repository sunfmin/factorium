# 15. An owner's urgent issue goes first, and cuts short heavy commands not urgent only when it has no room

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
- Once its worker starts, until the issue closes or is unmarked, a
  worker's or program's heavy command not urgent that runs is cut short
  once an urgent one waits with no room for what it holds (the halt).
  The halt passes no command over: one not urgent starts by the ordinary
  order and room whenever no urgent one waits, since holding them back
  kept 13 workers from an idle host for 30+ min while the urgent command
  used no CPU (2026-11-26 23:46 to 11-27 00:13). The owner, 2026-11-27:
  「被加急的票在 cpu 有容量的时候，也不档普通的命令」.
- The halt holds no worker's start: an issue not urgent starts by the
  ordinary order and admission, after the urgent ones. Holding starts kept
  every other checkout from work it could do without the CPU (reading,
  writing code, waiting its turn in the queue), while the queue already
  keeps the CPU for the urgent issue (2026-11-26: urgent issues held the
  host from 22:06 on, and eight issues ready since then had no worker at
  22:30). The owner: 「改成加急期间不挡新的 worker。」
- The halt holds no line move: other landings, promotions and cuts, on
  every checkout, go on through the heavy queue as usual and are never cut
  short by it. A line move delivers work already done and its gate is
  short (53–77s for a landing's); holding it made the urgent work no
  faster, since its own heavy commands already go ahead of a landing's
  waiting gate, while finished workers waited (2026-11-26: 15 done
  workers waited 20 min to land). The owner: 「都开吧，也加急」.
- The hold bound ends its heavy commands too, as any other (ADR 0010):
  one hung urgent gate held 13 waiting workers 34 min until killed by
  hand (2026-11-27 00:13). The owner: 「然后 30 也开票」.

## Consequences

- What shows an issue is urgent, and how it is marked, is `issue add
  --help`'s and `issue edit --help`'s.
- An urgent issue the owner forgets to unmark goes first, and cuts short
  heavy commands not urgent when the host has no room for its own, until
  it closes.
