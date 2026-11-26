# 3. The proving line is one pass for the tick, two moves for the owner; the cut has its own file

Status: accepted (factorium#93, slimfit `deepen` of `crates/factorium/src/line.rs`)

## Context

`line.rs` is the app side of the proving line (247 lines, 35 commits in 90
days): the domain's `Line` decides what is due, `line.rs` does the git,
gate and heavy-queue work it is told. Its interface was wider than its
callers needed, and two of its concepts leaked out:

- the daemon's tick drove the line move by move: every checkout's landing
  first, then each one's promotion and cut, then its local base following
  origin's. It imported `Move`, called `carry` per move and `follow_base`
  with origin's ref spelled out. The order of the moves (landings before
  promotions, the base after them) is the line's rule, held in `daemon.rs`;
- origin's base as last fetched, `refs/remotes/origin/<base>`, was spelled
  in four places (`line.rs`, `daemon.rs`, `doctor.rs`, `idle.rs`);
- the cut (its steps in one heavy turn, the tag, the formula's install,
  one record however it fails) is half the file and most of its churn
  (#35, #74, #97, #111). It shares nothing with landing and promotion but
  `carry`'s dispatch: no gate, no push requiring a tip, no lock.

## Decision

- `App::lines(checkouts)` is the tick's one call: each checkout's landing,
  then each one's promotion and cut as its line says each is due, then its
  local base following origin's. It answers what landed and what else
  moved. `carry` and `follow_base` are private to the line.
- `Checkout::fetched_base()` names origin's base as last fetched, once.
- The owner's interface is unchanged: `App::promote` and `App::cut` carry
  one move now, whatever the bound says; doctor asks `stuck_base`.
- `line/cut.rs` holds the cut: a cut's change touches that file alone.
  `line.rs` keeps the dispatch, landing and promotion.

No test changes: the scenarios drive the tick, `promote` and `cut` as
before, so each step is tested as it lands. The crate stays within its
budget (its target in `lines.sh`, ADR 0031).

## Consequences

- A new move, or a change in their order, edits `line.rs`, not the daemon.
- A cut's change (steps, tagging, the install) edits `line/cut.rs` only.
