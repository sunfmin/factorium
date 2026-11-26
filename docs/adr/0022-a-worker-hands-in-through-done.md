# 22. A worker hands in through `done`, and its landing's gate is its one full gate

Status: accepted

## Context

A worker's own run of the whole gate is on a base proving has usually
moved past by the time it lands, so it proves nothing the landing's run
does not prove again, and costs the host as much. A worker learns what
happened only from what reaches its session's prompt.

## Decision

- Text is typed at a session only once its harness's own screen is up there: typed at a shell, a line is a command it runs.
  A question the harness opens on its own mid-session is dismissed first
  with the harness's own key.
- The worker runs its gate's cheap whole-repo steps (`./lines.sh`, fmt,
  clippy, `truefit contract`, `docfit check`) and its crates' tests,
  through `worker exclusive`, the rest left to its landing, the one full
  gate, on the merge ([ADR 0012](0012-the-line-gate-landing-promotion-cut.md)); then `worker done` (`worker gate`
  optional). Work that changes no code ends with `worker done
  --no-landing "<evidence>"`, and no gate runs.
- What it wants kept it records with `worker comment`, a comment on its issue that changes nothing else and goes to no one; decisions go to the watch (`worker escalate`).
  A follow-up it files with `worker file`; it changes no other issue.
- Red: the worker is told why and goes on; red twice in a row goes to the
  watch. A red that does not reach the session's prompt goes to the watch
  at once.
- A branch that does not merge onto the proving tip is not the second red when the red before was one onto an older tip, which the branch now holds: proving moved since the worker merged what it was told.
  It is told as for a first red, with what landed since that tip and the
  files that conflict; past `moved_reds_max` of these in a row, it counts.
