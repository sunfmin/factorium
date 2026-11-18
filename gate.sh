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

# The tests spend 200 seconds of CPU or less once built (SPEC.md §6):
# user+sys of the cargo test tree, read from the shell's `times` before and
# after. CPU time is the work the tests do; wall time also counts how busy
# the host is.
bound=200
cargo test --workspace --no-run --quiet
cpu=$(mktemp)
trap 'rm -f "$cpu"' EXIT
times >"$cpu"
start=$(date +%s)
cargo test --workspace --quiet
times >>"$cpu"
took=$(( $(date +%s) - start ))
spent=$(awk 'NR % 2 == 0 {
  s = 0
  for (i = 1; i <= 2; i++) { split($i, t, "m"); sub("s", "", t[2]); s += t[1] * 60 + t[2] }
  if (NR == 2) before = s; else after = s
} END { printf "%d", after - before }' "$cpu")
echo "tests spent ${spent}s of CPU (${took}s wall)"
if [ "$spent" -gt "$bound" ]; then
  echo "gate: the tests spent ${spent}s of CPU, over the ${bound}s bound" >&2
  exit 1
fi
