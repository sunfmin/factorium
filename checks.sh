#!/bin/sh
# The checks ./gate.sh runs before the tests: the crates' lines (shown, not
# held), fmt, clippy (docs/adr/0031, docs/adr/0032).
set -eu
cd "$(dirname "$0")"

# Each crate's source lines against its target: a trend and a source of
# idle tasks, never a gate (docs/adr/0031-a-crate-s-line-target-is-a-trend-not-a-gate.md).
./lines.sh

# The domain is tested only from the outermost layer (docs/adr/0032; its
# `[lib] test = false`): a test there is never built, yet reads as one that
# holds the code (factorium#304). Its rules are tested as scenarios, like
# crates/factorium/tests/scenarios/order.rs.
if grep -rnE '#\[(cfg\(test\)|test)\]' crates/domain; then
    echo "checks.sh: a test in crates/domain never runs; move it to crates/factorium/tests/scenarios" >&2
    exit 1
fi

cargo fmt --all --check
cargo clippy --workspace --all-targets --quiet -- -D warnings
