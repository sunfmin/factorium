# 32. Tests only at the outermost layer

Status: accepted

## Context

A test beneath the program's entry holds a shape the next refactor
changes, and passes while what the CLI prints is wrong. The programs the
app runs cannot run in every test, yet what the app makes of their
answers must.

## Decision

- `App` is the program's one entry: every CLI command and the daemon's tick are each one method of it. Every scenario drives `App` as the CLI does and reads only what the CLI could print (`crates/factorium/tests/scenarios/fixture.rs`).
- No crate has unit or doc tests (`[lib] test = false`; `checks.sh` fails one in `crates/domain`). The effect crates' tests (`crates/host/tests`, `crates/harness/tests`) hold only their edge: the real program, file or process.
- Orca, Homebrew, stablehand, slimfit, rolesmith and launchd are stood in for at the one port the app runs programs through, with answers shaped as the real ones gave them. Each stand-in is held to its program by a truefit contract (`truefit.toml`): against the fakes in the fast gate, against the real program by hand (`truefit contract --real <name>`).
- A harness's stand-in session writes its file from that harness's own module, so no second copy of a format exists. Host readings and the clock are in-process stand-ins the scenario moves; no test sleeps.
- Git is real: a bare remote and clones, made once per run and copied, because landing's correctness is git's.
- A handful of tests run the built binary to prove the wiring (`crates/factorium/tests/scenarios/binary.rs`), in the scenarios' binary.
- A scenario is held to account by a change's mutants and slimfit's coverage-plus-review task; there is no whole-workspace mutation run (`slimfit help mutants-scope`).
- The fast gate runs the tests once, under `slimfit check`, which holds `test_seconds`: the CPU time (user+sys) of the tests' own processes, build excluded (`slimfit help host-load`). CPU, not wall time, so the bound fails when the tests got slower and passes the same code however busy the host is: the suite took 24s idle and 75-193s at load 55-73 on 18 cores (`time ./gate.sh`) while its `test_seconds` held.
- Nearly all of that CPU is git's, about 50 processes a scenario: git runs as its binary, not macOS's `/usr/bin/git` shim, which more than doubled each call.

## Consequences

- A refactor beneath `App` changes no test; a scenario fails only when what the CLI prints changes.
