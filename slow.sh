#!/bin/sh
# The slow gate (factorium.toml's `[gate] slow`): what the fast gate lacks, before a
# promotion moves the base: the release build and the models. A promotion
# runs these alone on a tip a green landing pushed, and ./gate.sh first on
# any other (docs/adr/0012-the-line-gate-landing-promotion-cut.md). Measured 2026-11-26: fast gate 563s; these
# ~250s (release dry 56s, the models 191s).
set -eu
cd "$(dirname "$0")"

# The universal build the tap signs, at Cargo.toml's version
# (docs/adr/0004), as far as it goes without publishing.
DIST=.slimfit/out/dist packaging/release.sh dry

# The models (truefit.toml: the heavy queue, models/HeavyQueue.tla; an
# issue's worktrees, models/Worktrees.tla; a checkout's line,
# models/Line.tla) hold their properties, and allow the scenario runs kept
# as their traces (`models/heavy.sh`, `models/worktrees.sh`,
# `models/line.sh` for the rest).
truefit model
truefit model heavy-run --trace models/heavy-run.jsonl
truefit model worktrees-run --trace models/worktrees-run.jsonl
truefit model line-run --trace models/line-run.jsonl
