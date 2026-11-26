#!/bin/sh
# Checks `release.sh cut` without building or publishing (gate.sh runs it):
# cargo, lipo, rustup, sleep and gh are faked, origin is a bare repo here. The
# cut commits the version, dispatches the tap's workflow, waits for that run
# through failing `gh` calls and fails with its log when it fails; a tag it
# pushed is gone from origin when it ends, after the run it left unfinished
# is cancelled, and a tag origin already had on HEAD stays.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# The fake cargo builds under ./target, where the cut reads it with no
# CARGO_TARGET_DIR: the one a gate runs with names the real cache.
unset CARGO_TARGET_DIR
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin" "$t/repo/packaging"
cp "$here/release.sh" "$t/repo/packaging/"
printf '[workspace.package]\nversion = "0.1.0"\n' >"$t/repo/Cargo.toml"
# cargo: pkgid reads Cargo.toml's version; build leaves a binary that says it.
cat >"$t/bin/cargo" <<'EOF'
#!/bin/sh
v=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml)
case "$1" in
pkgid) echo "path+file:///x#factorium@$v" ;;
build)
    for a; do case "$prev" in --target) target=$a ;; esac; prev=$a; done
    mkdir -p "target/$target/release"
    printf '#!/bin/sh\necho "factorium %s"\n' "$v" >"target/$target/release/factorium"
    chmod +x "target/$target/release/factorium"
    ;;
esac
EOF
cat >"$t/bin/lipo" <<'EOF'
#!/bin/sh
case "$1" in -archs) echo "x86_64 arm64" ;; -create) cp "$4" "$3" ;; esac
EOF
printf '#!/bin/sh\n' >"$t/bin/rustup"
printf '#!/bin/sh\nexec /bin/sleep 0.1\n' >"$t/bin/sleep"
# gh: `workflow run` makes run <n+1>, which `run view` reports as $RUN_ENDS
# after failing its first $VIEW_FAILS times with HTTP 500. `run cancel` is
# recorded with what origin's tag is on then.
cat >"$t/bin/gh" <<'EOF'
#!/bin/sh
s=$FAKE_GH
case "$1 $2" in
"workflow run") echo "$*" >>"$s.calls"; echo $(( $(cat "$s.n") + 1 )) >"$s.n" ;;
"run list") n=$(cat "$s.n"); [ "$n" = 0 ] || echo "$n" ;;
"run view")
    case "$*" in *--log-failed*) echo "the run's failed log"; exit ;; esac
    echo x >>"$s.views"
    if [ "$(wc -l <"$s.views")" -le "${VIEW_FAILS:-0}" ]; then
        echo "HTTP 500: Internal Server Error (https://api.github.com/)" >&2
        exit 1
    fi
    echo "$RUN_ENDS"
    ;;
"run cancel") echo "cancel $3 tag=$(git ls-remote "$FAKE_ORIGIN" refs/tags/v1.2.3 | cut -f1)" >>"$s.calls" ;;
esac
EOF
chmod +x "$t/bin/"*
git init -q --bare "$t/origin.git"
cd "$t/repo"
git init -q && git config user.email t@t && git config user.name t
printf 'dist/\ntarget/\n' >.gitignore
git add . && git commit -q -m one
git remote add origin "$t/origin.git"

# run_cut <what run view ends with> [<its HTTP 500s first>] [<wait limit>]
run_cut() {
    rm -f "$t/gh.views" "$t/gh.calls"
    PATH="$t/bin:$PATH" FAKE_GH="$t/gh" FAKE_ORIGIN="$t/origin.git" \
        RELEASE_WAIT_LIMIT="${3:-20}" RUN_ENDS="$1" VIEW_FAILS="${2:-0}" \
        packaging/release.sh cut v1.2.3 >"$t/out" 2>&1
}
fail() { echo "release-test: $*" >&2; cat "$t/out" >&2; exit 1; }
tagged() { git ls-remote "$t/origin.git" refs/tags/v1.2.3 | cut -f1; }

echo 0 >"$t/gh.n"
run_cut "completed success" || fail "a run that succeeds fails the cut"
grep -qx "workflow run release.yml -R sunfmin/homebrew-tap -f repo=factorium -f tag=v1.2.3" "$t/gh.calls" \
    || fail "the cut ran '$(cat "$t/gh.calls")'"
[ "$(git log -1 --format=%s)" = "factorium 1.2.3" ] || fail "the cut did not commit the version"
grep -q '^version = "1.2.3"$' Cargo.toml || fail "Cargo.toml does not say 1.2.3"
grep -q 'url "@URL@"' dist/factorium.rb && grep -q 'sha256 "@SHA256@"' dist/factorium.rb \
    || fail "the formula has no @URL@ and @SHA256@ for the tap"
! grep -q 'depends_on arch' dist/factorium.rb || fail "the formula still holds to one arch"
[ -z "$(tagged)" ] || fail "the tag the cut pushed is still on origin"
[ -z "$(git tag)" ] || fail "the cut left a tag here: factorium's own tag step would fail"

if run_cut "completed failure"; then fail "a run that fails passes the cut"; fi
grep -q "the run's failed log" "$t/out" || fail "a failed run's log is not printed"
[ -z "$(tagged)" ] || fail "the tag of a failed cut is still on origin"
! grep -q '^cancel' "$t/gh.calls" || fail "the cut cancelled a run that had ended"

# GitHub answering HTTP 500 while the run goes on ends nothing.
run_cut "completed success" 2 || fail "two HTTP 500s from gh run view fail the cut"
[ "$(wc -l <"$t/gh.views")" -eq 3 ] || fail "gh run view was asked $(wc -l <"$t/gh.views") times, not 3"
! grep -q '^cancel' "$t/gh.calls" || fail "the cut cancelled a run that succeeded"

# A run still queued at the limit is cancelled while its tag is there, then
# the tag goes.
if run_cut "queued " 0 1; then fail "a run queued past the limit passes the cut"; fi
grep -q "still 'queued '" "$t/out" || fail "the cut does not say the run is still queued"
grep -Eq "^cancel [0-9]+ tag=[0-9a-f]{40}$" "$t/gh.calls" \
    || fail "the run was not cancelled before the tag went: '$(cat "$t/gh.calls")'"
[ -z "$(tagged)" ] || fail "the tag of a cut past its limit is still on origin"

# factorium's tag step, after the cut: it tags HEAD and pushes the tag. Then a
# cut run again finds it there, and leaves it.
git tag -a v1.2.3 -m v1.2.3
git push -q origin refs/tags/v1.2.3
git tag -d v1.2.3 >/dev/null
run_cut "completed success" || fail "a tag origin has on HEAD fails the cut"
[ -n "$(tagged)" ] || fail "the cut took away a tag it did not push"

git commit -q --allow-empty -m two
if run_cut "completed success"; then fail "a tag origin has on another commit passes the cut"; fi
grep -q "not HEAD" "$t/out" || fail "the cut does not say the tag is on another commit"

# The tap's build at a tag Cargo.toml does not say fails.
if PATH="$t/bin:$PATH" packaging/release.sh build v9.9.9 "$t/o" >"$t/out" 2>&1; then
    fail "a build at a version Cargo.toml does not say passes"
fi
for bad in 1.2.3 v1.2 v1.2.3.4 v1..2 v1.2.x; do
    if PATH="$t/bin:$PATH" packaging/release.sh build "$bad" "$t/o" >"$t/out" 2>&1; then
        fail "build takes the tag '$bad'"
    fi
done
echo "release-test: ok"
