#!/bin/sh
# One version of factorium, cut on this Mac: factorium.toml's cut step runs
# `packaging/release.sh cut $FACTORIUM_VERSION` in a worktree at the base's
# tip, then factorium tags what it left at HEAD and `brew upgrade`s the
# formula.
#
#   release.sh cut <vX.Y.Z>   set the version and commit it, build for Apple
#                             Silicon, check `factorium --version`, package,
#                             and publish: the archive to the tap's release
#                             factorium-v<X.Y.Z>, the formula to the tap's
#                             Formula/factorium.rb
#
# The archive is downloaded from the tap (sunfmin/homebrew-tap), as every
# formula there is. DIST names another directory than dist/.
set -eu
cd "$(dirname "$0")/.."
PATH="$PATH:$HOME/.cargo/bin"
export PATH

REPO=https://github.com/sunfmin/factorium
TAP=sunfmin/homebrew-tap
TARGET=aarch64-apple-darwin
DIST=${DIST:-dist}

usage() {
    echo "usage: release.sh cut <vX.Y.Z>" >&2
    exit 2
}

[ "${1:-}" = cut ] || usage
tag=${2:-}
case "$tag" in v[0-9]*.[0-9]*.[0-9]*) ;; *) usage ;; esac
version=${tag#v}

# The version, in Cargo.toml and Cargo.lock, committed: the tag goes on it.
sed -i.bak "s/^version = \".*\"\$/version = \"$version\"/" Cargo.toml
rm Cargo.toml.bak
cargo update --quiet --workspace
said=$(cargo pkgid -p factorium | sed 's/.*[#@]//')
[ "$said" = "$version" ] || {
    echo "release: Cargo.toml says $said, not $version" >&2
    exit 1
}
if ! git diff --quiet; then
    git commit --quiet -am "factorium $version"
fi

cargo build --release --locked -p factorium --target "$TARGET"
built=target/$TARGET/release/factorium
said=$("$built" --version)
[ "$said" = "factorium $version" ] || {
    echo "release: $built --version says '$said', not 'factorium $version'" >&2
    exit 1
}

mkdir -p "$DIST"
archive=factorium-$version-macos-arm64.tar.gz
tar -czf "$DIST/$archive" -C "$(dirname "$built")" factorium
sha=$(shasum -a 256 "$DIST/$archive" | cut -d' ' -f1)
release=factorium-v$version
formula=$DIST/factorium.rb
cat > "$formula" <<FORMULA
class Factorium < Formula
  desc "Runs AI coding agents on one machine while nobody watches"
  homepage "$REPO"
  url "https://github.com/$TAP/releases/download/$release/$archive"
  sha256 "$sha"
  version "$version"

  depends_on :macos
  depends_on arch: :arm64

  def install
    bin.install "factorium"
  end

  def caveats
    <<~EOS
      factorium starts its sessions through Orca:
        brew install --cask stablyai/orca/orca
      Engine accounts are read from mytokens.
    EOS
  end

  test do
    assert_equal "factorium #{version}", shell_output("#{bin}/factorium --version").strip
  end
end
FORMULA
ruby -c "$formula" >/dev/null

# A release that is there already takes the archive again: a cut that
# failed half way is run a second time.
if gh release view "$release" -R "$TAP" >/dev/null 2>&1; then
    gh release upload "$release" -R "$TAP" --clobber "$DIST/$archive"
else
    gh release create "$release" -R "$TAP" --title "factorium $version" \
        --notes "The binary the formula \`factorium\` installs." "$DIST/$archive"
fi
tap=$(mktemp -d)
gh repo clone "$TAP" "$tap" -- --quiet --depth 1
cp "$formula" "$tap/Formula/factorium.rb"
git -C "$tap" add Formula/factorium.rb
if git -C "$tap" diff --cached --quiet; then
    echo "release: the tap's formula is factorium $version already"
else
    git -C "$tap" commit --quiet -m "factorium $version"
    git -C "$tap" push --quiet origin HEAD
fi
rm -rf "$tap"
