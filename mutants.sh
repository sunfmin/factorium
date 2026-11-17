#!/bin/sh
# Mutation testing (SPEC.md §6): break a rule and a scenario or binary test
# must fail. Too slow for ./gate.sh; run it after changing a rule or a
# scenario. Arguments go to cargo-mutants: `./mutants.sh -f
# crates/domain/src/gate.rs` mutates one file; with no `-f`, every file is
# mutated, one file per run.
#
# cargo-mutants builds every mutant in its own copy of the tree, never
# this one, so no mutant can run on another's build and the tree is left
# as it was found (checked at the end). What it skips, and why, is
# .cargo/mutants.toml.
#
# In a factorium worker each run (the baseline, each file) is one heavy
# command, so landings get through between files.
set -eu
cd "$(dirname "$0")"

version=27.1.0
if [ "$(cargo mutants --version 2>/dev/null)" != "cargo-mutants $version" ]; then
  cargo install --locked "cargo-mutants@$version"
fi
picked=
for a in "$@"; do
  case "$a" in
  --in-place) echo "mutants: --in-place would mutate this tree" >&2; exit 2 ;;
  -f | --file | -f* | --file=*) picked=1 ;;
  esac
done

heavy() {
  if [ -n "${FACTORIUM_WORKER:-}" ]; then
    factorium worker exclusive -- "$@" >/dev/null
  else
    "$@"
  fi
}

# As a fresh host, as ./gate.sh runs the tests.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export CARGO_MUTANTS_JOBS="${CARGO_MUTANTS_JOBS:-6}"
before=$(git status --porcelain --untracked-files=all)
start=$(date +%s)

# The baseline: the unmutated tree passes, and how long its tests take.
# (cargo-mutants' own baseline runs the mutated crates' tests, which are
# none, so it is skipped.)

heavy sh -c 'cargo test -p factorium --test scenarios --test binary --no-run --quiet &&
  t=$(date +%s) &&
  cargo test -p factorium --test scenarios --test binary --quiet >/dev/null &&
  echo $(( $(date +%s) - t )) >target/mutants.base'
base=$(cat target/mutants.base)
rm -f target/mutants.base
# A mutant whose tests run past this hangs; a hang counts as caught.
timeout=$(( base * 5 + 30 ))
echo "mutants: baseline tests took ${base}s; each mutant's tests get ${timeout}s"

out=target/mutants.out
rm -rf "$out"
mkdir -p "$out"
failed=0
# One cargo-mutants run, its outcomes in $out/$1.
mutate() {
  dir=$1
  shift
  set +e
  # The tree is copied with its target, so the dependencies are built once;
  # the workspace's own crates rebuild in the copy.
  heavy cargo mutants --output "$out/$dir" --baseline skip --copy-target true \
    --timeout "$timeout" --build-timeout 600 \
    --test-package factorium --cargo-test-arg=--test --cargo-test-arg=scenarios \
    --cargo-test-arg=--test --cargo-test-arg=binary \
    "$@" >"$out/$dir.console" 2>&1
  code=$?
  set -e
  # cargo-mutants: 0 all caught, 2 some missed, 3 some timed out.
  case "$code" in
  0 | 2 | 3) ;;
  *) cat "$out/$dir.console" >&2; failed=$code; return ;;
  esac
  # A file with no mutants leaves no outcomes.
  [ -f "$out/$dir/mutants.out/outcomes.json" ] || return 0
  jq -r '.outcomes[] | select(.scenario != "Baseline")
    | [ (if .summary == "CaughtMutant" or .summary == "Timeout" then "caught"
         elif .summary == "MissedMutant" then "missed"
         elif .summary == "Unviable" then "unviable"
         else .summary end),
        ((([.phase_results[].duration] | add) // 0) | floor | tostring) + "s",
        .scenario.Mutant.name ]
    | join("\t")' "$out/$dir/mutants.out/outcomes.json"
}

if [ -n "$picked" ]; then
  mutate all "$@"
else
  for f in $(cargo mutants --list-files); do
    mutate "$(echo "$f" | tr / _)" -f "$f" "$@"
  done
fi

jq -s -r 'map(.) | "caught \(map(.caught + .timeout) | add), missed \(map(.missed) | add), unviable \(map(.unviable) | add)"' \
  "$out"/*/mutants.out/outcomes.json
missed=$(cat "$out"/*/mutants.out/missed.txt | wc -l | tr -d ' ')
echo "mutants: the run took $(( $(date +%s) - start ))s; details in $out"

after=$(git status --porcelain --untracked-files=all)
if [ "$before" != "$after" ]; then
  echo "mutants: the tree changed during the run:" >&2
  git status --short >&2
  exit 1
fi
[ "$failed" = 0 ] || exit "$failed"
# Exit 2 when a mutant is missed; a hang is caught.
[ "$missed" = 0 ] || exit 2
