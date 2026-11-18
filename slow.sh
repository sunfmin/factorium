#!/bin/sh
# The slow gate (SPEC.md §5, §6): the fast gate, then slimfit holds the
# suite against .slimfit/baseline.json: no failures, no more uncovered
# lines, no slower tests than the baseline allows.
set -eu
cd "$(dirname "$0")"

./gate.sh
slimfit check
