#!/bin/sh
# The slow gate (SPEC.md §5, §6): the fast gate's checks, then slimfit
# runs the suite once, instrumented, and holds it against
# .slimfit/baseline.json: no failures, no more uncovered lines, no slower
# tests than the baseline allows. The uninstrumented test run and its 60s
# wall bound are the fast gate's, which every landing on proving passed.
set -eu
cd "$(dirname "$0")"

./checks.sh
slimfit check
