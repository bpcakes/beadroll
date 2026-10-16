# bead's fuzz targets

cargo-fuzz targets for the code that reads what bead did not write itself (roll-k2wy.14.4). Each
target asserts what must hold of any input, so that a wrong answer is a crash as much as a panic
is, and libFuzzer keeps the input of either:

- `json`: the JSON parser. Anything it reads, written in every dialect (the 12 `Format`s), reads
  back as the same value and writes again as the same text; a text with lone surrogate escapes
  survives the rewrite the store gives its small files.
- `segment`: the segment and attempt readers. An attempt read whole holds what the segment reader
  reads; what was read, written again as journal lines, reads back alike; replayed (each record in
  the journal of the machine it names) it answers alike whichever order the journals are read in,
  except for ids with several contents and not exactly one copy of their own (roll-k2wy.14.14);
  its export is read back by the importer's reader; and its projection, encoded and decoded,
  answers as replay does.
- `import`: the Beads importer. The dry run finds a problem exactly when the import is refused;
  an import that goes through plans valid events that keep every record's id as its key; the
  tracker's export reads back and, imported again, plans nothing.

`fuzz/` is a workspace of its own, as the root `Cargo.toml` notes: cargo-fuzz needs a nightly
compiler, which `rust-toolchain.toml` here pins for this directory alone, and its dependencies
(`libfuzzer-sys`, under `deny.toml` here) stay out of the main workspace's `Cargo.lock`. Nothing
changes for the main workspace, which builds on the root's toolchain.

## Running them

With cargo-fuzz (`cargo install --locked cargo-fuzz@0.13.2`) and the nightly
`rust-toolchain.toml` here names (`rustup toolchain install` with its `channel` and `--profile
minimal`, as the nightly job does), the targets build and run from this directory:

```sh
cargo build --package beadroll                  # in the checkout's root: the bead seed.sh runs
fuzz/seed.sh ~/.cache/beadroll-fuzz             # in the root: the synthetic corpus, outside the checkout
cd fuzz
cargo fuzz build                                # all three targets
cargo fuzz run json ~/.cache/beadroll-fuzz/json -- -max_total_time=300
cargo fuzz run segment ~/.cache/beadroll-fuzz/segment -- -max_total_time=300
cargo fuzz run import ~/.cache/beadroll-fuzz/import -- -max_total_time=300
```

A crash's input is written to `artifacts/<target>/`; `cargo fuzz run <target> <input>` runs it
again and `cargo fuzz tmin <target> <input>` makes it smaller. A finding is fixed with a test in
the module of its subject, or filed under roll-k2wy.14. `corpus/` and `artifacts/` are ignored by
git.

## The corpus

`seed.sh OUT` writes the seeds into `OUT/json`, `OUT/segment` and `OUT/import`, from synthetic data
only: what a bead (`BEAD`, else `target/debug/bead`) writes in a scratch store, checkout and bare
remote under a temporary directory, importing `infra/seed.jsonl` and editing with awkward text (its
journal segments, attempt files, small JSON files and export, cut into small inputs), and
`infra/seed.jsonl` itself. That bead sees only `PATH` and the scratch homes of this shell's
environment.

Real exports seed it only locally: `seed.sh OUT EXPORT...` reads each Beads export where it is
and cuts it into `OUT` too. `OUT` must be outside the checkout (`seed.sh` refuses one inside), and
so must the corpus directory given to `cargo fuzz run`, the first of which libFuzzer writes what it
finds into: nothing derived from a real export lands where it could be checked in. Never copy an
export, a journal or a corpus built from one into the repository.

## Nightly

`.github/workflows/rust-nightly.yml`'s `fuzz` job installs the pinned nightly and cargo-fuzz,
checks the targets' formatting, builds bead, seeds the corpus with `seed.sh`, and runs each target
for `FUZZ_SECONDS` (600). A crash fails the job and its input is kept as the `fuzz-artifacts`
workflow artifact; `xtask`'s `ci::fuzzing` keeps the job running every target declared in
`Cargo.toml` with a budget.
