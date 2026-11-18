#!/bin/sh
# The checks both gates run before the tests: the line budgets, fmt,
# clippy. ./gate.sh runs the tests after them; ./slow.sh runs them once,
# instrumented, under slimfit (SPEC.md §6).
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
