#!/bin/sh
# The slow gate (SPEC.md §5, §6): the fast gate again on the proving tip,
# before a promotion. Every landing already passed it on its own merge;
# this holds the tip a promotion moves, against .slimfit/baseline.json.
set -eu
cd "$(dirname "$0")"

./gate.sh
