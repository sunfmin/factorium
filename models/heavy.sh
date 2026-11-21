#!/bin/sh
# The heavy queue's model (models/HeavyQueue.tla), beyond what the gate
# checks (`truefit model`, and the kept trace):
#
#   models/heavy.sh trace [<scenario> [<file>]]
#       Runs one scenario (default: the settle one, whose run is kept in
#       models/heavy-run.jsonl) with the queue's records kept as a trace
#       (crates/factorium/tests/scenarios/fixture/trace.rs), and checks it
#       against the model: `allowed`, or the first step it does not allow.
#   models/heavy.sh broken
#       Checks each broken variant (BrokenVariants in the model) under the
#       config that checks its property: each must fail on that property.
set -eu
cd "$(dirname "$0")/.."

. models/tlc.sh

case "${1:-}" in
  trace)
    scenario=${2:-queue::beside_what_runs_asks_in_one_second_start_one_per_settled_reading}
    file=${3:-models/heavy-run.jsonl}
    FACTORIUM_HEAVY_TRACE="$PWD/$file" cargo test -q -p factorium --test scenarios -- --exact "$scenario" >&2 || true
    truefit model heavy-run --trace "$file" | jq '.trace | del(.output)'
    ;;
  broken)
    broken models/HeavyQueue.tla models/HeavyStarts.cfg models/HeavyEnds.cfg models/HeavyRules.cfg
    ;;
  *)
    sed -n '2,12p' "$0"; exit 2 ;;
esac
