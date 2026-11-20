#!/bin/sh
# The checks ./gate.sh runs before the tests: the crates' lines (shown, not
# held), fmt, clippy (SPEC.md §5, §6).
set -eu
cd "$(dirname "$0")"

# Each crate's source lines against its target (SPEC.md §5): a trend and a
# source of idle tasks, never a gate (factorium#163).
./lines.sh

cargo fmt --all --check
cargo clippy --workspace --all-targets --quiet -- -D warnings
