# 6. Mutants are owed at promotion, by one issue

Status: accepted (the owner, 2026-11-26: 「快测试结束后就合并到 proving 上面，然后在 proving 上面 promotion 的时候再去用 worker 做 mutant，统一对所有提交的 Diff 做一次」)

## Context

Each worker picked and ran its change's mutants before `done`: 21 of 86,
79 of 161 and 12 of 106 minutes of three changes, all before the landing.

## Decision

- A checkout whose `slimfit.toml` sets `[check] mutants_target` (factorium:
  `origin/main`) holds the record at the base: a landing waits for its fast
  gate alone, and its worker runs no mutants before `done` (its brief says
  so). The growth review stays before `done`: it is short, and a cut is
  cheapest before the landing.
- Before a promotion's slow gate, the daemon asks slimfit's listing of the
  range since the base (`slimfit check --mutants --target <it>`): git
  alone, not a test run. A range that adds source lines or removes tests
  with no record owes one: the daemon files one issue, "Mutants of
  <base>..<tip>", task `promotion-mutants`, which starts ahead of the
  ordinary issues, behind the urgent. Its worker picks over the range
  (`worker pick-mutants --target <base>`), kills or loosens each survivor,
  and lands the tests and the record on their fast gate.
- A checkout at its `workers_per_checkout` does not hold that issue;
  admission's load rules do
  ([ADR 0016](0016-a-checkout-runs-at-most-workers-per-checkout.md)).
- The promotion waits for that issue at most `mutants_bound` from its
  filing. Past it, or while no worker can start it, it goes on: a survivor
  is a weak test, not a broken change, so it does not hold the base back.
  The issue stays open, for the range it was filed for; one is open per
  checkout at a time, so a later promotion files none while it is.
- A promotion by hand goes on whatever is owed.
- `doctor`'s `mutants` names each promotion waiting so, its issue and when
  the wait ends.

## Consequences

- The fast gate's `slimfit check` no longer holds a landing's record.
- One picker sees the whole range, so it can spread picks across it;
  `picks_max` stays at slimfit's default until slimfit drops the cap.
- A range that removes tests makes slimfit measure the base tree once
  while listing.
