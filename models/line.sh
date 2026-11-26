#!/bin/sh
# A checkout's line's model (models/Line.tla), beyond what the gate checks
# (`truefit model`, and the kept trace):
#
#   models/line.sh trace [<scenario> [<file>]]
#       Runs one scenario (default: two landings stacked and landed in one
#       push, whose run is kept in models/line-run.jsonl) with the line's
#       records kept as a trace (crates/factorium/tests/scenarios/fixture/
#       line_trace.rs), and checks it against the model: `allowed`, or the
#       first step it does not allow.
#   models/line.sh broken
#       Checks each broken variant (BrokenVariants in the model) under the
#       config that checks its property: each must fail on that property.
#   models/line.sh finding
#       Checks what the pid file's lock rules out: two daemons at once
#       push a stack at once (LineTwoFinding.cfg). The code runs one.
set -eu
cd "$(dirname "$0")/.."

. models/tlc.sh

case "${1:-}" in
  trace)
    scenario=${2:-stacked::two_stacked_the_upper_green_first_land_in_one_push}
    file=${3:-models/line-run.jsonl}
    FACTORIUM_LINE_TRACE="$PWD/$file" cargo test -q -p factorium --test scenarios -- --exact "$scenario" >&2 || true
    truefit model line-run --trace "$file" | jq '.trace | del(.output)'
    ;;
  broken)
    broken models/Line.tla models/LineTwo.cfg models/Line.cfg models/LineAmend.cfg models/LineEnd.cfg models/LineLive.cfg
    ;;
  finding)
    expect "$(check models/Line.tla models/LineTwoFinding.cfg none)" OneAtATime "two daemons, which the lock rules out (models/LineTwoFinding.cfg)"
    ;;
  *)
    sed -n '2,15p' "$0"; exit 2 ;;
esac
