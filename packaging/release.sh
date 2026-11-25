#!/bin/sh
# One version of factorium (docs/adr/0004): this repo builds it;
# sunfmin/homebrew-tap's release workflow signs, notarizes and publishes it.
# factorium.toml's cut step runs `packaging/release.sh cut $FACTORIUM_VERSION`
# in a clone at the base's tip, then factorium tags what it left at HEAD and
# `brew upgrade`s the formula.
#
#   release.sh build <vX.Y.Z> <outdir>
#                             build aarch64 and x86_64 at the version
#                             Cargo.toml says (it must be the tag's), check
#                             `factorium --version` says it, `lipo -create`
#                             them: <outdir> holds the universal `factorium`
#                             and the formula `factorium.rb`, its url and
#                             sha256 left as @URL@ and @SHA256@ for the tap
#                             to fill in. Signs nothing, needs no secret.
#   release.sh cut <vX.Y.Z>   set the version in Cargo.toml and commit it,
#                             build, push the tag to origin, run the tap's
#                             release.yml on it and wait for that run, bounded
#                             (RELEASE_WAIT_LIMIT seconds from the cut's start,
#                             default 1700), a failing `gh` asked again until
#                             then; a cut that ends before its run does
#                             cancels that run; publishes nothing from this host
#   release.sh dry            what the gate runs: build at the version
#                             Cargo.toml says, check the formula, publish
#                             nothing
#
# DIST names another directory than dist/.
set -eu
cd "$(dirname "$0")/.."
PATH="$PATH:$HOME/.cargo/bin"
export PATH

REPO=https://github.com/sunfmin/factorium
TAP=sunfmin/homebrew-tap
TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
DIST=${DIST:-dist}

usage() {
    echo "usage: release.sh build <vX.Y.Z> <outdir> | cut <vX.Y.Z> | dry" >&2
    exit 2
}

# check_tag <tag>: a version tag vX.Y.Z, or usage.
check_tag() {
    case "$1" in
    v*[!0-9.]* | v*..* | v.* | v*.) usage ;;
    v[0-9]*.[0-9]*.[0-9]*) ;;
    *) usage ;;
    esac
    case "$1" in v*.*.*.*) usage ;; esac
}

# cargo_version: the version Cargo.toml (and so the binary) says.
cargo_version() {
    cargo pkgid -p factorium | sed 's/.*[#@]//'
}

# build <version> <outdir>: the universal binary and the formula in <outdir>.
build() {
    said=$(cargo_version)
    [ "$said" = "$1" ] || {
        echo "release: Cargo.toml says $said, not $1" >&2
        exit 1
    }
    rustup target add $TARGETS >/dev/null 2>&1 || true
    slices=
    for target in $TARGETS; do
        cargo build --release --locked --quiet -p factorium --target "$target"
        slices="$slices target/$target/release/factorium"
    done
    rm -rf "$2"
    mkdir -p "$2"
    # shellcheck disable=SC2086 # the slices are paths without spaces
    lipo -create -output "$2/factorium" $slices
    archs=$(lipo -archs "$2/factorium")
    [ "$archs" = "x86_64 arm64" ] || {
        echo "release: $2/factorium holds '$archs', not 'x86_64 arm64'" >&2
        exit 1
    }
    said=$("$2/factorium" --version)
    [ "$said" = "factorium $1" ] || {
        echo "release: $2/factorium --version says '$said', not 'factorium $1'" >&2
        exit 1
    }
    cat >"$2/factorium.rb" <<FORMULA
class Factorium < Formula
  desc "Runs AI coding agents on one machine while nobody watches"
  homepage "$REPO"
  url "@URL@"
  sha256 "@SHA256@"
  version "$1"

  depends_on :macos

  def install
    bin.install "factorium"
  end

  def caveats
    <<~EOS
      factorium starts its sessions through Orca:
        brew install --cask stablyai/orca/orca
      Engine accounts are stablehand's:
        brew install sunfmin/tap/stablehand
    EOS
  end

  test do
    assert_equal "factorium #{version}", shell_output("#{bin}/factorium --version").strip
  end
end
FORMULA
    ruby -c "$2/factorium.rb" >/dev/null
}

# push_tag <tag>: origin has <tag> on HEAD. factorium's own tag step tags
# HEAD after the cut and fails on a tag origin already holds, so a tag this
# cut pushed is taken away again when the cut ends, however it ends.
push_tag() {
    head=$(git rev-parse HEAD)
    # An annotated tag's peeled line (^{}) comes after it: the last line is the commit.
    there=$(git ls-remote origin "refs/tags/$1" "refs/tags/$1^{}" | tail -n1 | cut -f1)
    if [ -z "$there" ]; then
        git push --quiet origin "$head:refs/tags/$1"
        trap untag EXIT
    elif [ "$there" != "$head" ]; then
        echo "release: origin's $1 is on $there, not HEAD ($head)" >&2
        exit 1
    fi
}

# untag: the cut's end, however it ends, once it pushed the tag. A run it
# dispatched and did not see finish is cancelled first: without the tag it
# could only fail.
untag() {
    if [ -n "${dispatched:-}" ] && [ -z "${finished:-}" ]; then
        [ -n "$run" ] || run=$(latest_run "$tag") || run=
        if [ -n "$run" ] && [ "$run" != "$before" ]; then
            echo "release: cancelling run $run of factorium $tag" >&2
            gh run cancel "$run" -R "$TAP" >&2 || true
        fi
    fi
    git push --quiet origin ":refs/tags/$tag"
}

# latest_run <tag>: the id of the newest run of the tap's release.yml on
# factorium <tag>, or nothing.
latest_run() {
    gh run list -R "$TAP" --workflow release.yml --event workflow_dispatch --limit 50 \
        --json databaseId,displayTitle \
        --jq "[.[] | select(.displayTitle == \"factorium $1\") | .databaseId] | max // empty"
}

case "${1:-}" in
build)
    [ $# -eq 3 ] || usage
    check_tag "$2"
    build "${2#v}" "$3"
    echo "release: built factorium ${2#v}, universal, in $3"
    exit 0
    ;;
dry)
    [ $# -eq 1 ] || usage
    build "$(cargo_version)" "$DIST"
    echo "release: dry ok (factorium $(cargo_version), universal, $DIST)"
    exit 0
    ;;
cut) [ $# -eq 2 ] || usage ;;
*) usage ;;
esac
check_tag "$2"
tag=$2
version=${tag#v}
started=$(date +%s)
# Inside factorium.toml's [cut] limit and the heavy queue's hold bound, both 1800s.
limit=${RELEASE_WAIT_LIMIT:-1700}

# The version, in Cargo.toml and Cargo.lock, committed: the tag goes on it,
# and the tap's build at the tag reads it there.
sed -i.bak "s/^version = \".*\"\$/version = \"$version\"/" Cargo.toml
rm Cargo.toml.bak
cargo update --quiet --workspace
if ! git diff --quiet; then
    git commit --quiet -am "factorium $version"
fi

# Built here first: a tag goes out only on a commit that builds at its version.
build "$version" "$DIST"
before=$(latest_run "$tag")
push_tag "$tag"
run=
gh workflow run release.yml -R "$TAP" -f repo=factorium -f "tag=$tag"
dispatched=1

# A failing `gh` (GitHub answers HTTP 500 at times) is asked again until the
# limit: the run it asks about goes on regardless.
while :; do
    found=$(latest_run "$tag") || found=
    [ -n "$found" ] && [ "$found" != "$before" ] && run=$found && break
    [ $(($(date +%s) - started)) -lt "$limit" ] || {
        echo "release: no release.yml run of factorium $tag in $TAP after ${limit}s" >&2
        exit 1
    }
    sleep 5
done
echo "release: waiting for https://github.com/$TAP/actions/runs/$run"
while :; do
    state=$(gh run view "$run" -R "$TAP" --json status,conclusion --jq '.status + " " + .conclusion') ||
        state="unknown (gh run view failed)"
    case "$state" in
    "completed success") finished=1 && break ;;
    completed*)
        finished=1
        echo "release: run $run of factorium $tag ended '$state'" >&2
        gh run view "$run" -R "$TAP" --log-failed >&2 || true
        exit 1
        ;;
    esac
    [ $(($(date +%s) - started)) -lt "$limit" ] || {
        echo "release: run $run of factorium $tag still '$state' after ${limit}s: gh run view $run -R $TAP" >&2
        exit 1
    }
    sleep 15
done
echo "release: factorium $tag is published ($(($(date +%s) - started))s)"
