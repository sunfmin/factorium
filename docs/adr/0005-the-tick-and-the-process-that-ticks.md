# 5. The tick and the process that ticks are two modules

Status: accepted (slimfit `deepen` of `crates/factorium/src/daemon.rs`)

## Context

`daemon.rs` held two concepts that change for different reasons (383
lines, 79 commits in 90 days):

- the tick: one pass over every step that moves without a command, in
  order, timed by its steps. It changes when a step is added or moved;
- the daemon process: one per database, holding a `flock` on its pid file
  for its life and through each exec of a newer binary (the descriptor
  handed on in `FACTORIUM_DAEMON_LOCK_FD`), the ticked file that says it
  is alive, its start, its loop between ticks, and `stop`. It changes
  when the lifecycle does (the lock, the exec hand-off, the service).

The tick used none of the process's details, but every reader of one
read past the other, and the lock, pid and ticked files sat in the
tick's module.

## Decision

- `daemon.rs` is the tick: `App::tick`, and `App::after`, which answers
  what binary to run next. Nothing in it knows of the pid file, its lock
  or the exec.
- `daemon/process.rs` is the process: `daemon_start`, `run`, `stop`,
  `daemon_down`, `daemon_pid`, `doctor_here` and `handed_lock`. The lock
  descriptor's variable, the exec, the ticked file and the stop poll are
  private to it. It drives the tick only through `tick` and `after`.

No test changes: the scenarios call the same `App` methods.

## Consequences

- A new tick step edits `daemon.rs` only.
- A change to the daemon's lifecycle (lock, exec, stop, liveness) edits
  `daemon/process.rs` only.
