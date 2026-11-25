#!/bin/sh
# The checks every change passes. Fast enough to run before each commit.
set -eu
cd "$(dirname "$0")"

./checks.sh

# The release script, as far as it goes without publishing (docs/adr/0004):
# its cut against fakes, then the universal build the tap signs, at
# Cargo.toml's version.
sh -n packaging/release.sh
sh packaging/release-test.sh
DIST=.slimfit/out/dist packaging/release.sh dry

# The tests run once, instrumented, under slimfit (SPEC.md §6). What
# `slimfit check` holds is slimfit's, test time among it (`test_seconds`,
# the tests' CPU time): `slimfit check --help`, `slimfit help mutants-scope`.
slimfit check

# Each stand-in of the scenario world answers its contract's cases
# (truefit.toml): the stand-ins alone, never the real programs.
truefit contract

# The models (truefit.toml: the heavy queue, models/HeavyQueue.tla; an
# issue's worktrees, models/Worktrees.tla; a checkout's line,
# models/Line.tla) hold their properties, and allow the scenario runs kept
# as their traces (`models/heavy.sh`, `models/worktrees.sh`,
# `models/line.sh` for the rest).
truefit model
truefit model heavy-run --trace models/heavy-run.jsonl
truefit model worktrees-run --trace models/worktrees-run.jsonl
truefit model line-run --trace models/line-run.jsonl

# Each document in its one home, within its budget, and no tracked file
# citing this host's tracker anew (docfit.toml; `docfit check --help`).
docfit check
