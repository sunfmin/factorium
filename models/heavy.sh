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

# check <config> <bug>: `truefit model` on the model under <config> with
# Bug = <bug>, in a scratch directory; prints its report.
check() {
  dir=$(mktemp -d)
  cp models/HeavyQueue.tla "$dir/"
  sed "s/Bug = \"none\"/Bug = \"$2\"/" "$1" > "$dir/c.cfg"
  printf '[[model]]\nname = "v"\nspec = "HeavyQueue.tla"\nconfig = "c.cfg"\nlimit = 120\ncommit = "%s"\ncovers = [{ action = "Next", file = "HeavyQueue.tla", function = "Next" }]\n' \
    "$(git rev-parse HEAD)" > "$dir/truefit.toml"
  (cd "$dir" && git init -q && truefit model || true)
  rm -rf "$dir"
}

# expect <report> <property> <what>: the report fails on <property>.
expect() {
  error=$(printf '%s' "$1" | jq -r '.models[0].error // ""')
  steps=$(printf '%s' "$1" | jq -r '[.models[0].behaviour[]?.action] | join(" > ")')
  case "$error" in
    *" $2 is violated."*) echo "fails  $3: $error ($steps)" ;;
    *) echo "PASSES $3: $(printf '%s' "$1" | jq -c '.models[0] | {result, error}')"; return 1 ;;
  esac
}

case "${1:-}" in
  trace)
    scenario=${2:-queue::beside_what_runs_asks_in_one_second_start_one_per_settled_reading}
    file=${3:-models/heavy-run.jsonl}
    FACTORIUM_HEAVY_TRACE="$PWD/$file" cargo test -q -p factorium --test scenarios -- --exact "$scenario" >&2 || true
    truefit model heavy-run --trace "$file" | jq '.trace | del(.output)'
    ;;
  broken)
    failed=0
    for v in $(sed -n '/^BrokenVariants/,/^]/s/^ *\([a-z_]*\) *|-> "\([A-Za-z]*\)".*/\1:\2/p' models/HeavyQueue.tla); do
      bug=${v%%:*} property=${v#*:}
      config=$(grep -l "^\(PROPERTY\|INVARIANT\) .*\b$property\b" models/HeavyStarts.cfg models/HeavyEnds.cfg models/HeavyRules.cfg | head -n1)
      expect "$(check "$config" "$bug")" "$property" "$bug ($config)" || failed=1
    done
    exit $failed
    ;;
  *)
    sed -n '2,12p' "$0"; exit 2 ;;
esac
