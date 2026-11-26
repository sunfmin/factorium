# 17. A worktree that holds work nowhere else is kept, and resumed

Status: accepted

## Context

A worker's worktree may hold commits not on proving or changes never
committed: an attempt that went red, died or was stopped. Removing it
would lose that work; starting the next attempt elsewhere would redo it.
The tick and several commands act on one issue's worktree, and one that
acts on what it read while another changed it removes the wrong thing.

## Decision

- One test decides it all: a worktree holds work that is nowhere else when
  it has commits not on proving, or changes not committed.
- A start resumes the worktree its issue's last attempt kept, if that one
  holds such work, and its brief says what is there; else its worktree is
  branched from the proving tip.
- Green: the issue closes, the worker ends, its worktree is removed unless it holds work that is nowhere else, read against the last attempt that started; only a worktree still on disk is resumed or discarded.
- When the proving tip cannot be read, what a worktree holds is unknown,
  so it is held: a worker that ends keeps its worktree, start-over
  discards it (keeping its branch), and a start refuses and takes nothing.
- A kept worktree that went bad is discarded on purpose with `issue start-over --why`, only while no worker is on the issue.
  Closing an issue unlanded removes its worktree too. Either way its
  uncommitted changes are committed and its branch kept as
  `<branch>-abandoned`, named in a comment.
- A start and the commands that abandon a worktree (start-over, close,
  discard) each claim the issue, one at a time; a claim whose process is
  gone is cleared by the next tick.
- An open issue owns one worktree: its last started attempt's. A worker's
  worktree no open issue owns is a doctor finding, fixed by `discard`,
  which removes it the same way.
- A git worktree registered inside a worker's worktree (slimfit's
  comparison worktree) goes first when it holds no work; one that holds
  work is kept, and the removal around it changes nothing.

## Consequences

- `models/Worktrees.tla` checks the first and the start-over rule under
  TLC.
