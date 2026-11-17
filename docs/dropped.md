# What factorium did not take from afkconvoy, and why

SPEC.md §7: each afkconvoy mechanism is either a scenario here or dropped on
purpose, listed below with why. The rest is kept, each with its scenario in
`crates/factorium/tests/scenarios/`.

## Dropped with what it served

| Mechanism | Why it is gone |
|---|---|
| Teams, roles, workflows, stages, verdicts, rework, consults, patrols | One kind of worker works one issue; the watch is the only other session. |
| GitHub: issues, pull requests, checks, branch protection, quota, Discussions, labels | The tracker is local; landing is git merge + push requiring the tip. |
| Jira deliveries | No deliveries to others' trackers. |
| Several hosts: remote claims, a shared proving branch, cross-host reports, leases a shared server holds | One host. |
| The daybook | Its notes are the watch's reports in the database. |
| The panel | `doctor` is the owner's view. |
| The asking session (human decisions asked in a resident session) | Everything goes to the watch; only spending money goes to the owner, in the report's `human` part. |
| Batched rounds and the pairing tree | One landing at a time per checkout onto the proving branch; the fast gate runs on each merge. |
| A PR red twice is handed to a new worker in a new worktree | Red twice goes to the watch (SPEC §9). |
| Remote refs and cache directories as state | One SQLite database (SPEC §3). |
| The object store the fleet owned | Nothing factorium keeps needs one. |
| A private repo's release cut on a Mac / several cut hosts | One host cuts. |
| Codex goals as the worker's frame | The harness can still keep a goal; factorium does not depend on it. |
| Model research and holding a model by its listed id | The kit names the model; the harness checks it. |
| Worker health judged by an external judge | Health is the session file, the terminal, and the readings. |
| Flux refinement types | Scenarios and mutation hold the rules (SPEC §9). |

## Not yet carried over

| Mechanism | Where it stands |
|---|---|
| An issue that needs the next version waits on the cut | Express it with `issue block` on an issue the cut closes; no automatic link yet. |
| A repo's declared gate needs (tools to install before the gate) | Install them yourself; `factorium.toml` has no `needs` yet. |
| A repo's declared heavy commands, queued without `worker exclusive` | Workers are told to use `factorium worker exclusive`; the guard does not yet reroute declared commands. |
| Usage windows read from each harness's status line | Accounts are limited when their quota screen shows; windows are not yet read. |
| Serial builds (cargo's fine-grain locking opted out) | Gate runs and exclusive commands go through the queue; per-build locking is not set. |
