#!/bin/sh
# Keep the host's launch command opaque and append DSH's arguments as words.
# The native loader dependency, not this wrapper, enforces guard activation.
set -eu
launch=$1
kept=$2
role=$3
worktree=$4
profile=$5
for file in factorium.mjs factorium.patch.yml; do
    if [ ! -r "$kept/$file" ]; then
        echo "factorium: DSH guard file $file is missing; refusing session" >&2
        exit 1
    fi
done
set --
if [ "$profile" = kept ]; then
    set -- --patch "$kept/sandbox.patch.yml"
fi
if [ -f "$role/.dsh/cordis.patch.yml" ]; then
    set -- "$@" --patch "$role/.dsh/cordis.patch.yml"
fi
set -- "$@" --patch "$kept/factorium.patch.yml" "$worktree"
exec sh -c "$launch \"\$@\"" factorium-dsh "$@"
