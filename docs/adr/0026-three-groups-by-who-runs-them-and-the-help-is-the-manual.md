# 26. The commands are three groups by who runs them, and the help is the manual

Status: accepted

## Context

One binary serves the owner, the workers and the watch, each of which is
an agent session or a person reading it as one. A worker that could
close, promote or stop would act past its own issue. An agent knows a tool
by its `--help`, its stdout and its exit code; a manual written apart from
the code drifts from it.

## Decision

- One binary, `factorium`, its commands in three groups by who runs them:
  the owner's (every top-level command but `worker`, `watch` and the hidden
  `guard`), `factorium worker …` and `factorium watch …`.
- The guard, the hook a session runs before each tool call
  (`Caller::guard`, ADR 0002), keeps each group to who runs it: the guard lets a worker run only `factorium worker …` and lets only the watch run `factorium watch …`.
- The watch runs every owner command too but `stop`: it lives under the
  daemon, so the guard refuses `stop` to it, as to workers. It leaves an
  issue to a person with `issue wait-for-human` and releases a hold with
  `hold --release`.
- The help in the binary is the manual, and there is no other: every command's `--help` says what it does and what it answers. Answers are JSON.
- It is written with agent-help, as stablehand's and slimfit's is: each
  command's and argument's doc comment in `crates/factorium/src/cli.rs`, Markdown with each
  paragraph on one line, kept as written (`verbatim_doc_comment`), each
  top-level command in the group of who runs it.

## Consequences

- A command's change edits its doc comment beside its clap type; nothing
  else describes it.
- `crates/factorium/tests/scenarios/manual.rs` runs `agent_help::check` on the manual.
