#!/bin/sh
# The fast gate (factorium.toml's `[gate] fast`): what every landing needs, run on its merge
# with proving. The release build and the models are the slow gate's
# (./slow.sh), before a promotion moves the base.
set -eu
cd "$(dirname "$0")"

./checks.sh

# The release script parses, and its cut runs against fakes (docs/adr/0004):
# cheap, so a change to it is held at its landing.
sh -n packaging/release.sh
sh packaging/release-test.sh

# The tests run once, instrumented, under slimfit (docs/adr/0032). What
# `slimfit check` holds is slimfit's, test time among it (`test_seconds`,
# the tests' CPU time): `slimfit check --help`, `slimfit help mutants-scope`.
slimfit check

# Each stand-in of the scenario world answers its contract's cases
# (truefit.toml): the stand-ins alone, never the real programs.
truefit contract

# Each document in its one home, within its budget, and no tracked file
# citing this host's tracker anew (docfit.toml; `docfit check --help`).
docfit check
