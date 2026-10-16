#!/bin/sh
# fuzz/seed.sh OUT [EXPORT...] — writes the fuzz targets' seed corpus into OUT/json, OUT/segment
# and OUT/import (roll-k2wy.14.4), OUT being outside this checkout.
#
# The corpus is synthetic: what a bead (BEAD, else target/debug/bead: `cargo build --package
# beadroll`) writes in scratch stores, checkouts and bare remotes under a temporary directory, and
# infra/seed.jsonl itself. Two trackers: one machine imports infra/seed.jsonl and edits with
# awkward text; on the other, two machines write the same issues concurrently, offline, and a third
# whose store is a copy of the second's writes as it, until its sync is refused. Their journal
# segments (each alone, and the machines' interleaved, so that one input holds several journals and
# a copied one's divergence), attempt files, small JSON files and export are cut into small inputs.
# Nothing here touches a real store or a real remote: bead and git run with only PATH and the
# scratch homes in their environment.
#
# Each EXPORT, a Beads export of a real tracker, is read where it is and cut into OUT as well, which
# is why OUT may not be inside this checkout: nothing derived from a real export may land where it
# could be checked in. libFuzzer writes what it finds into the first corpus directory it is given,
# and what crashes into its artifact prefix, so run the targets on OUT with OUT/artifacts as that
# prefix (fuzz/README.md).
set -eu

if [ $# -lt 1 ]; then
    echo "usage: fuzz/seed.sh OUT [EXPORT...]" >&2
    exit 2
fi
repo=$(cd "$(dirname "$0")/.." && pwd -P)
refuse() {
    echo "fuzz/seed.sh: $1" >&2
    exit 2
}
# OUT resolved before anything is made: its nearest directory that exists, resolved, and the names
# below it that do not exist yet.
case $1 in
    /*) path=$1 ;;
    *) path=$(pwd -P)/$1 ;;
esac
shift
rest=
while [ ! -d "$path" ]; do
    name=$(basename "$path")
    case $name in
        . | ..) refuse "give OUT without . or .. among the directories it would make" ;;
    esac
    rest=/$name$rest
    path=$(dirname "$path")
done
out=$(cd "$path" && pwd -P)$rest
case "$out/" in
    "$repo"/*) refuse "$out is inside the checkout $repo; give a directory outside it" ;;
esac
bead=${BEAD:-$repo/target/debug/bead}
[ -x "$bead" ] || refuse "no bead at $bead: run cargo build --package beadroll, or set BEAD"
mkdir -p "$out/json" "$out/segment" "$out/import" "$out/artifacts"

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/home"
printf '[user]\n\tname = seed\n\temail = seed@example.invalid\n[init]\n\tdefaultBranch = main\n' \
    > "$scratch/home/.gitconfig"
# git, and bead as machine M: its checkout $scratch/M, its store $scratch/store-M, and nothing of
# this shell's environment but PATH
git_() {
    env -i PATH="$PATH" HOME="$scratch/home" git "$@"
}
on() {
    m=$1
    shift
    (cd "$scratch/$m" && env -i PATH="$PATH" HOME="$scratch/home" BEAD_HOME="$scratch/store-$m" \
        "$bead" "$@")
}
bead() {
    on "$@" > /dev/null
}
# a command's key, from its --json answer
key() {
    on "$@" --json | sed -n 's/^{"id": "\([^"]*\)".*/\1/p'
}
# a bare remote NAME whose main has one commit, and the checkouts of the machines given after it
remote() {
    origin=$scratch/$1.git
    shift
    git_ init -q --bare "$origin"
    for m in "$@"; do
        git_ clone -q "$origin" "$scratch/$m" 2> /dev/null
    done
    git_ -C "$scratch/$1" commit -q --allow-empty -m seed
    git_ -C "$scratch/$1" push -q origin main
}

# One machine, an import and awkward text.
remote alone a
bead a init --prefix fz --import "$repo/infra/seed.jsonl"
one=$(key a create 'Quotes " and \ backslashes' -d "$(printf 'tab\there\nnew line, \342\200\250 and \360\237\230\200')")
two=$(key a create 'Ünïcödé — ✓ 中文 עברית' -p 0 -t bug -l alpha -l 'β' --parent "$one")
three=$(key a create 'Loose end' --acceptance "$(printf 'all \001 controls \037 and DEL \177')")
bead a comments add "$one" "$(printf 'a comment\nover two lines \033[1m with an escape')"
bead a dep add "$two" "$three"
bead a update "$one" --notes 'notes: {"json": [1, 2.5, -0.0, 1e300]}' --add-label urgent
bead a claim "$one"
bead a release "$one"
bead a claim "$two"
bead a close "$two" -r 'done, "quoted"'
bead a reopen "$two"
bead a delete "$three" -r 'not needed'
bead a sync
bead a export -o "$scratch/export.jsonl"

# Machines b and c write the same issues without seeing each other's writes; d, a copy of c's
# store, writes as c.
remote duo b c d
bead b init --prefix du
x=$(key b create 'Shared' -d 'both edit this')
y=$(key b create 'Second' -l one)
bead c init
bead b update "$x" --title 'b says' --offline
bead c update "$x" --title 'c says' --offline
bead b update "$y" --add-label two --offline
bead c update "$y" --remove-label one --offline
bead b claim "$y" --offline
bead c claim "$y" --offline
bead b dep add "$y" "$x" --offline
bead c comments add "$x" 'from c' --offline
cp -R "$scratch/store-c" "$scratch/store-d"
bead d comments add "$x" 'from a copy of c' --offline
bead d update "$y" --priority 0 --offline
bead c close "$x" -r 'c is done' --offline
bead b sync
bead c sync
# refused, and for that reason only: the same identity used from two places
if refused=$(on d sync 2>&1 > /dev/null); then
    refuse "a copy of c's store synced as c"
fi
case $refused in
    *"the same machine identity is being used from two places"*) ;;
    *) refuse "the copy's sync failed otherwise: $refused" ;;
esac
bead b sync
z=$(key b create 'After the merge' --parent "$x")
bead c sync
bead c update "$z" --status blocked

# Small inputs: whole files of a few lines, cut files of more, and single lines.
cut_into() { # FILE DIR NAME LINES
    if [ "$(wc -l < "$1")" -le "$4" ]; then
        cp "$1" "$2/$3"
    else
        split -l "$4" -a 4 "$1" "$2/$3-"
    fi
}
stores="$scratch/store-a $scratch/store-b $scratch/store-c $scratch/store-d"
n=0
# shellcheck disable=SC2086 # $stores: mktemp's names, which hold no space
for file in $(find $stores -type f -name '*.jsonl' | sort); do
    n=$((n + 1))
    cut_into "$file" "$out/segment" "segment-$n" 16
    split -l 1 -a 5 "$file" "$out/json/segment-$n-"
done
# shellcheck disable=SC2086
for file in $(find $stores -type f -name '*.json' | sort); do
    n=$((n + 1))
    cp "$file" "$out/segment/file-$n"
    cp "$file" "$out/json/file-$n"
done
# Several machines' journals in one input: b's store holds b's and c's, d's its copy of c's.
journals=$(find "$scratch/store-b" "$scratch/store-d" -type f -name '*.jsonl' | sort)
backwards=$(printf '%s\n' "$journals" | sort -r)
# shellcheck disable=SC2086 # one name a journal, none with a space
{
    paste -d '\n' $journals > "$out/segment/machines-interleaved"
    paste -d '\n' $backwards > "$out/segment/machines-interleaved-backwards"
    cat $journals > "$out/segment/machines-one-after-another"
}
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
