#!/bin/sh
# The checks every change passes. Fast enough to run before each commit.
set -eu
cd "$(dirname "$0")"

# Each crate's source stays within its budget (SPEC.md §5).
# Raising one is a decision recorded in SPEC.md, not a side effect.
budget() {
  lines=$(find "crates/$1/src" -name '*.rs' -exec cat {} + | wc -l | tr -d ' ')
  printf '%-12s %5s / %5s lines\n' "$1" "$lines" "$2"
  if [ "$lines" -gt "$2" ]; then
    echo "gate: crates/$1 is over its budget of $2 lines" >&2
    exit 1
  fi
}
budget domain 7000
budget harness 8000
budget host 4000
budget factorium 4000

cargo fmt --all --check
cargo clippy --workspace --all-targets --quiet -- -D warnings

# The tests run in 60 seconds or less once built (SPEC.md §6).
cargo test --workspace --no-run --quiet
start=$(date +%s)
# As a fresh host: no git identity or config of the user's.
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 cargo test --workspace --quiet
took=$(( $(date +%s) - start ))
echo "tests took ${took}s"
if [ "$took" -gt 60 ]; then
  echo "gate: the tests took ${took}s, over the 60s bound" >&2
  exit 1
fi
