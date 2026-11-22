# 4. Cuts are universal, signed and notarized by the tap's workflow

Status: accepted (factorium#240, after slimfit#100 and its
[ADR 0013](https://github.com/sunfmin/slimfit/blob/main/docs/adr/0013-releases-are-signed-and-notarized-by-the-taps-workflow.md))

## Context

Until factorium#240 a cut (`packaging/release.sh cut`) built on the cutting
host, for aarch64 only, and published from there: the archive to the tap's
release, the formula to the tap, `depends_on arch: :arm64`. The binary was
only ad hoc signed; Intel Macs could not run it and Gatekeeper rejected a
copy downloaded in a browser.

The owner, 2026-11-22: 「把所有的工具都变成 universal。」「把公开的工具全部在
GitHub CI 上签。」「这个密钥的环境变量可以统一配置到账号上面吗？」 A personal
account has no account-wide secrets; slimfit#100 put the one copy of them in
sunfmin/homebrew-tap, whose `release` workflow signs every tool (its contract:
the tap's `.github/workflows/README.md`).

## Decision

Invariant, as slimfit's: **the Developer ID identity and the notary key live
in sunfmin/homebrew-tap only, and only its `release` workflow signs.**
factorium holds no signing secret and no release workflow.

- **factorium builds.** `packaging/release.sh build <vX.Y.Z> <outdir>`
  builds `aarch64-apple-darwin` and `x86_64-apple-darwin`, checks the
  binary says the version, `lipo -create`s them, and leaves the universal
  `factorium` and `factorium.rb` (url and sha256 `@URL@` / `@SHA256@`, no
  `depends_on arch`) in `<outdir>`. Unlike slimfit, factorium's version is
  Cargo.toml's: the cut commits it (`factorium X.Y.Z`) and the tag goes on
  that commit, so `build` at the tag only checks Cargo.toml says the tag's
  version. `dry` (in the gate) runs the same `build` at Cargo.toml's version.
- **The cut waits.** `cut` commits the version, builds here first, pushes
  the tag, runs `gh workflow run release.yml -R sunfmin/homebrew-tap -f
  repo=factorium -f tag=<tag>`, finds that run and waits for it, bounded by
  `RELEASE_WAIT_LIMIT` (1700 s from the cut's start); a failed run fails the
  cut and prints its `--log-failed`. The tag it pushed is taken away when it
  ends, however it ends: factorium's own tag step (`Git::tag_pushed`) tags
  HEAD after the steps and fails on a tag origin already holds.
- **factorium cuts itself.** The daemon running the cut is the factorium
  being released. Nothing in it changes: the step is one heavy job bounded by
  `[cut] limit` (1800 s: the host's build plus the wait) and by the heavy
  queue's hold bound (1800 s: a heavy command that holds others up that long
  is ended), so the wait stops at 1700 s. Only after the step
  passes does the daemon tag, `brew upgrade` (its tap already holds the new
  formula, published by the run it waited for) and prove the new binary
  before switching to it. A cut that fails leaves the old daemon running.

## Consequences

- The gate builds two release targets more (`dry`).
- A cut takes the CI run's minutes, and holds the cut's heavy turn while it
  waits.
- A cut no longer needs a tap token on the host: only `gh` logged in as a
  user who may push factorium and dispatch the tap's workflow.
