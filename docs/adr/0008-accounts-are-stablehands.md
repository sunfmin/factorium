# 8. Engines, models and accounts are stablehand's to pick

Status: accepted

## Context

Which engine, model and account a session starts on was once kept in
factorium's kit, beside an order of engines and each engine's command. The
same subscriptions serve more than factorium (Guild's instances, other
hosts), and their usage windows and the limits they met are one fact
about the account, not one per program that starts on it. Two homes of
that fact drift: factorium would start on an account another program had
just used up.

## Decision

- An account is an engine subscription, kept in stablehand (its docs/adr
  0001), its one home: how it logs in, where it may run, its usage
  windows and the limits it met. factorium keeps no account of its own.
- Each start asks `stablehand pick`, counting factorium's own live
  sessions on each account, and starts the engine, model and account it
  answers, by the command stablehand keeps for that engine (`stablehand
  command set`); factorium adds only its own arguments, the guard and the
  session. A checkout's pin narrows the pick to its engine; an issue's own
  model or account narrows it further.
- While the pick answers none for an issue, its issue waits, behind ready issues that can start, and holds admission (`kit: …`, the pick's reasons) only when none can.
- With stablehand missing or failing, claude-code alone starts, by `claude --dangerously-skip-permissions` (built in, for that case only), on the host user's own login.
  It knows no other engine's accounts or windows without stablehand; an
  issue that names an account waits, and doctor finds it (`stablehand`).
- A session on an account is handed its login as `stablehand credential`
  gives it, in a home factorium builds per account, so no two accounts
  share a home and nothing else of the account's home reaches the session.

## Consequences

- A worker whose screen says its account is used up ends, its issue
  starts again, and stablehand is told (`stablehand limit`).
- `factorium accounts` answers `stablehand status`'s accounts.
