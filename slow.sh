#!/bin/sh
# The slow gate (SPEC.md §5, §6): on the proving tip, before a promotion
# moves the base. The fast gate again (every landing passed it on its own
# merge; this holds the tip against .slimfit/baseline.json), then what no
# single landing needs and the base must hold: the release build and the
# models. Measured 2026-11-26: fast gate 812s before this split, 563s after
# less these; these add ~250s (release dry 56s, the models 191s).
set -eu
cd "$(dirname "$0")"

./gate.sh

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
