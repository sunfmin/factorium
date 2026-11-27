# 4. Cuts are universal, signed and notarized by the tap's workflow

Status: accepted, after slimfit's
[ADR 0013](https://github.com/sunfmin/slimfit/blob/main/docs/adr/0013-releases-are-signed-and-notarized-by-the-taps-workflow.md)

## Context

Before this, a cut built on the cutting host, for aarch64 only, and
published from there. The binary was only ad hoc signed; Intel Macs could
not run it and Gatekeeper rejected a copy downloaded in a browser.

The owner, 2026-11-22: 「把所有的工具都变成 universal。」「把公开的工具全部在
GitHub CI 上签。」「这个密钥的环境变量可以统一配置到账号上面吗？」 A personal
account has no account-wide secrets, so their one copy is in
sunfmin/homebrew-tap.

## Decision

Invariant, as slimfit's: **the Developer ID identity and the notary key live
in sunfmin/homebrew-tap only, and only its `release` workflow signs.**
factorium holds no signing secret and no release workflow.

- **factorium builds** the universal binary and its formula
  (`packaging/release.sh build`). Its version is Cargo.toml's: the cut
  commits it and the tag goes on that commit, so
  `build` at the tag only checks Cargo.toml says the tag's version.
- **The cut waits** for the tap's run it dispatched, bounded; a failed run
  fails the cut, and the tag it pushed is taken away when it ends.
- **factorium cuts itself.** The daemon running the cut is the factorium
  being released, and the step is one heavy job inside `[cut] limit` and
  the heavy queue's hold bound (factorium.toml says the numbers).
  Only after the step passes does the daemon tag, `brew upgrade` and prove
  the new binary before switching to it. A cut that fails leaves the old
  daemon running.

## Consequences

- A cut takes the CI run's minutes, and holds the cut's heavy turn while it
  waits.
- A cut needs no tap token on the host: only `gh` logged in as a user who
  may push factorium and dispatch the tap's workflow.
