#!/bin/sh
# Mutation testing (SPEC.md §6): break a rule and a scenario or binary test
# must fail. Too slow for ./gate.sh; run it after changing a rule or a
# scenario. Arguments go to cargo-mutants, e.g. `./mutants.sh -f
# crates/domain/src/gate.rs` or `./mutants.sh -j 8`.
#
# cargo-mutants builds every mutant in its own copy of the tree, never
# this one, so no mutant can run on another's build and the tree is left
# as it was found (checked at the end). What it skips, and why, is
# .cargo/mutants.toml.
set -eu
cd "$(dirname "$0")"

version=27.1.0
if [ "$(cargo mutants --version 2>/dev/null)" != "cargo-mutants $version" ]; then
  cargo install --locked "cargo-mutants@$version"
fi
for a in "$@"; do
  case "$a" in
  --in-place) echo "mutants: --in-place would mutate this tree" >&2; exit 2 ;;
  esac
done

# As a fresh host, as ./gate.sh runs the tests.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
tests="--test scenarios --test binary"
before=$(git status --porcelain --untracked-files=all)
start=$(date +%s)

# The baseline: the unmutated tree passes, and how long its tests take.
# (cargo-mutants' own baseline runs the mutated crates' tests, which are
# none, so it is skipped.)
# shellcheck disable=SC2086
cargo test -p factorium $tests --no-run --quiet
t0=$(date +%s)
# shellcheck disable=SC2086
cargo test -p factorium $tests --quiet >/dev/null
base=$(( $(date +%s) - t0 ))
# A mutant whose tests run past this hangs; a hang counts as caught.
timeout=$(( base * 5 + 30 ))
echo "mutants: baseline tests took ${base}s; each mutant's tests get ${timeout}s"

out=target/mutants.out
set +e
# shellcheck disable=SC2086
cargo mutants --output target --baseline skip \
  --timeout "$timeout" --build-timeout 600 \
  --test-package factorium --cargo-test-arg=--test --cargo-test-arg=scenarios \
  --cargo-test-arg=--test --cargo-test-arg=binary \
  "$@" >"$out.console" 2>&1
code=$?
set -e

if [ -f "$out/outcomes.json" ]; then
  jq -r '.outcomes[] | select(.scenario != "Baseline")
    | [ (if .summary == "CaughtMutant" or .summary == "Timeout" then "caught"
         elif .summary == "MissedMutant" then "missed"
         elif .summary == "Unviable" then "unviable"
         else .summary end),
        ((([.phase_results[].duration] | add) // 0) | floor | tostring) + "s",
        .scenario.Mutant.name ]
    | join("\t")' "$out/outcomes.json"
  jq -r '"caught \(.caught + .timeout), missed \(.missed), unviable \(.unviable)"' \
    "$out/outcomes.json"
fi
echo "mutants: the run took $(( $(date +%s) - start ))s; details in $out"

after=$(git status --porcelain --untracked-files=all)
if [ "$before" != "$after" ]; then
  echo "mutants: the tree changed during the run:" >&2
  git status --short >&2
  exit 1
fi
# Exit 2 when a mutant is missed; a timeout is caught. (cargo-mutants
# says 3 for a timeout even when it also missed some.)
case "$code" in
0 | 2 | 3) [ "$(jq .missed "$out/outcomes.json")" = 0 ] || exit 2 ;;
*) cat "$out.console" >&2; exit "$code" ;;
esac
