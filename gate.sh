#!/bin/sh
# The checks every change passes. Fast enough to run before each commit.
set -eu
cd "$(dirname "$0")"

./checks.sh

# The tests run in 60 seconds or less once built (SPEC.md §6).
cargo test --workspace --no-run --quiet
start=$(date +%s)
cargo test --workspace --quiet
took=$(( $(date +%s) - start ))
echo "tests took ${took}s"
if [ "$took" -gt 60 ]; then
  echo "gate: the tests took ${took}s, over the 60s bound" >&2
  exit 1
fi
