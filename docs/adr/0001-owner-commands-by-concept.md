# 1. The owner's commands live with their concept, the model with the engine

Status: accepted (factorium#91, slimfit `deepen` of `crates/factorium/src/owner.rs`)

## Context

`owner.rs` was the binary's biggest, busiest file (459 lines, 35 commits in
90 days). Its history shows why: most of those commits only added a line to
`model()`, the list of every aggregate and command the engine runs. That
list is the engine's wiring, not an owner command, yet each new domain
command touched `owner.rs`. The rest of the file held four concepts behind
one "owner" heading:

- the tracker: `issue add/list/show/edit/close/wait-for-human/start-over/
  ready/comment/block`, plus `issue`, `note` and `blocker`, which the daemon,
  the line, the watch and the worker's life also call;
- doctor's reading (`seen`), which both `doctor` and the watch's look read,
  and `doctor` itself;
- the host's configuration: checkouts, the kit, accounts, settings, holds,
  the heavy queue's `unqueue`.

`close` and `wait-for-human` each read the issue's worker, submitted their
command, then ended that worker with their own words: one step written twice.

## Decision

- `model()` moves to `lib.rs`, beside `App::open`, its one caller. A new
  domain command adds a line next to the engine it registers with.
  It does not move into `factorium-domain`: `Model` belongs to factum's
  engine crate, and the domain stays on `factum-core` alone (no engine, no
  IO). One list in the binary is the price of that.
- `doctor.rs` holds what doctor reads (`seen`) and `doctor`: the one
  place that knows how doctor's inputs are gathered, for the owner and the
  watch alike.
- `issues.rs` is the tracker: every `issue …` command and the issue helpers
  other modules call (`issue`, `note`). An issue that leaves its worker
  (`close`, `wait-for-human`) does so through one private step that ends the
  worker with the reason and says whether its worktree is kept.
- `owner.rs` keeps the host's configuration: checkouts, kit, accounts,
  settings, holds, unqueue.

Each `App` method keeps its signature: the CLI and the scenarios call the
same interface, so the scenarios are the test of every step.

## Consequences

- A domain command change touches the domain and one line in `lib.rs`.
- A change to the tracker, doctor or configuration lands in the file named
  for it; none of them churns the others.
