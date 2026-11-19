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
#
# They spend 310 seconds of CPU or less once built (SPEC.md §6): user+sys
# of slimfit's tree, read from the shell's `times` before and after. CPU
# time is the work the tests do; wall time also counts how busy the host is.
# The build is slimfit's instrumented one, made here first so its
# dependencies are not timed; slimfit itself rebuilds only the workspace's
# crates.
bound=310
(
  eval "$(cargo llvm-cov show-env --export-prefix)"
  cargo nextest run --workspace --no-run --target-dir target/llvm-cov-target
)
cpu=$(mktemp)
trap 'rm -f "$cpu"' EXIT
times >"$cpu"
start=$(date +%s)
status=0
slimfit check || status=$?
times >>"$cpu"
took=$(( $(date +%s) - start ))
spent=$(awk 'NR % 2 == 0 {
  s = 0
  for (i = 1; i <= 2; i++) { split($i, t, "m"); sub("s", "", t[2]); s += t[1] * 60 + t[2] }
  if (NR == 2) before = s; else after = s
} END { printf "%d", after - before }' "$cpu")
echo "tests spent ${spent}s of CPU (${took}s wall)"
[ "$status" -eq 0 ] || exit "$status"
if [ "$spent" -gt "$bound" ]; then
  echo "gate: the tests spent ${spent}s of CPU, over the ${bound}s bound" >&2
  exit 1
fi
