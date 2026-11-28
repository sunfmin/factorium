# 11. Each resource has one queue, shortest declared hold first

Status: accepted

## Context

Some things one worker uses at a time: a phone, a build host. Ordering
their users by hand-made `issue block` chains between issues guessed how
long each issue takes, which nobody knows before it starts. A holder
knows how long it holds when it asks.

## Decision

- A resource is a name and a capacity rule, with one queue each, the rule
  of `crates/domain/src/neutral/queue.rs`: the shortest declared hold first — a hold's `--for`; a command declares none and is read as `hold_default` — oldest first among equals; one waiting past `starve_bound` goes ahead of all that have not, oldest first, so a long one never starves.
  The order is the resource queue's, never the issue's.
- A named resource is declared in the host's settings (`settings
  --resource <name> --slots <n>`), never in a checkout's `factorium.toml`:
  a phone serves every checkout on the host. `slots = n`: n holders at most (1 is a mutex).
  The resource `host` is never declared; its rule is the room the host's
  readings allow (ADR 0010). A new kind of lock is a new rule and nothing
  else.
- A command asking for several is granted all at once or not at all: one waiting for a resource whose slots are all held is passed over in every queue, as a stale one is, so it holds none up and never holds one while it waits for another.
- A worker's hold across commands (`worker hold`) waits as a command does; with `--between 22:00-07:00` (the host's local time, every day; an overnight run on the charger) it is passed over outside its window, joining the line only inside it, its starve bound counted from the window's opening; granted, it outlives its waiter until `worker release <name>` or its worker ends, and the worker's own commands asking for what it holds run inside it, taking no slot of their own.
- A repo's gate may name the resources each of its gates holds besides `host` (`factorium.toml`'s `[gate] resources`): a device one gate run may use at a time, such as a Mac a repo's screen tests run on. Its stacked landings' gates then take it in turn through the queue, each gate's limit starting once it holds it, so none is ended by its limit while it waits behind another; a lock of the repo's own, which the queue cannot see, would count that wait in the limit. The name is the host's settings', as any resource's: one they do not declare makes the gate red, naming the fix.
- A hold is never cut short, whatever the hold bound; past its declared until it is a finding for the watch, as a silent worker is.

## Consequences

- A named resource is given back by one path: its command ends,
  `release`, or its holder is gone, which the next tick gives back.
- What doctor's `resources` and `workers` show of each resource is their
  `--help`'s.
- Promotion, cut, the issue claim and the accounts' windows are not
  resources; each becomes a rule when a second need shows.
