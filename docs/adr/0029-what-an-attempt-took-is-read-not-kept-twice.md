# 29. What an attempt took is read from what is already kept

Status: accepted

## Context

Whoever settles an issue's cost (Guild) needs each attempt's time, its
tokens and the role it ran with; the owner needs where that time went. A
second copy of the queue's, the line's or the outbox's records would drift
from them, and a session's harness keeps its own count of tokens.

## Decision

- `issue show` answers `spent`: each attempt's time from start to end (a
  live one's, to now), in ms, and the tokens its session spent, given
  (cached counted) and written, with their sums.
- Tokens are read from the session files that record the turn, as the
  attempt ends, and kept, since its harness may clear them: Claude Code's (each assistant message's `usage` once, by its id), Codex's (its last `token_count`), pi's (each assistant message's `usage`), agy's (each `PLANNER_RESPONSE` of `transcript.jsonl`). DSH's, Kimi Code's and Grok's are not read: `null`.
- Each attempt answers the role it started with and `role_hash`: a hash of
  its rules and every file under its skills, read through the link, by
  path and bytes, taken at its start. It is of the content alone, the same
  from one build to the next; factorium reads no rolesmith file or rev.
- An attempt's steps (`issue show --steps`) are read from what the
  database keeps of its heavy runs, landings, waits, parks and messages,
  written nowhere twice; `heavy_runs` is built from the heavy queue's events.
- `spent <checkout> [N|FROM..TO] [--since TIME]` reads those steps of every attempt of the issues named (all, open and closed, by default) and stores nothing.

## Consequences

- Steps kept before their kinds and whys were read as they were kept: a
  red with no kind, a park with "(no reason given)", a heavy command with
  no outcome.
- A picker's (`worker pick-mutants`) cost is its own comment, not an
  attempt.
