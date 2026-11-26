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

## 5. Crates

| Crate | Holds | Target (source lines) |
|---|---|---|
| `factorium-domain` | The pure half, on factum: the issue, worker, landing, promotion, cut, account and watch aggregates and processes; admission, the heavy-command queue's start rule, a worker's health (up, hot, hung, silent, quota) decided by one command from one observation; the disk floor as a pure function of readings; doctor's findings. No IO, no clock. | 6,240 |
| `factorium-harness` | The six harnesses: start line and brief, the hook's tool call, session files and turn lines, quota and refusal screens; where a session on an account keeps its files (the account itself is stablehand's). Moved from afkconvoy-harness, nothing GitHub in it, and only what factorium calls. | 8,000 |
| `factorium-host` | The effects: git (worktrees, merge, push requiring a value), orca (terminals), host readings (CPU ticks, memory, pressure, disk, process creation), process trees and sampling, the programs it reads the answers of (Homebrew, stablehand). | 4,000 |
| `factorium` | The binary: CLI, the daemon (factum's tick plus three workers — the starter, the gate runner, the lander), the watch's terminal, `doctor`'s text. | 4,830 |

A crate's target is a trend, not a gate (factorium#163): `./lines.sh`,
which `checks.sh` runs, prints each crate's lines against its target here
and an overage, and never fails. A crate over its target, or that grew at
each of its last five landings that changed it, is a task: the idle tasks
line runs `./lines.sh --tasks` after slimfit's `next`, and a `crate-lines`
issue names the crate and the files that grew most (§1.16). Its worker
deepens the crate; a target changes only with its reason in the issue that
changes it. fmt and clippy stay bounds.

**What a repo declares**, in `factorium.toml` at its root (§3); every
table may be left out:

```toml
[gate]
fast = "./gate.sh"      # what every landing needs, on its merge
fast_limit = 600        # seconds
slow = "./slow.sh"      # what fast lacks (§1.13)
slow_limit = 3600
regenerated = [".truefit/models/*.json"]  # git globs: records the gate rewrites (§1.12)

[cut]                   # a version from the base tip (§1.14)
steps = ["./release.sh $FACTORIUM_VERSION"]
formula = "tap/name"    # the Homebrew formula upgraded once cut
limit = 3600

[idle]                  # only while the host is idle (§1.16)
steps = ["./soak.sh"]
tasks = "slimfit --version && slimfit next --all"   # filed as issues (§1.16)
every = 86400           # at most once per this many seconds
limit = 14400           # seconds, all the steps together; past it, red
```

**For Guild.** Guild depends on `factorium-harness`, `factorium-host`
and the domain's neutral parts (admission, the queue's start rule,
hot-worker rules, landing onto a proving branch), as git dependencies
pinned to a revision. Those public items name no issue, worker, watch or
checkout: what only the caller knows it passes in, or says through a
trait it implements. They live in `factorium_domain::neutral`; factorium's
own modules wrap them with what it knows. A test from outside
(`crates/factorium/tests/neutral.rs`) checks they name none of those and
depend on nothing else of factorium.

## 6. Tests: only at the outermost layer

- **Scenarios through `App`.** `App` is the program's one entry: every
  CLI command is one method of it, and the daemon's tick is one too.
  Every scenario drives `App` as the CLI does and reads only what the CLI
  could print. No crate has tests of its own beneath it.
- **What is faked, and how.** Orca, Homebrew, stablehand, slimfit,
  rolesmith and launchd: the programs themselves. The app runs them through one port
  (run a program, get its answer), and a scenario stands in there with
  answers shaped as the real programs gave them — Orca's keeping terminals
  and what was sent to them — so what the app makes of their answers runs
  in every scenario. Each stand-in is held to its real program by a truefit
  contract (`truefit.toml`): the cases of what the app reads of it, run
  through one driver against the scenarios' own fakes in the fast gate
  (`truefit contract`) and against the real program by hand (`truefit
  contract --real <name>`), the last agreeing run kept in `.truefit/`.
  Harnesses: a stand-in session that writes the session file each harness
  describes, line by line, from its own module, so no second copy of a
  format exists. Host readings and the clock: in-process stand-ins the
  scenario moves.
  Git is real: a bare remote and clones in a temporary directory, made
  once per test run and copied, because landing's correctness is git's.
- **The binary.** A handful of tests run the built binary to prove the
  wiring: argument parsing, JSON answers, the guard refusing a worker an
  owner command, the CLI and the daemon sharing the database.
- **Real harnesses.** Opt-in smoke tests per harness, run by hand.
- **Every scenario is held to account**, by two mechanisms and no
  whole-workspace mutation run (factorium#114): a change's mutants, and
  slimfit's coverage-plus-review task, which finds a scenario that only
  repeats another, or shares its setup, to delete or merge. Both rules are
  slimfit's ([`slimfit help mutants-scope`](https://github.com/sunfmin/slimfit/blob/main/src/help/mutants-scope.md); slimfit#31), not restated here.
- **Bounds.** No test sleeps; time moves when a scenario moves it.
  slimfit holds test time, not `gate.sh`: its `test_seconds` is the CPU
  time (user+sys) the tests' own processes spent, build excluded, and the
  gate fails when it rises past the baseline's tolerance (`slimfit check
  --help`; `slimfit help host-load`, [src/help/host-load.md](https://github.com/sunfmin/slimfit/blob/main/src/help/host-load.md)).
  CPU time, not wall time, because the bound must fail when the tests got
  slower and pass the same code on the same host
  however busy it is: the suite took 24s idle and 75-193s at load 55-73
  on 18 cores, while its CPU time held (factorium#103). Until
  factorium#123 `gate.sh` held a CPU bound of its own around the run;
  slimfit#43 made that the same fact twice, so it went. Nearly all of the
  tests' CPU is git's processes, about 50 a scenario, so a scenario's cost
  is the git it runs: git is run as its binary, not macOS's `/usr/bin/git`
  shim, which more than doubled each call; the scenarios' homes turn off
  git's auto maintenance; a world's repos are copied in-process; a
  scenario of several worlds runs them at once (`each`, `at_once!`), so it
  takes its slowest world's time, not their sum; and the binary's tests
  run in the scenarios' binary, beside them rather than after
  (factorium#98 halved the suite this way).
- **slimfit holds the suite, in the fast gate.** `./gate.sh` is
  `./checks.sh` (lines, §5; fmt, clippy) then `slimfit check`
  against the committed `.slimfit/`, in place of a plain `cargo test`, holding
  the tests' time as above, then `truefit contract`; the slow gate (`./slow.sh`)
  is the release build and the models. What
  `slimfit check` holds, a change's record of its mutants among it, is
  slimfit's ([`slimfit help mutants-scope`](https://github.com/sunfmin/slimfit/blob/main/src/help/mutants-scope.md)), not restated here; factorium
  holds it at the base, once per promotion (ADR 0006). The worker follows
  `slimfit next`; what is factorium's is the priority: a mutants run goes
  through `factorium worker exclusive --background`, a run whose baseline
  it ratchets through `factorium worker exclusive` (§1, row 9). And the
  second agent that picks a change's mutants ([`slimfit help picker`](https://github.com/sunfmin/slimfit/blob/main/src/help/picker.md)):
  `factorium worker pick-mutants` asks a fresh headless session, on the
  engine, model and account stablehand picks among the other engines
  that can be asked headless (else the worker's, anew; §1.7), within `pick_bound`, given only
  slimfit's goal, the diff from origin's proving and `slimfit check
  --mutants`; its time and tokens go on the issue (factorium#147). Claude
  Code, Codex, Grok and agy can be asked, each in a user directory of its
  own that holds only its login (agy's, its HOME, also an agent with no
  tools, factorium#200). One whose answer is no JSON object, or not one
  as asked, is asked once more in the same session (Claude Code and Grok
  by `--resume`, agy by `--conversation`; Codex's session is ephemeral,
  so it is not), told only what was wrong and the one form the answer
  takes; both asks' time and tokens count, and the record says it was
  "answered as asked when asked again" (factorium#226). One that fails,
  or answers off the form twice, hands the pick to the one stablehand
  picks among those left, the worker's own last, and the record names each
  that failed and why; when none is left the worker gets the last refusal,
  never a pick (factorium#154). When slimfit lists no candidates (an
  empty listing, or its refusal that the tool listed none, slimfit#77)
  of a change its listing's `landing` says adds no source line and removes
  no test, no picker is asked: it answers `none`, "slimfit lists no
  candidates", with no picker, and the issue keeps that line
  (factorium#178). Otherwise the gate still wants a record of none, so the
  picker is asked as ever, given what slimfit printed, and answers why
  none and the `ratchet --no-mutants` naming it (factorium#247). The second agent that judges a change's growth
  ([`slimfit help reviewer`](https://github.com/sunfmin/slimfit/blob/main/src/help/reviewer.md)):
  `factorium worker review-growth` asks one the same way, with the same
  fallback and bound, given only slimfit's goal and the diff; it answers
  `as_is` with why or `cut` with what, and the `slimfit ratchet` that
  writes the growth record naming it (`harness:session:model`) and the
  issue it serves; its time and tokens go on the issue (factorium#174).

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
