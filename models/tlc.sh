# What models/heavy.sh and models/line.sh share: `. models/tlc.sh`.

# check <spec> <config> <bug> [<property>]: `truefit model` on <spec>
# under <config> with Bug = <bug> (and, given <property>, that one alone
# checked, beside TypeOK), in a scratch directory; prints its report.
check() {
  dir=$(mktemp -d)
  cp "$1" "$dir/"
  sed "s/Bug = \"none\"/Bug = \"$3\"/" "$2" > "$dir/c.cfg"
  if [ -n "${4:-}" ]; then
    if grep -q "^PROPERTY .*\b$4\b" "$2"; then
      sed -i '' -e "s/^PROPERTY .*/PROPERTY $4/" -e "s/^INVARIANT .*/INVARIANT TypeOK/" "$dir/c.cfg"
    else
      sed -i '' -e "s/^INVARIANT .*/INVARIANT TypeOK $4/" -e "/^PROPERTY /d" "$dir/c.cfg"
    fi
  fi
  printf '[[model]]\nname = "v"\nspec = "%s"\nconfig = "c.cfg"\nlimit = 300\ncommit = "%s"\ncovers = [{ action = "Next", file = "%s", function = "Next" }]\n' \
    "$(basename "$1")" "$(git rev-parse HEAD)" "$(basename "$1")" > "$dir/truefit.toml"
  (cd "$dir" && git init -q && truefit model || true)
  rm -rf "$dir"
}

# expect <report> <property> <what>: the report fails on <property>.
expect() {
  error=$(printf '%s' "$1" | jq -r '.models[0].error // ""')
  steps=$(printf '%s' "$1" | jq -r '[.models[0].behaviour[]?.action] | join(" > ")' | sed 's/ line [0-9]*, col [0-9]* to line [0-9]*, col [0-9]* of module [A-Za-z]*//g')
  case "$error" in
    *" $2 is violated."*|*"Temporal properties were violated."*) echo "fails  $3: $error ($steps)" ;;
    *) echo "PASSES $3: $(printf '%s' "$1" | jq -c '.models[0] | {result, error}')"; return 1 ;;
  esac
}

# broken <spec> <config>...: each variant of <spec>'s BrokenVariants,
# checked under the first <config> that checks its property, fails on it.
broken() {
  spec=$1; shift
  failed=0
  for v in $(sed -n '/^BrokenVariants/,/^]/s/^ *\([a-z_]*\) *|-> "\([A-Za-z]*\)".*/\1:\2/p' "$spec"); do
    bug=${v%%:*} property=${v#*:}
    config=$(grep -l "^\(PROPERTY\|INVARIANT\) .*\b$property\b" "$@" | head -n1)
    expect "$(check "$spec" "$config" "$bug" "$property")" "$property" "$bug ($config)" || failed=1
  done
  return $failed
}
