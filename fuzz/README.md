# bead's fuzz targets

cargo-fuzz targets for the code that reads what bead did not write itself (roll-k2wy.14.4). Each
target asserts what must hold of any input, so that a wrong answer is a crash as much as a panic
is, and libFuzzer keeps the input of either:

- `segment`: the segment and attempt readers. An attempt read whole holds what the segment reader
  reads; what was read, written again as journal lines, reads back alike; replayed (each record in
  the journal of the machine it names) it answers alike whichever order the journals are read in,
  what replay reports compared as a set. An input with an id that has several contents, not
  exactly one copy of its own, in two journals or more is not compared across orders at all: the
  copy read first decides that id's stand-in and its issue, and so everything that issue answers
  (roll-k2wy.14.14). Its export is read back by the importer's reader, and its projection, encoded
  and decoded, answers as replay does.
- `import`: the Beads importer. The dry run finds a problem exactly when the import is refused;
  an import that goes through plans valid events that keep every record's id as its key; the
  tracker's export reads back and, imported again, plans nothing.

`fuzz/` is a workspace of its own, as the root `Cargo.toml` notes: cargo-fuzz needs a nightly
compiler, which `rust-toolchain.toml` here pins for this directory alone, and its dependencies
(`libfuzzer-sys`, under `deny.toml` here) stay out of the main workspace's `Cargo.lock`. Nothing
changes for the main workspace, which builds on the root's toolchain. The targets need no nightly
to compile: every push checks and lints them, checks their formatting and runs cargo-deny on this
workspace on the root's toolchain (`.github/workflows/rust.yml`).

## Running them

With cargo-fuzz (`cargo install --locked cargo-fuzz@0.13.2`) and the nightly
`rust-toolchain.toml` here names (`rustup toolchain install` with its `channel` and `--profile
minimal`, as the nightly job does), the targets build and run from this directory. Keep the corpus
and the inputs that fail outside the checkout, in `OUT` (here `~/.cache/beadroll-fuzz`):

```sh
cargo build --package beadroll                  # in the checkout's root: the bead seed.sh runs
fuzz/seed.sh ~/.cache/beadroll-fuzz             # in the root: the synthetic corpus, and OUT/artifacts
cd fuzz
cargo fuzz build                                # both targets
out=~/.cache/beadroll-fuzz
cargo fuzz run segment "$out/segment" -- -max_total_time=300 -artifact_prefix="$out/artifacts/segment-"
cargo fuzz run import "$out/import" -- -max_total_time=300 -artifact_prefix="$out/artifacts/import-"
```

libFuzzer writes what it finds into the first corpus directory, and a crash's input to its artifact
prefix: given last, that prefix overrides the `fuzz/artifacts/<target>/` cargo-fuzz passes, which
is inside the checkout (and ignored by git). `cargo fuzz run <target> <input>` runs a crash's input
again, and `cargo fuzz tmin <target> <input> -- -artifact_prefix="$out/artifacts/<target>-"` makes
it smaller, there too. A finding is fixed with a test in the module of its subject, or filed under
roll-k2wy.14. The test is written from what the input shows, never by copying an input from a corpus
seeded with a real export: such an input, however small, is that tracker's data.

A change of the main workspace's version or of beadroll-core's dependencies refreshes this
workspace's lock too, with `cargo update --workspace --manifest-path fuzz/Cargo.toml` in the root:
CI checks the targets `--locked`, and xtask's `ci::fuzzing` fails while the two locks name
different versions of `beadroll-core`.

## The corpus

`seed.sh OUT` writes the seeds into `OUT/segment` and `OUT/import`, from synthetic data
only: what a bead (`BEAD`, else `target/debug/bead`) writes in scratch stores, checkouts and bare
remotes under a temporary directory, and `infra/seed.jsonl` itself. One tracker has one machine,
which imports `infra/seed.jsonl` and edits with awkward text. On the other, two machines write the
same issues offline without seeing each other's writes, and a third, whose store is a copy of the
second's, writes as it until its sync is refused. Their journal segments, attempt files, small JSON
files and export are cut into small inputs, and the second tracker's journals are also interleaved
and joined into inputs that each hold several machines' journals and the copy's divergence, which
is what the segment target's order check needs. Those bead and git see only `PATH` and the scratch
homes of this shell's environment.

Real exports seed it only locally: `seed.sh OUT EXPORT...` reads each Beads export where it is
and cuts it into `OUT` too. `seed.sh` refuses an `OUT` inside the checkout, before it makes
anything. The corpus directory and the artifact prefix given to `cargo fuzz` must be outside it as
well, as above: nothing derived from a real export lands where it could be checked in. Never copy
an export, a journal, or a corpus or crash input built from one into the repository.

## Nightly

`.github/workflows/rust-nightly.yml`'s `fuzz` job installs the pinned nightly and cargo-fuzz,
restores the corpus earlier nights grew (`actions/cache`), builds bead, adds the seeds with
`seed.sh`, and runs each target in a step of its own for `FUZZ_SECONDS` (600). A crash fails the
job and its input is kept as the `fuzz-artifacts` workflow artifact. The corpus of each target that
ran clean is then minimized with libFuzzer's merge, which passes over an input that crashes; a
target that failed keeps its corpus whole, with the inputs that failed added, so that it fails
again each night until a fix makes them pass. The corpus is saved under the run's own key, for the
next night to restore the newest. `xtask`'s `ci::fuzzing` keeps the job so: every target declared
in `Cargo.toml` in a step of its own, after the seeding, a budget greater than zero that the job's
timeout leaves room for (a job that times out is cancelled and keeps nothing), a crash's input
kept, the merge of clean targets only, the corpus saved under a key of its own run that the restore
finds by its prefix; the per-push checks of the targets; and this workspace's lock holding
beadroll-core as the root's does.
