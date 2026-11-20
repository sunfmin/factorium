#!/bin/sh
# Each crate's source lines against its target in SPEC.md §5: a trend and a
# source of tasks, never a gate (factorium#163).
#
#   ./lines.sh          prints each crate's lines, target, and overage; exits 0
#   ./lines.sh --tasks  prints a tasks answer (as `slimfit next --all` does) with
#                       one `crate-lines` task per crate over its target or that
#                       grew at each of its last RISES landings that changed it,
#                       naming the files that grew most
#
# The targets live in SPEC.md §5's table, one row per crate, and nowhere
# else. A landing is a first-parent commit of HEAD.
set -eu
cd "$(dirname "$0")"
RISES=${RISES:-5}
WINDOW=${WINDOW:-40}

# `dir target` of each crate, read from SPEC.md §5's table.
targets() {
  sed -n 's/^| `\(factorium[a-z-]*\)` |.*| \([0-9][0-9,]*\) |$/\1 \2/p' SPEC.md |
    sed 's/^factorium-//; s/,//g'
}

# Each `.rs` file's lines under crates/$1/src at revision $2, as `lines path`.
per_file() {
  git grep -c '' "$2" -- "crates/$1/src/*.rs" 2>/dev/null |
    sed "s/^$2://" | awk -F: '{ print $2, $1 }' || true
}

# The crate's lines at revision $2.
at() {
  per_file "$1" "$2" | awk '{ s += $1 } END { print s + 0 }'
}

now() {
  find "crates/$1/src" -name '*.rs' -exec cat {} + | wc -l | tr -d ' '
}

if [ "${1:-}" != "--tasks" ]; then
  targets | while read -r crate target; do
    lines=$(now "$crate")
    over=""
    [ "$lines" -gt "$target" ] && over="  over by $((lines - target)): a trend, not a gate (./lines.sh --tasks)"
    printf '%-12s %5s / %5s lines%s\n' "$crate" "$lines" "$target" "$over"
  done
  exit 0
fi

# The tasks answer: one JSON object.
landings=$(git rev-list --first-parent -n "$WINDOW" HEAD)
first=1
printf '{"by":"lines.sh","next":"file each task","tasks":['
targets | while read -r crate target; do
  lines=$(now "$crate")
  # Its lines at each landing, newest first, keeping only the landings that
  # changed them: `rev lines`.
  last=""
  changes=$(for rev in $landings; do
    n=$(at "$crate" "$rev")
    [ "$n" = "$last" ] || echo "$rev $n"
    last=$n
  done)
  # It rose at each of its last RISES changes: the counts fall, newest first,
  # over RISES+1 entries. `from` is the landing before the first of them.
  from=$(echo "$changes" | awk -v k="$RISES" '
    NR > 1 && $2 >= prev { exit } { prev = $2; rev = $1 }
    NR == k + 1 { print rev; exit }')
  why=""
  if [ "$lines" -gt "$target" ]; then
    why="$lines lines, over its target of $target by $((lines - target))"
  fi
  if [ -n "$from" ]; then
    was=$(at "$crate" "$from")
    rose="rose at each of its last $RISES landings that changed it, from $was"
    why=${why:+$why; }$rose
  else
    from=$(echo "$landings" | tail -n 1)
  fi
  [ -n "$why" ] || continue
  # The files that grew most since `from`.
  files=$(
    { per_file "$crate" "$from" | awk '{ print -$1, $2 }'; per_file "$crate" HEAD; } |
      awk '{ d[$2] += $1 } END { for (f in d) if (d[f] > 0) print d[f], f }' |
      sort -rn | head -n 5 | awk '{ printf "%s\"%s\"", (NR > 1 ? "," : ""), $2 }'
  )
  [ "$first" = 1 ] || printf ','
  first=0
  printf '{"kind":"crate-lines","crate":"%s","lines":%s,"target":%s,' "$crate" "$lines" "$target"
  printf '"files":[%s],"summary":"crates/%s: %s",' "$files" "$crate" "$why"
  printf '"steps":"Deepen crates/%s where it grew, starting with these files: fewer, deeper modules; no budget moves. Its target in SPEC.md §5 changes only with a reason in this issue.",' "$crate"
  printf '"accept":"./gate.sh"}'
done
printf ']}\n'
