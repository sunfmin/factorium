# factorium — specification (draft 1, 2026-11-17)

factorium runs AI coding agents on one machine while nobody watches: it
takes issues from a local tracker, gives each to a worker in its own
worktree, lands what passes the repo's gate onto a proving branch,
promotes the proving branch to the base branch once the slow gate passes,
cuts versions, and keeps the machine healthy. A resident **watch** agent
deals with everything that goes wrong.

It succeeds afkconvoy and is built on factum. Guild depends on its crates.

Status: approved 2026-11-17. Phases 1–6 are done. Their real runs: one
Claude Code worker on a throwaway repo, one real watch round, and
factorium cut its own v0.1.0 through Homebrew. Phase 3 had real runs for
Claude Code, and for DSH and Kimi Code: each worker started by
a daemon from its kit entry, refused by the real guard when it pushed, landed;
and for agy: the same, its tokens read from its real
transcript, and a real headless ask read; codex, pi and grok have had no
such run on this host. Phase 7 waits
on the owner: which repo moves first, and how Guild moves its gate and landing dependencies, since factorium has no
counterpart for afkconvoy-gate and afkconvoy-landing. What afkconvoy had
and factorium does not is in `docs/dropped.md`.

## 7. From afkconvoy

Kept, as afkconvoy decided them: the session file is the turn; the kit
orders harnesses and a checkout may pin one; a login that refreshes is
bound to one machine (which account a worker starts on is stablehand's
pick now, §1.7); admission alone decides each start; the queue starts by the
host's CPU reading, never starves, and bounds a hold; hot workers and
their evidence; the disk floor and reclaiming `target/`; serial builds
(cargo's fine-grain locking opted out); the gate is local, declared, fast
and slow, recorded by tree; the proving branch and promotion; the fleet
cuts its own versions; the watch, its look, episodes, rounds and report;
guards, not identities; the help is the manual.

Not in factorium: teams, roles kept by factorium (an issue, or a checkout as its default, may name a role directory, §1.2–1.3, and that is all), workflows, stages, verdicts, rework,
consults, patrols, intake and flow labels; GitHub in every form (tracker,
pull requests, checks, branch protection, quota, Discussions); the
daybook; the panel; batched rounds and the pairing tree; several hosts
(remote claims, a shared proving branch, cross-host reports: `issue move`,
§4, is not that, for no state is shared between hosts and nothing is read
from the other host afterwards: one owner command hands one issue over by
filing it there); Jira
deliveries; the asking session; remote refs as state; caches as state.

Each afkconvoy mechanism added after an incident is either a scenario
here or dropped on purpose, listed in `docs/dropped.md` with why.

## 8. Phases

| # | What | Done when |
|---|---|---|
| 0 | This spec | The owner approves it |
| 1 | Skeleton: crates, factum, `App`, the stand-ins, `gate.sh` with budgets, CI | Gate green |
| 2 | The path with one harness: issue → worker → `done` → landing on proving → promotion → issue closed | Its scenarios pass; one real Claude Code smoke run |
| 3 | All six harnesses, the kit, accounts and their windows | Scenarios; a smoke run per harness |
| 4 | Admission, the heavy-command queue, hot workers, the disk floor | Scenarios |
| 5 | The watch and `doctor` | Scenarios; a real watch round |
| 6 | Cut and self-upgrade through Homebrew | factorium cuts its own version |
| 7 | Move repos from afkconvoy one at a time (a repo belongs to one); Guild moves its dependencies from afkconvoy's crates to factorium's | First repo landed by factorium; Guild builds against it |

## 9. Decisions, and why

1. **Repo declarations live in the repo** (`factorium.toml`): they change
   with the code and are reviewed with it.
2. **Every checkout has a proving branch**: one path. A repo with no slow
   steps promotes as soon as it lands.
3. **A landing red twice in a row goes to the watch**; no second worker in
   a new worktree. A merge red on a proving tip that moved since the last
   red's tip, which the branch holds, is not the second: the worker did
   what the first asked, and in one day eight watch rounds only said
   "merge the new tip" (factorium#171). `moved_reds_max` bounds how many
   in a row are let off.
4. **No Flux.** Scenarios and mutation hold the rules; afkconvoy's fast gate
   spent 65 of its 138 seconds in Flux.
5. **factorium is the one writer of a managed repo's base.** A repo
   factorium manages has its base moved only by promotion; anything else
   that moves the base turns promotion red.
6. **A landing waits for no review** (factorium#167). It lands on the fast
   gate alone and reaches only proving, which is promoted after the slow
   gate; verdicts and rework stay out of factorium (§7). Review comes after
   landing, from outside: Guild's warden, and control's job on a proving
   branch, file what they find as new issues. What would reopen it: a
   landing that harms before review can catch it (secrets, data loss,
   something externally visible) — then a finding that holds promotion
   (`factorium hold`), not an approval on every landing; or measured
   evidence that findings after landing cost more than review before it.
