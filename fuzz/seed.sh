#!/bin/sh
# fuzz/seed.sh OUT [EXPORT...] — writes the fuzz targets' seed corpus into OUT/json, OUT/segment
# and OUT/import (roll-k2wy.14.4), OUT being outside this checkout.
#
# The corpus is synthetic: what a bead (BEAD, else target/debug/bead: `cargo build --package
# beadroll`) writes in a scratch store, checkout and bare remote under a temporary directory,
# importing infra/seed.jsonl and editing with awkward text — its journal segments, attempt files,
# small JSON files and export, cut into small inputs — and infra/seed.jsonl itself. Nothing here
# touches a real store or a real remote: bead runs with only PATH and the scratch HOME and
# BEAD_HOME in its environment.
#
# Each EXPORT, a Beads export of a real tracker, is read where it is and cut into OUT as well, which
# is why OUT may not be inside this checkout: nothing derived from a real export may land where it
# could be checked in. libFuzzer writes what it finds into the first corpus directory it is given,
# so run the targets on OUT too (fuzz/README.md).
set -eu

if [ $# -lt 1 ]; then
    echo "usage: fuzz/seed.sh OUT [EXPORT...]" >&2
    exit 2
fi
out=$1
shift
repo=$(cd "$(dirname "$0")/.." && pwd -P)
made=
[ -d "$out" ] || made=yes
mkdir -p "$out"
out=$(cd "$out" && pwd -P)
case "$out/" in
    "$repo"/*)
        [ -z "$made" ] || rmdir "$out"
        echo "fuzz/seed.sh: $out is inside the checkout $repo; give a directory outside it" >&2
        exit 2
        ;;
esac
bead=${BEAD:-$repo/target/debug/bead}
if [ ! -x "$bead" ]; then
    echo "fuzz/seed.sh: no bead at $bead: run cargo build --package beadroll, or set BEAD" >&2
    exit 2
fi
mkdir -p "$out/json" "$out/segment" "$out/import"

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/home" "$scratch/store"
printf '[user]\n\tname = seed\n\temail = seed@example.invalid\n[init]\n\tdefaultBranch = main\n' \
    > "$scratch/home/.gitconfig"
# bead and git see nothing of this shell's environment but PATH and the scratch homes
run() {
    env -i PATH="$PATH" HOME="$scratch/home" BEAD_HOME="$scratch/store" "$@"
}
run git init -q --bare "$scratch/origin.git"
run git clone -q "$scratch/origin.git" "$scratch/work" 2>/dev/null
run git -C "$scratch/work" commit -q --allow-empty -m seed
run git -C "$scratch/work" push -q origin main
bead() {
    (cd "$scratch/work" && run "$bead" "$@") > /dev/null
}
# a command's key, from its --json answer
key() {
    (cd "$scratch/work" && run "$bead" "$@" --json) | sed -n 's/^{"id": "\([^"]*\)".*/\1/p'
}
bead init --prefix fz --import "$repo/infra/seed.jsonl"
one=$(key create 'Quotes " and \ backslashes' -d "$(printf 'tab\there\nnew line, \342\200\250 and \360\237\230\200')")
two=$(key create 'Ünïcödé — ✓ 中文 עברית' -p 0 -t bug -l alpha -l 'β' --parent "$one")
three=$(key create 'Loose end' --acceptance "$(printf 'all \001 controls \037 and DEL \177')")
bead comments add "$one" "$(printf 'a comment\nover two lines \033[1m with an escape')"
bead dep add "$two" "$three"
bead update "$one" --notes 'notes: {"json": [1, 2.5, -0.0, 1e300]}' --add-label urgent
bead claim "$one"
bead release "$one"
bead claim "$two"
bead close "$two" -r 'done, "quoted"'
bead reopen "$two"
bead delete "$three" -r 'not needed'
bead sync
bead export -o "$scratch/export.jsonl"

# Small inputs: whole files of a few lines, cut files of more, and single lines.
cut_into() { # FILE DIR NAME LINES
    if [ "$(wc -l < "$1")" -le "$4" ]; then
        cp "$1" "$2/$3"
    else
        split -l "$4" -a 4 "$1" "$2/$3-"
    fi
}
n=0
for file in $(find "$scratch/store" -type f -name '*.jsonl' | sort); do
    n=$((n + 1))
    cut_into "$file" "$out/segment" "segment-$n" 16
    split -l 1 -a 5 "$file" "$out/json/segment-$n-"
done
for file in $(find "$scratch/store" -type f -name '*.json' | sort); do
    n=$((n + 1))
    cp "$file" "$out/segment/file-$n"
    cp "$file" "$out/json/file-$n"
done
cut_into "$scratch/export.jsonl" "$out/import" export 8
split -l 1 -a 5 "$scratch/export.jsonl" "$out/json/export-"
cut_into "$repo/infra/seed.jsonl" "$out/import" infra-seed 8

# Real exports, read where they are.
n=0
for export in "$@"; do
    n=$((n + 1))
    cut_into "$export" "$out/import" "real-$n" 8
    split -l 1 -a 6 "$export" "$out/json/real-$n-"
done
for target in json segment import; do
    echo "$out/$target: $(find "$out/$target" -type f | wc -l | tr -d ' ') inputs"
done
