# 19. A checkout's landings are stacked, their gates run at once

Status: accepted. Replaces the rule of [ADR 0012](0012-the-line-gate-landing-promotion-cut.md)
that a checkout lands one worker at a time.

## Context

The owner, 2026-11-26: "我觉得落地到 proving 上面的不应该限制只能有一个。大家都跑只要 CPU
挺得住。然后谁绿了谁先 merge。" (landing on proving should not be one at a time:
all run while the CPU holds, and whoever is green merges first), then, told
that a green branch gated on an old tip would push a combination no gate
ran, he chose stacked merges; and of a cap on how many: "这个也不设上限了吧"
(no cap on that either).

Measured that day on factorium: the fast gate took 59–68s, yet landings came
2.5–6 min apart, eight workers waiting to land and the CPU 69–70% idle.
Gating each branch on the same tip and pushing whichever is green first
would push merges no gate ran: two branches each green alone, one changing
a function's arguments the other still called, did not compile merged.

## Decision

- Every worker that asks to land is stacked: its branch merged onto the
  merge below it, the first onto the proving tip (T+A, T+A+B, T+A+B+C).
  The urgent ones go first among those stacked anew; one already stacked
  keeps its place.
- Each stacked merge's fast gate runs in a work dir of its own (with its
  own build cache slot, ADR 0017), as a heavy entry of the queue, beside
  the ticks. The queue's room alone decides how many run: no count, no
  setting.
- Proving moves, in one push requiring the tip the stack's bottom merged
  onto, to the deepest merge whose gate and every gate below it are green.
  Each worker in it lands, recorded as before. A green merge whose lower
  one still runs waits for it.
- A red gate's worker is told, as before; the merges above it leave the
  stack, their gates ended, and are stacked again without it on the next
  tick. So does a merge whose worker no longer asks or whose branch moved
  (it lands again later), and the whole stack when proving moved under it.
- A branch that does not merge onto the top of the stack waits while it
  merges onto the proving tip; one that does not merge onto the tip is red,
  as before.
- A merge's conflicts only in the records the gate regenerates take the copy
  below; its gate rewrites them, with those of the merges below, and they
  are amended into its merge. Merges stacked on one that was amended and
  pushed are stacked again on what was pushed.
- The stack is kept in the store (`stack` per checkout), so doctor shows it;
  the daemon lets it go as it starts, since its gates ran in the process it
  replaces.

## Consequences

- Proving holds only commits a fast gate ran on, as before; landings come as
  fast as the host has room for their gates.
- A red low in the stack costs the gates above it a rerun.
- Each slot keeps a build cache as large as a landing's (22 GB for
  factorium that day): the disk holds one per landing gated at once.
