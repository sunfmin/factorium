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

# check <config> <bug>: `truefit model` on the model under <config> with
# Bug = <bug>, in a scratch directory; prints its report.
check() {
  dir=$(mktemp -d)
  cp models/Worktrees.tla "$dir/"
  sed "s/Bug = \"none\"/Bug = \"$2\"/" "$1" > "$dir/c.cfg"
  printf '[[model]]\nname = "v"\nspec = "Worktrees.tla"\nconfig = "c.cfg"\nlimit = 120\ncommit = "%s"\ncovers = [{ action = "Next", file = "Worktrees.tla", function = "Next" }]\n' \
    "$(git rev-parse HEAD)" > "$dir/truefit.toml"
  (cd "$dir" && git init -q && truefit model || true)
  rm -rf "$dir"
}

# expect <report> <property> <what>: the report fails on <property>.
expect() {
  error=$(printf '%s' "$1" | jq -r '.models[0].error // ""')
  steps=$(printf '%s' "$1" | jq -r '[.models[0].behaviour[]?.action | sub(" line .*"; "")] | join(" > ")')
  case "$error" in
    *" $2 is violated."*) echo "fails  $3: $error ($steps)" ;;
    *) echo "PASSES $3: $(printf '%s' "$1" | jq -c '.models[0] | {result, error}')"; return 1 ;;
  esac
}

case "${1:-}" in
  trace)
    scenario=${2:-kept::a_worktree_s_life_from_start_to_close_is_a_behaviour_of_its_model}
    file=${3:-models/worktrees-run.jsonl}
    FACTORIUM_KEPT_TRACE="$PWD/$file" cargo test -q -p factorium --test scenarios -- --exact "$scenario" >&2 || true
    truefit model worktrees-run --trace "$file" | jq '.trace | del(.output)'
    ;;
  broken)
    failed=0
    for v in $(sed -n '/^BrokenVariants/,/^]/s/^ *\([a-z_]*\) *|-> "\([A-Za-z]*\)".*/\1:\2/p' models/Worktrees.tla); do
      bug=${v%%:*} property=${v#*:}
      expect "$(check models/Worktrees.cfg "$bug")" "$property" "$bug" || failed=1
    done
    exit $failed
    ;;
  finding)
    failed=0
    expect "$(check models/WorktreesRace.cfg none)" NoStartOverUnderWorker "the code, racing (WorktreesRace.cfg)" || failed=1
    expect "$(check models/WorktreesDeath.cfg none)" DoctorFinds "the code, dying mid-start (WorktreesDeath.cfg)" || failed=1
    exit $failed
    ;;
  *)
    sed -n '2,17p' "$0"; exit 2 ;;
esac
