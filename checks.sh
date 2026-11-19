#!/bin/sh
# The checks ./gate.sh runs before the tests: the line budgets, fmt,
# clippy (SPEC.md §5, §6).
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
budget domain 6700
budget harness 8000
budget host 4000
budget factorium 4300

cargo fmt --all --check
cargo clippy --workspace --all-targets --quiet -- -D warnings
