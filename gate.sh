#!/bin/sh
# The checks every change passes. Fast enough to run before each commit.
set -eu
cd "$(dirname "$0")"

./checks.sh

# The tests run once, instrumented, under slimfit (SPEC.md §6). What
# `slimfit check` holds is slimfit's, test time among it (`test_seconds`,
# the tests' CPU time): `slimfit check --help`, `slimfit help mutants-scope`.
slimfit check

# Each stand-in of the scenario world answers its contract's cases
# (truefit.toml): the stand-ins alone, never the real programs.
truefit contract
