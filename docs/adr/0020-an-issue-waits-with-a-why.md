# 20. An issue waits for the issues and the time it names, each block with its why

Status: accepted

## Context

Issues need each other's code, across checkouts too, and a tool one repo
uses from another reaches it only once a cut installs it. A tick that saw
an issue ready before what it needs landed would start a worker on code
that is not there. A block whose reason is lost cannot be judged later:
nobody can tell whether it still holds.

## Decision

- `issue add` makes an issue, ready unless it says otherwise, with the
  issues it waits for recorded with it, so no tick sees it ready without
  them.
- One of another checkout is met once it is closed (landed); `<checkout>#N@cut` is met only once a cut of that checkout holds its landing, and installed it where its `[cut]` names a formula, for a tool used from another repo.
- Every block says why: `--blocked-by` and `issue block --by` are refused without `--why <text>`, kept with the block.
  Each `--blocked-by` takes the `--why` after it, so an issue filed with several blocks keeps a reason for each; one `--why` with one `--blocked-by` is its reason wherever it stands.
  A block made before whys were asked for reads "(no reason given)".
- Two issues that only use one device or host do not block each other:
  that is a resource ([ADR 0011](0011-resources-queue-shortest-hold-first.md)), which the watch turns such a block into.
- An issue can wait until a time too, met on the first tick at or after
  that time, as a met block is.
- A tick admits one start per reading, of the ready issues unblocked and
  untaken: urgent first; then the fewest live workers in its checkout,
  ready longest, checkout, number.
- A worker whose issue the watch blocks (on issues, or until a time) is parked: once it is idle (it waits, for any reason, is in a wait it declared, or sits at its prompt, its turn ended), the first tick that finds everything its issue waits for met, before it went idle or after, sends it back to work as `watch restart` does, told which blockers were met at which landing, and to merge the proving tip before it gates again.

## Consequences

- No round runs for a parked worker, and nobody has to notice the landing
  that frees it.
- `issue show` answers each wait with its why and how it was met; doctor's
  `parked` names each unmet one with its why.
