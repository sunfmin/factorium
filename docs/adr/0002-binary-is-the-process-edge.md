# 2. The binary is the process edge; the command line and the guard each have one home

Status: accepted (factorium#92, slimfit `deepen` of `crates/factorium/src/main.rs`)

## Context

`main.rs` was the binary's whole source (439 lines, 26 commits in 90 days),
holding three concepts in one file:

- the command line: the clap types, which are the manual (SPEC §4), and the
  one `App` call each command makes, with the readings of its arguments
  (`--body-file`, the words of `worker exclusive`);
- the guard hook: which harness asks, its tool call read from stdin, the
  line refused by `Caller::may_run_line`, and the exit code that harness
  refuses with. Half of this lived in `main.rs`, half in
  `session/launch.rs`; factorium#99 (refuse what the guard cannot read)
  had to change both;
- the process: the runtime, the caller's permission, the daemon's signal
  handlers, printing the answer, the exit code.

Most of its commits add or change a command or a flag: they edit the
manual and the dispatch, which is right (a dispatcher is not shallow; the
manual and its dispatch are one concept). The others (#99 the guard, #100
the daemon's signals, #110 exclusive's exit code) changed the process or
the guard, and had to be threaded through the dispatch: `Top::Guard` was an
`unreachable!` arm, `Top::Run` built signal handlers inline.

## Decision

- The guard is the library's: `Caller::guard(harness, payload)` beside
  `may_run_line` answers whether a session's tool call may run, and if not
  why and with which exit code. What the guard refuses, and how, lands in
  `session/launch.rs` alone; the binary reads stdin and exits as told.
- `cli.rs` is the command line: the clap types and `Top::answer`, which
  turns each command into its one `App` call. A new command or flag edits
  this file and the `App` method behind it.
- `main.rs` is the process edge: the guard before any database, the
  caller's permission, the runtime, the daemon's signals, printing and the
  exit code. It changes only when how the process runs changes.

`App`'s interface is unchanged: the scenarios and the CLI keep calling the
same methods, and `tests/scenarios/binary.rs` tests the binary end to end,
so every step is tested as it lands. The crate stays within its budget
(SPEC §5): no step adds more than a module's header.

## Consequences

- A command change touches `cli.rs` and the concept's module, not the
  process code; a guard change touches `session/launch.rs` only.
- `main.rs` is small and quiet; if it churns again, something about the
  process changed.
