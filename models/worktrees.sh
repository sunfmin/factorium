#!/bin/sh
# The worktrees' model (models/Worktrees.tla), beyond what the gate checks
# (`truefit model`, and the kept trace):
#
#   models/worktrees.sh trace [<scenario> [<file>]]
#       Runs one scenario (default: the one whose run is kept in
#       models/worktrees-run.jsonl) with its worktrees' records kept as a
#       trace (crates/factorium/tests/scenarios/fixture/kept_trace.rs), and
#       checks it against the model: `allowed`, or the first step it does
#       not allow.
#   models/worktrees.sh broken
#       Checks each broken variant (BrokenVariants in the model) under the
#       gate's config: each must fail on its property.
#   models/worktrees.sh finding
#       Checks the two configs whose behaviours the code does not yet
#       handle (WorktreesRace.cfg, WorktreesDeath.cfg): each must fail.
set -eu
cd "$(dirname "$0")/.."

. models/tlc.sh

case "${1:-}" in
  trace)
    scenario=${2:-kept::a_worktree_s_life_from_start_to_close_is_a_behaviour_of_its_model}
    file=${3:-models/worktrees-run.jsonl}
    FACTORIUM_KEPT_TRACE="$PWD/$file" cargo test -q -p factorium --test scenarios -- --exact "$scenario" >&2 || true
    truefit model worktrees-run --trace "$file" | jq '.trace | del(.output)'
    ;;
  broken)
    broken models/Worktrees.tla models/Worktrees.cfg
    ;;
  finding)
    failed=0
    expect "$(check models/Worktrees.tla models/WorktreesRace.cfg none)" NoStartOverUnderWorker "the code, racing (WorktreesRace.cfg)" || failed=1
    expect "$(check models/Worktrees.tla models/WorktreesDeath.cfg none)" DoctorFinds "the code, dying mid-start (WorktreesDeath.cfg)" || failed=1
    exit $failed
    ;;
  *)
    sed -n '2,17p' "$0"; exit 2 ;;
esac
