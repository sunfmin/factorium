#!/bin/sh
# The checks every change passes. Fast enough to run before each commit.
set -eu
cd "$(dirname "$0")"

./checks.sh

# The tests run once, instrumented, under slimfit (SPEC.md §6): no failure,
# no more uncovered lines or slower tests than .slimfit/baseline.json allows,
# and no source line the change adds outside a ratcheted mutants record
# (.slimfit/mutants/). So a landing without its coverage or its mutants
# record is stopped here, before it reaches proving (factorium#121).
# slimfit holds test time too: `test_seconds` is the tests' CPU time, build
# excluded, ratcheted per suite (`slimfit check --help`, factorium#123).
slimfit check
