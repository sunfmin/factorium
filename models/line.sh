#!/bin/sh
# A checkout's line's model (models/Line.tla), beyond what the gate checks
# (`truefit model`, and the kept trace):
#
#   models/line.sh trace [<scenario> [<file>]]
#       Runs one scenario (default: the lost answer's, whose run is kept in
#       models/line-run.jsonl) with the line's records kept as a trace
#       (crates/factorium/tests/scenarios/fixture/trace.rs), and checks it
#       against the model: `allowed`, or the first step it does not allow.
#   models/line.sh broken
#       Checks each broken variant (BrokenVariants in the model) under the
#       config that checks its property: each must fail on that property.
#   models/line.sh finding
#       Checks what the code breaks: two daemons that raced their start
#       land at once (LineTwoFinding.cfg); a daemon that dies between a
#       worker's end and its issue's close leaves it open (LineEnd.cfg).
set -eu
cd "$(dirname "$0")/.."

. models/tlc.sh

case "${1:-}" in
  trace)
    scenario=${2:-line::a_landing_whose_push_answer_is_lost_lands_again_and_closes_its_issue}
    file=${3:-models/line-run.jsonl}
    FACTORIUM_LINE_TRACE="$PWD/$file" cargo test -q -p factorium --test scenarios -- --exact "$scenario" >&2 || true
    truefit model line-run --trace "$file" | jq '.trace | del(.output)'
    ;;
  broken)
    broken models/Line.tla models/LineTwo.cfg models/Line.cfg models/LineLive.cfg
    ;;
  finding)
    expect "$(check models/Line.tla models/LineTwoFinding.cfg none)" OneAtATime "the code (models/LineTwoFinding.cfg)"
    expect "$(check models/Line.tla models/LineEnd.cfg none)" LandedCloses "the code (models/LineEnd.cfg)"
    ;;
  *)
    sed -n '2,16p' "$0"; exit 2 ;;
esac
