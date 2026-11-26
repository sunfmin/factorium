# 8. A worker's role is a directory read from the host, and a waiting issue waits for a person

Status: accepted

## Context

Whoever files an issue decides what its worker is: Guild keeps roles and
accounts' quotas across hosts, and hands factorium the role a worker
plays as a directory of rules and skills, say lore checked out at a rev.
That directory's path holds only on this host. A role a worker could
write to would change the rules of every worker after it.

An issue that needs a person is no work for the watch or the daemon:
anything that moved it again would start a worker on a question nobody
answered.

## Decision

- A role is a directory, by its absolute path. A worker starts in a
  directory of its own whose rules are factorium's worker rules and then
  the role's, and whose skills are the role's, linked, the role's
  directory read and never written.
- A checkout may have a default role, kept in the host's checkout list
  and not in the repo's `factorium.toml`, since the path holds only on
  this host. An issue that names no role takes it; an issue's own role
  always wins over it. A role is named at `checkout add --role`,
  `checkout edit --role`, `issue add --role` or `issue edit --role`,
  each refused unless it is such a directory.
- A role directory that holds a rolesmith.toml is checked by `rolesmith
  doctor` before a worker starts with it, and what it lacks installed
  first; while it still lacks anything, no worker starts with it and
  doctor names what it lacks.
- A role directory is linked live, so a role changed while a worker runs
  changes what it reads. On each tick, a live worker whose role directory
  holds other content than it last knew of is told so once, through its
  outbox (ADR 0027), naming each file under the role's directory that
  changed, was added or went, and asking it to re-read the role's core
  skill and those files; where the rules changed, its session's rules are
  written again. One waiting for the watch, unless parked on its engine's
  limit, is told once it works again.
- An issue waiting for a human stays so: only a person's `issue ready`
  moves it again.

## Consequences

- A worker already started keeps the directory it started in; an edit of
  the role reaches the next attempt, and is a comment on the issue.
- `issue show` answers the role the next worker starts with (`role`), and
  whose it is (`role_from`: `issue` or `checkout`).
