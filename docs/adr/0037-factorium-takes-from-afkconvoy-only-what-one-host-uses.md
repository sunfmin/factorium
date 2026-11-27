# 37. factorium takes from afkconvoy only what one host uses

Status: accepted

## Context

factorium succeeds afkconvoy, which ran teams of agents across several
hosts against GitHub. factorium runs one kind of worker on one machine
against a local tracker; each afkconvoy mechanism it keeps has a scenario
in `crates/factorium/tests/scenarios/`, and the rest is dropped on purpose.

## Decision

- One kind of worker works one issue; the watch is the only other session.
  afkconvoy's teams and their roles, workflows, stages, verdicts, rework,
  consults, patrols and intake are gone.
- The tracker is local, and landing is a git merge and a push that
  requires the tip: no GitHub issues, pull requests, checks, branch
  protection, quota, Discussions or labels, and no deliveries to others'
  trackers (Jira).
- One host: no remote claims, shared proving branch, cross-host reports or
  leases a shared server holds. One host cuts, so neither a private repo's
  cut on a Mac nor several cut hosts. Nothing factorium keeps needs the
  object store the fleet owned.
- The daybook's notes are the watch's reports, in the database.
- A worker's health is its session file, its terminal and the readings,
  never an external judge's. A harness may keep a goal (Codex's); factorium
  does not depend on it.
- The harness crate offers what factorium calls and no more: afkconvoy's
  token and step counting, status hooks read back, model, flag and login
  probes, notes, resumed sessions, goal rows, first-run and limits files,
  background-work reads, pi's status extension and DSH's profile files are
  gone. Each comes back from git history with its caller.

Not carried over yet:

- `factorium.toml` declares no tools to install before the gate; a role's
  needs are checked by rolesmith before its worker starts.
- Workers are told to run heavy commands through `worker exclusive`; the
  guard does not reroute a repo's declared heavy commands.
- Gate runs and exclusive commands go through the heavy queue, but cargo's
  fine-grain build locking is not opted out: builds are not made serial.

## Consequences

Whoever wants a dropped mechanism back brings it with its caller and its
scenario, and replaces the line of this ADR that dropped it.
