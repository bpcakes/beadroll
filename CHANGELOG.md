# Changelog

All notable changes to beadroll. The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/). The on-disk and published format is
versioned separately (format 1, frozen; see *Format and upgrades* in `docs/GUARANTEES.md`).

## [Unreleased]

- An issue's ownership history, the claims that ended, the claims that lost and the ends that
  came too late, is kept with its texts in the projection rather than in the state part a read
  decodes (roll-lqjj.40, X16; `projection::ENCODING` 10, so the first command of this build replays
  the journal once): the commands that print issues whole read it in, as they read in descriptions
  and comments, and the store reads it in before it applies records about the issue. Each issue's
  claim, generation and offline claims stay in the state part, which `claims`, `prime`, `ready` and
  every ownership decision read, and so does who ended its last claim and how, which `close`,
  `delete` and `update` ask of issues whose texts they do not read in; a close or delete that
  published the claim it ended reads in the texts of the issues it named to tell whether a takeover
  made it too late. At 200,000 events, measured on 9cbd60a before roll-lqjj.39 and roll-lqjj.43
  landed, the state part is 8.63 MB rather than 9.45 MB and decodes in 36.5 ms rather than 40.7 ms
  in process (36.9 ms rather than 39.3 ms in the review's run), holding 45.3 MB rather than 51.4 MB,
  and the text file is 3.31 MB rather than 2.00 MB; on the ownership-heavy history 4.79 MB rather than 8.70 MB, 20.9 ms
  rather than 32.8 ms and 26.7 MB rather than 47.1 MB (`cargo xtask timing --decode N`). `list`
  takes 0.108 s rather than 0.114 s, `show` 0.100 s rather than 0.106 s and `ready` 0.146 s rather
  than 0.160 s; a claim and a release, each published, cost what they did (0.57 s and 0.58 s,
  against 0.61 s and 0.60 s; `cargo xtask timing --read N`, which now times them). Measured again
  on fd5ae9b, with them (both builds alternated, three rounds of the decode and five of the reads,
  on a busier machine, so slower throughout): the state part is 6.85 MB rather than 7.69 MB and
  decodes in 57.2 ms rather than 61.1 ms (58.6 ms rather than 60.0 ms, the fastest of the review's
  six rounds: −2% to −6%), holding 37.1 MB rather than 43.2 MB, and the text file, with each issue's
  edges' metadata and ownership history, is 4.93 MB rather than 3.62 MB; on the ownership-heavy
  history 4.79 MB rather than 8.70 MB, 40.4 ms rather than 53.5 ms and 26.7 MB rather than 47.1 MB,
  the texts 6.44 MB rather than 1.02 MB; `list` takes 81 ms rather than 87 ms, `show` 74 ms rather
  than 82 ms and `ready` 121 ms rather than 128 ms, the projection file is 15.62 MB rather than
  16.46 MB, and a published claim and a release take 0.42 s each. Every answer
  is unchanged, and format 1 too. `Issue::last_ended` is new: who ended the last claim and how,
  whether or not the issue's texts are read in; `Issue::claim_history`, `lost_claims` and `stale`
  are empty until they are.

- A command that reads decodes dependency edges typed (roll-lqjj.39, X16): each edge's target is an
  issue's id, held as its 16 bytes, when its text is an id's spelling, and the text otherwise; its
  kind is one name for every edge of that kind in a state replayed or decoded; and the metadata each
  addition carries (`created_at`, `created_by` and whatever an import or the `dep_add` brought),
  which only `bead export` prints, is kept with the issue's texts (roll-lqjj.6) and read in where
  export and a later event need it. Readiness, blockers and dependents no longer parse each target
  as an id. At 200,000 events (`cargo xtask timing --decode N`, both builds back to back on a shared
  machine, measured on 9cbd60a before roll-lqjj.43 landed) the state part a read decodes is 7.69 MB
  where it was 9.45 MB and decodes in 33.1 ms where it took 41.0 ms, holding 43.2 MB where it held
  51.4 MB; the text file is 3.62 MB where it was 2.00 MB; `cargo xtask timing --read N`: `list`
  0.099 s where 0.111 s, `show` 0.091 s where 0.106 s, `ready` 0.146 s where 0.165 s, a local write
  0.162 s where 0.184 s, and at 20,000 events 13.9, 13.3 and 17.3 ms where 15.7, 16.1 and 19.2 ms.
  The ownership-heavy history, which has no edges, has the same state part and heap, and decodes
  within half a millisecond of the base's median in three alternated sets of 8 to 10 rounds, and 1.0
  to 3.6 ms slower (3 to 11%) in the review's three; replay's heap peak at 200,000 events is
  unchanged (102.1 MB). A write that adds or removes an edge needs that issue's texts read in, as
  one that sets a long field does, and so does a create of a key another issue's import waits for,
  in the issue that waits: `Projection::about` lists them, and the store and a transaction read in
  what it lists. `Issue::deps` and `Issue::dep_tags` give the same answers in the same order, as
  `Edge` (`Target` and kind) and `Meta` (`None` until the texts are read in); `State::target` and
  `State::target_key` answer of a `Target` as `issue_named` and `key_of` of text. Output, exit codes
  and format 1 are unchanged; the projection is made again once (`projection::ENCODING` 9).
  `projection/tests/edges.rs`, recorded from the code before the change, pins what every issue's
  edges answered on every state the differential histories pass through and on three bead-built
  journals, and the differential generator now draws targets that name no issue, kinds beyond blocks
  and parent-child, imports' metadata, concurrent, re-added and renamed edges.

- A write-back of the projection compacts what every event had seen only now and then (roll-lqjj.43,
  X17): it had found every event the state names and cut the knowledge down to them at every
  write-back, though a projection read back and extended holds what was kept and the batch's events.
  The index now records how many events the knowledge held when it was last compacted, and a
  write-back compacts again only once the events taken in since are more than an eighth of those
  (`projection::COMPACTED`, `Projection::compaction_due`); between, it writes the knowledge as it
  is. A projection replay makes is compacted as before. At 200,000 events the read after a write
  takes 0.223 s rather than 0.263 s, and at 20,000 27.2 ms rather than 29.6 ms (`cargo xtask timing
  --read N`); its encode in process 0.048 s rather than 0.103 s, and the write-back that compacts as
  long as before (`--decode N`). Writes, syncs and reads that write nothing back do not move.
  Between compactions the projection is up to an eighth larger in events: at the worst point, 10,989
  events taken in at 200,000, its index is 9.65 MB rather than 8.93 MB and its file 0.73 MB larger,
  and the heap a decoded projection holds 77.1 MB rather than 75.7 MB (`--decode N`). With a store a
  scratch harness brought to that point, a write's heap peak there was 100.7 MB rather than 98.5 MB
  (`cargo xtask bead`; 98.0 MB either way just after a compaction), and the read after a write 0.284
  s against the base's 0.316 s. Output, exit codes and format 1 are unchanged;
  `projection::ENCODING` is 8. `cargo xtask timing --decode N` also times the encode, between
  compactions and where one compacts, and the worst point.

- A write on a tracker with no remote keeps its transaction until the process exits, as an
  offline write does, rather than freeing it first for a publish with nowhere to go (roll-lqjj.50,
  X17): the command line now chooses where a write goes before it lets go of the transaction
  (`beadroll_sync::destination`, then `publish_soon_at`), and frees it first only when a remote is
  chosen, whether or not it answers. A remote that cannot be chosen (`.beadroll.json` naming one where the store
  coordinates through another) keeps it too, and the run still says "recorded locally, not yet
  published (CONFIG)" where it did. At 200,000 events such a write takes 0.174 s rather than
  0.264 s, as an offline write does, and at 20,000 32.0 ms rather than 35.6 ms; its heap peak is
  unchanged. `cargo xtask timing --read N` reports it (`no_remote_write_seconds`). Output, exit
  codes, format 1 and the projection are unchanged.

- Two properties join the fast suite (roll-k2wy.14.4): replay answers alike whatever order the
  records of a generated history reach it in, causal or as a store reads them, compared through
  every answer the public API gives (`replay::tests`, about 1.7 s of test time); and JSON
  round-trips in every dialect bead writes: all twelve `Format`s, the shortest float that reads
  back, lone surrogates through a store file's rewrite, journal lines, export records through the
  importer's reader, `--json` views and error envelopes (`compat::json_round_trip`, `storage`).
  Both draw from seeded generators; `beadroll_test_support::values` is new. cargo-fuzz targets for
  the JSON parser, the segment reader and the Beads importer are in `fuzz/`, a workspace of its own
  on a pinned nightly, and run nightly for ten minutes each on the corpus earlier nights grew and
  the seeds `fuzz/seed.sh` writes from scratch stores, a crash failing the job with its input kept
  (`fuzz/README.md`); every push checks, lints and runs cargo-deny on them. What the segment
  target found, an id with several contents and no single copy of its own placed by the copy read
  first, is roll-k2wy.14.14. Nothing bead does or prints changes.

- A write that is not published at once (`--offline`) keeps its transaction until the process
  exits, as a read keeps its state, rather than freeing it first (roll-lqjj.42, X17): `create`,
  `comments add`, `dep add` and `remove`, `close`, `reopen` and `delete`, and `import` hand the
  transaction, which holds the whole projection with its index, to the command line, which leaves
  it to the process's end. At 200,000 events an offline write takes 0.196 s rather than 0.269 s (a
  write after a write 0.183 s rather than 0.257 s) and at 20,000 about 32 ms rather than 37 ms. A
  write that goes on to publish frees it first, so that no command holds two, as it freed it
  before (writes published at once and syncs do not move), and `claim`, `release`, `takeover` and
  `update` with a claim read the store after they write, so they free it too. Output, exit codes,
  format 1 and the projection (`projection::ENCODING`) are unchanged.
  `beadroll_sync::writing_spent` hands the transaction over; `writing`, `writing_state`,
  `coordinated` and `run_with` free as before. `cargo xtask bead` also says what its process still
  held when bead returned (`heap held:`), and xtask's `heap` tests keep, with the peak, that no
  command holds a transaction beside the state it reads and that nothing of a tracker is held once
  a command returns, answered or failed.

- The unpublished count every command gives costs less on a tracker with many attempt files
  (roll-lqjj.47): it was not the 75 ms the issue was opened on for a store that has synced (1.3 ms
  of a 115 ms `list` at 200,000 events, 0.3 ms of 16 ms at 20,000, unchanged), which only a store
  the memo does not vouch for pays (a copy or move of the store, a missing memo, a store never
  synced: 22 ms at 200,000 events, until the next sync), but it grew with attempt files, which
  nothing compacts. On the ownership-heavy history (91,952 attempt files at 200,000 events) it
  takes 91 ms rather than 155 in a `list`, which takes 0.67 s rather than 0.74 s, and 8 ms rather
  than 14 at 9,132 files, where `list` takes 68 ms rather than 75; a sync's count 31 ms rather than
  98 and `info`'s 39 ms rather than 101. The memo's lines for files the count does not read are
  checked for their shape and not made into values, the attempt files are named by their bytes,
  and a claims directory that does not exist yet (this machine's, until it first claims) no longer
  makes every call copy the whole listing of attempt names. Every number and message is as it
  was. What remains is `meta.json`'s `published` map, one entry per journal file (roll-lqjj.49),
  and a write's second listing of the attempt files (roll-lqjj.48).

- `cargo xtask coverage [--stress] [--sweep]` reports which lines of the product's code the suites
  run, and the nightly workflow keeps its report as an artifact (roll-k2wy.14.5): cargo-llvm-cov
  over the workspace's tests, with `--stress` the stress models and with `--sweep` all 660 schedules
  of the ownership sweep (the nightly runs both), written as `target/coverage/html/index.html`,
  `lcov.info` and `summary.json`, and summarised on standard output: the lines by crate, then the
  ownership protocol's files with the runs of lines no test reaches. Its lines are those of
  `lcov.info`, each source line with code once; the HTML index and `summary.json` are llvm-cov's
  own, which adds up the lines of each function (96.12% of 21,018 lines there against 96.4% of
  19,705 printed), and both include the unit tests written inline in the product's files (1,789
  lines at 99.6%). The `bead` processes the black-box tests start are in the count: the command
  fails when no line of `crates/beadroll/src/main.rs`, which only a `bead` process runs, ran (one
  black-box takeover test reaches 125 of the 230 lines of `commands/ownership.rs` with an
  instrumented `bead` and none with a plain one), and when a profile could not be merged and its
  counts are missing. The fast suite runs 96.4% of the 19,705 lines and the protocol's files 96.6%
  of 2,822 (the stress models and the full sweep add three lines); it takes 164 s here against 130
  s without the report (16 test threads, a shared 64-core machine), the stress models 252 s and the
  sweep 84 s. The profiles are written into 16 files per binary rather than one for each `bead`
  process (3.5 GB for the fast suite alone, against 30 MB). It needs cargo-llvm-cov and the
  toolchain's `llvm-tools`, neither in `rust-toolchain.toml`: the command says how to install them,
  and the nightly workflow installs `cargo-llvm-cov@0.8.7`. A test that runs `bead` under `ulimit`
  takes `beadroll_test_support::UNPROFILED` (`LLVM_PROFILE_FILE=/dev/null`) for that run, since the
  profile written at exit would fail on the limit: the V6, G48 and segment-limit tests do. A `bead`
  killed by SIGKILL or run under a limit writes no profile, and the doctests (the workspace has
  none) are not covered. The protocol paths the first report showed untested are roll-k2wy.14.6
  to roll-k2wy.14.13 and roll-k2wy.14.15.

- A decoded issue holds each short field once (roll-lqjj.41, X16): title, priority, type,
  assignee, estimate, reference, and status with its close time and reason are shown from their
  registers, as long fields have been since roll-lqjj.10, rather than copied out of them into
  sixteen slots beside them; an issue keeps beside its registers only when and by whom it was
  created and when it was last touched (`projection::ENCODING` 7, so the first command of this
  build replays the journal once). At 200,000 events the state part a read decodes is 9.45 MB
  rather than 10.68 MB and takes 38.8 ms rather than 43.6 ms in process, holding 51.4 MB rather
  than 60.4 MB (an issue is 560 bytes inline rather than 976); `list` takes 0.108 s rather than
  0.115 s, `show` 0.103 s rather than 0.113 s, `info` 0.141 s rather than 0.156 s, a write
  0.236 s rather than 0.257 s, and at 20,000 events `list` 14.6 ms rather than 16.8 ms (`cargo
  xtask timing --decode N`, `--read N`). Replaying 200,000 events takes 0.55 s rather than 0.60 s
  and peaks at 102 MB on the heap rather than 108 MB. Every answer is unchanged, and format 1
  too.

- A git that gives no answer within a `BEAD_NET_TIMEOUT` that is not a whole number of seconds is
  reported with that timeout, to the millisecond (`git fetch gave no answer within 0.2s`, and
  `within 2.5s`), where it was rounded to whole seconds (`within 0s`, `within 2s`); a timeout of
  whole seconds reads as before (roll-0bp4).

- A write that follows a write no longer writes the store's projection back (roll-lqjj.36, X17):
  a command that writes leaves the stored projection as it found it while the records after it
  hold at most a 64th of its bytes, and the next read takes them in and writes it back, as every
  command did. At 200,000 events a write after a write takes 0.26 s rather than 0.38 s, a sync
  after a local write 0.34 s rather than 0.51 s and a write published at once 0.64 s rather than
  0.80 s; at 20,000, a write after a write 36 ms rather than 48 ms. A write after a read, and the
  read after one write, cost what they did. A read after a run of writes applies what they left,
  and the write that ends a run writes it back: at the most, after about 845 comments in a row at
  200,000 events, the read takes 0.32 s rather than 0.31 s and that write 0.42 s rather than
  0.40 s. What every command answers and the projection's bytes are unchanged. A merge whose
  events were written concurrently only with this machine's writes left after the projection, not
  with what it holds, now extends it where it replayed the journal (a refinement of roll-lqjj.8,
  by the maintainer's decision of 2026-10-16): a sync that merges an event concurrent with three
  such writes takes 0.63 s rather than 3.69 s at 200,000 events. One that brings events
  concurrent with what the projection holds still replays.
  `cargo xtask timing --read N` measures a write cycle: a write after a read, the read after it and
  a write after a write.

- A read that finds the projection current (`list`, `ready`, `blocked`, `show`, `prime`, ...)
  decodes of the events in it only who wrote the values two writers set without seeing each other,
  which conflicts show; every event's record, with its causes, moved to the projection's index,
  which only `Projection::decode` reads: a write, the first read after one (which extends the
  projection and saves it), `info` and `machines`, none of which gains (roll-lqjj.35, X16; `projection::ENCODING` 6, so the first command of this build
  replays the journal once). At 200,000 events the state a read decodes is 10.7 MB rather than 14.0
  MB and takes 44 ms rather than 57 ms in process, holding 60 MB rather than 73 MB (`cargo xtask
  timing --decode N`, new); `list` takes 0.121 s rather than 0.146 s, `show` 0.116 s rather than
  0.150 s and `ready` 0.157 s rather than 0.201 s, and at 20,000 events `list` 16 ms rather than 20
  ms (`cargo xtask timing --read N`). A write decodes the same. Every answer is unchanged, and
  format 1 too. The rest of the design, each step its own issue: roll-lqjj.39, roll-lqjj.40,
  roll-lqjj.41.

- An ownership round (claim, release, takeover, close of an owned issue) goes on from the
  projection its merge read only while the journal's files are the ones it was made from
  (roll-lqjj.37). Since roll-lqjj.29 it started its transaction from that projection because
  nothing writes under `events/` between the merge and the transaction, which only a comment kept
  true: a segment or an attempt file written there would have had the event built on stale heads
  and the claim checked against an old state, and no test would have noticed. The projection a
  merge keeps now comes with the identities of the files it was made from, and `Tx::on` compares
  them with the files and reads the store again when they differ: the segments are listed and
  stat-ed, the attempt files are the listing and identities the phase already took, and so is the
  identity of each claims directory, which a file added, removed or renamed over changes whoever
  wrote it. Nothing writes there today, so no output changes and a round still reads the state
  once; the check costs 26 µs with one segment and 0.67 ms with 300, and the best of ten claims on
  20,000 issues took 0.27 s before and 0.28 s after (a machine shared with other builds, so the
  same within noise). A new failpoint, `ownership.before-transaction`, holds a round between its
  merge and its transaction, so that a test can write the journal there.

- The guards held back from the work of roll-lqjj.34 are added now that their changes are on main
  (roll-lqjj.38, X15): `bead blocked`'s lists (X01), `blocked` in each of several details (X02)
  and a renamed copy's key (X03) each fail a test of beadroll-core when they grow with the square
  of their input, as import planning, finishing, waiting imports, readiness and dependents already
  do. Counted on the chains and lattices `timing --ready` measures and on two machines' creates of
  the same keys, at 250 and at 2,000 issues the code reads 8.0 to 10.1 times as much, and read 62
  to 67 times as much before roll-lqjj.24 and .25. A copy of a numbered key (`ex-1.1`), which
  counts upward past every number taken, is not guarded and grows with the square of the copies
  (roll-lqjj.46). `State::claims_key` and `State::claimed_keys` count the keys they read, in this
  crate's tests alone; no build of bead carries the counting, and nothing bead does or prints
  changes. A renamed copy's key is also timed, for the lookup a count cannot see turned back into
  a scan: `xtask`'s growth tests, run nightly in release, replay 2,000 and 16,000 copies two
  machines minted alike and fail above 24 times as long (10 on main; 70 where every claimed key is
  collected for each copy, 54 and 40 where a lookup is a scan).

- A tracker whose journal holds anything `bead info` reports under `integrity` (an event of an
  operation a newer bead writes, as in a rolling upgrade, an id found with two contents, a broken
  chain from a copied machine identity, a refused takeover) keeps reading its projection and
  applying new events to it (roll-lqjj.28), where one such entry had every later command replay the
  whole journal for good. Only a takeover of an id no event has and an id with two contents whose
  causes are not all there or loop back onto it still do, since an event still to come can change
  what replay makes of them. `bead close`, `bead reopen` and `bead delete` decide every issue they
  name from the state before they write, and read it once more after, instead of once after each
  issue's event; only an issue named again is decided from the state read again after the events
  before it. 60 closes on 4,000 issues with such an event in the journal take 0.066 s rather than
  11.6 s, and 0.26 s where the projection still cannot be kept. What they write and answer is
  unchanged. `cargo xtask timing --extend N` measures closes past an event of a newer bead.

- A command pays for each attempt file once where it paid several times (roll-lqjj.30, X07): its
  identity is taken once a phase under the store lock, whoever asks (the projection, the blobs a
  merge and a publish reuse), `meta.json` and its map of every published file are read once a
  command, the unpublished count reads only the memo's segment lines, and a listing sorts its
  names without taking each path apart. Every file is still stat-ed, never vouched for by its
  directory: an online write stats each attempt file four times where it did seven, a sync three
  where it did five, a read or a local write once. At 91,952 attempt files (`cargo xtask timing
  --read 200000 --ownership`) `list` takes 0.75 s rather than 0.97 s and a local write 0.93 s
  rather than 1.26 s; an online write 3.9 s rather than 5.7 s and a sync with nothing new 2.9 s
  rather than 4.0 s. Outputs and format 1 are unchanged.

- A reader outside the store lock lists the attempt files again once it has read the segments
  (X13): a claims directory that gains a file within one tick of an earlier listing keeps its
  identity on kernels that stamp it from a coarse clock, and a record in those segments that
  builds on that attempt was then read without it, waiting for one read. A new failpoint,
  `journal.before-read`, holds a reader before each segment it reads, so that a test can have a
  writer accept an attempt and append a record built on it there.

- `bead show` says `merged: unknown` of a closing commit that is not 4 to 64 lowercase hex digits,
  without asking git (roll-lqjj.26, X14): bead records the commit as `git rev-parse HEAD` prints
  it, and text another machine's event holds, such as `HEAD~1` or `:/text`, was evaluated as a
  revision in this checkout, where it names some other commit or searches every commit message.

- `bead show` asks git whether the closing commits are on the default branch once for all the
  issues it shows (roll-lqjj.26, review 16's X08), instead of about five gits for each closed
  issue, each finding the default branch again: the default branch is found once, each distinct
  commit is asked once, and the full commit ids bead records are asked together, so `show` of
  1,000 closed issues starts 8 or 9 gits rather than 5,003 and takes 0.1 s rather than 23 to 29 s.
  `merged` and everything `show` prints are unchanged, also for a commit made while the clock was
  ahead, which `rev-list` alone can take for one the default branch lacks, and for a default
  branch that leads to no commit; only in a repository missing a commit of the default branch's
  history can a commit beside it be merged where it was not, or unknown. A closing commit from
  another machine's event now reaches git only as a revision, never as an option, and one with a
  NUL in it is `unknown` where it made `show` fail with `IO` (X14).

- A write published at once reads the store's state twice, once for the write and once for the
  merge of the sync that publishes it, and `bead sync` once a round (roll-lqjj.29, X06), where an
  online write read it four or five times: the sync that publishes a write no longer counts renames
  or makes the report it threw away, a sync's report and a claim's transaction go on from the
  state its merge read, `bead sync` counts the keys renamed before it from the state its first
  merge reads, and `bead update` answers from the state its write left. `update`, `dep`, `claim`
  and `release` no longer copy the whole state to read it while they write. At 20,000 imported
  issues an online comment takes 0.72 s rather than 1.05 s, an update 0.75 s rather than 1.39 s,
  a claim 0.64 s rather than 0.97 s, and a sync that brings nothing 0.27 s rather than 0.60 s.
  Outputs, receipts and the protocol are unchanged. `cargo xtask timing --sync N` records a write
  published at once (`online_seconds`).

- A reading command keeps the state it answered from until its process exits instead of freeing
  every issue of it just before (roll-lqjj.32): at 200,000 events decoding takes about 100 ms and
  freeing 50 to 85 ms, so `bead list`, `show` and `ready` take about a quarter less there. A run
  that publishes what the command wrote frees it first, so no command holds two states, and
  `cargo xtask timing --import` peaks at the same heap as before. A write, and each round of an
  ownership command, now frees its transaction's whole projection after the store lock is let go of
  rather than inside it, so other commands no longer wait for the memory to be freed. Nothing a
  command prints changes.

- `bead info` and `bead machines` answer from the store's projection, as `list` does, instead of
  loading the whole journal, and `bead log` takes the keys it names issues by from the store's
  state instead of replaying the journal it reads for its events (roll-lqjj.31): at 200,000 events
  `info` takes 0.23 s rather than 3.30 s, `machines` 0.23 s rather than 2.40 s and `log` 3.21 s
  rather than 3.78 s; at 20,000 issues, 0.21, 0.22 and 1.24 s rather than 1.16, 0.88 and 1.37 s.
  Every output is the same. The projection now keeps each machine's latest event beside its
  journal length, so its encoding is 5 and the first command after upgrading replays the journal
  once. `cargo xtask timing --read N` times the three too.

- `bead blocked` lists every held issue's blockers in one pass over the issues and their edges
  (roll-lqjj.24), instead of walking each issue's ancestors again for each: on a chain of 16,000
  issues under one held root it takes 0.28 s where it took 39 s, and on a lattice whose issues each
  list 64 gates 0.45 s where it took 81 s. `bead show`, `bead update`, `bead claim` and `bead
  takeover` judge `blocked` once for all the issues they show, instead of walking each one's
  ancestors: `bead show` of every issue of that chain takes 0.62 s where it took 27 s. The lists,
  `blocked` and everything printed are the same bytes; one issue's detail still walks its own
  ancestors. Sorting issues for `list` and `ready` no longer allocates a string for every
  comparison (0.95 ms rather than 2.1 ms for 20,000 issues). `cargo xtask timing --ready N`
  measures the lists both ways, on a lattice with gates of its own too.

- The work roll-lqjj.15 to .20 made grow with its input is guarded against growing with its square
  again (roll-lqjj.34, X15). beadroll-core counts, in its own tests alone, the edges read, the
  issues relabelled, the parent-child index built, the waiting imports visited and the import keys
  read, and a test for each algorithm — import planning and its dry run, a projection's finishing
  pass, the waiting imports, readiness and the dependents of the issues shown — fails when an
  input eight times as large does more than sixteen times the work. What a count cannot see, a
  lookup turned back into a scan or a set back into a vector, is timed instead: `xtask`'s growth
  tests, ignored elsewhere, run nightly in release and fail when eight times the input takes more
  than 24 times as long. Nothing bead does or prints changes, and a build of bead carries none of
  the counting.

- Replay renames a copy two machines minted alike by asking of each key it tries whether an issue
  has it or an import still waits for it (roll-lqjj.25), instead of collecting every issue's key
  and every key an import waits for once for each copy. Two machines that import one export of
  8,000 records offline and then sync rename 8,000 copies: the second machine's merging sync takes
  3.0 s rather than 7.0 s and its first command after, a replay, 1.2 s rather than 5.0 s (measured
  together on a shared machine). The keys chosen, the renames reported and everything printed are
  unchanged, and `State::claimed_keys` keeps its answer. `cargo xtask timing --renames N` measures
  replay of N renamed copies against the same creates renamed by nobody.

- The projection and its text file are no longer fsynced (roll-lqjj.27, review 16 X05). Both are
  caches that carry a sum of every part and that bead makes again from the journal whenever they
  are missing, cut, damaged or out of step with each other, yet the command after every write
  flushed them as the journal is flushed. They are still written whole and renamed into place; a
  crash or a power loss can now leave either lost, cut, zeroed or stale, and the next command that
  cannot read them as written replays the journal once, as it does for a projection deleted.
  The command after a write takes 9.5 ms rather than 39 ms on 300 issues and 46 ms rather than
  72 ms on 5,000 (a write then a `show`, medians of interleaved runs), and `cargo xtask timing
  --read 1000` times a write at 20 ms where it took 29 ms. Journal segments, attempts, pending
  files, receipts, `meta.json`, `replica.json` and the journal memo are flushed as before, and
  answers, the projection's and the text file's bytes and format 1 are unchanged.

- Every command starts the committed `.beadroll.json`'s `git cat-file` together with the other two
  gits that find the workspace (roll-lqjj.33), instead of after both had answered, wherever the
  directory it runs in shows no `.beadroll.json` in the tree. That is the common case, a repository
  that never committed one and so uses the workspace derived from its origin URL (the default
  setup, `docs/REVIEWING.md` §2), where every command ran that `cat-file` after the other two; and
  the rarer ones, a worktree on a branch cut before the file was committed, a bisect, a bare clone,
  a repository named by `GIT_DIR`. The same gits run, and the answer is used only where git's own
  top has no `.beadroll.json`, so every answer, warning and error is unchanged; the third git no
  longer waits for the first two, which was a git's start and exit (about 1.4 ms here, more on
  macOS) of every such command. A write still reads HEAD and its branch when its transaction
  starts, under the store lock, so a branch switched while it waited is what its events record.

- `bead show`, `bead update`, `bead claim` and `bead takeover` find the issues that depend on each
  issue they show once for all of them (roll-lqjj.20), instead of reading and sorting every issue's
  dependencies again for each: showing all 8,000 issues of a sparse tracker takes 0.28 s rather
  than 3.26 s, and all 4,000 0.13 s rather than 0.85 s. What each detail lists, its order and the
  order of the details are unchanged; one issue's detail still reads every issue's dependencies
  once, and costs no more than it did. `cargo xtask timing --details N` measures it.

- `bead ready`, `bead blocked` and `bead prime` judge whether anything holds an issue once for
  every issue (roll-lqjj.19), instead of building each issue's complete blocker list, walking its
  ancestors, to test the list for emptiness: a chain of 4,000 issues under one held root is judged
  in 2 ms where it took 1.7 s, and `bead ready` on such a tracker takes 0.037 s rather than
  2.05 s. The rule, what each command lists and its order are unchanged; `bead blocked` still
  lists each held issue's blockers, and `bead show` still judges its one issue by walking its
  ancestors, stopping at the first thing found. `cargo xtask timing --ready N` measures it.

- Replay finds an imported dependency on a key no issue has yet by that key (roll-lqjj.18): a
  create visits only the dependencies waiting for its own key, instead of every issue still waiting
  and each one's whole list of them. 16,000 imports each waiting for a key that never comes replay
  in 0.09 s where they took 30 s, and one issue waiting for 16,000 keys that come one create at
  a time costs about what the same edges cost it given at once. Edges, what still waits, their order
  and format 1 are unchanged. The projection no longer stores the waiting issues, which it derives
  instead, so its encoding is 4 and the first command after upgrading replays the journal once;
  `cargo xtask timing --dangling N` measures it.

- A projection finishes only what a batch changed (roll-lqjj.17): the labels of the issues whose
  operations change labels, each issue's derived once however many of the batch's events changed
  them, and the parent-child index only when an operation changes a dependency edge, instead of
  deriving both over the whole state for every batch applied. The ids it holds are a set, so a
  batch's go in where they belong wherever they sort among them. A command that writes one event
  per issue and reads the state between them — `close`, `reopen` and `delete`, over several ids or
  a subtree — therefore costs the state once rather than once per issue: one `bead close` naming
  8,000 issues of 8,000 takes 0.7 s where it took 19.8 s. Replay, the state every command sees,
  the projection's bytes and format 1 are unchanged; `cargo xtask timing --extend N` measures it,
  including a batch that changes one issue's labels over and over and writes whose ids sort before
  every id held.

- The journal memo and the projection vouch for a file only once it was settled when they were
  written (Y01): a rewrite in place that keeps a file's length keeps every part of its identity
  when it falls in the same timestamp tick, which the clock an inode's times come from need not
  have left. Such a file is hashed again, or read against the sum kept of its bytes, which attempt
  files now carry as segments did. Writing an attempt file forgets the attempt names listed in
  that phase for the same reason, so an attempt this process just wrote is never left out of a
  publication tree.

- Import planning and dry-run validation index dependency keys once per batch (roll-lqjj.15),
  avoiding repeated scans as the number of imported issues grows, and both resolve a key through
  the same lookup. A re-import indexes a record's listed dependencies only when the issue still
  holds unresolved ones, and an import no longer copies each record to read it.
  `cargo xtask timing --plan N` measures these calls.

- The README is the front door (roll-dih8.27): the problem, what beadroll does differently from
  Beads, a session with its output, what agents need, how it works, and the limits a user meets
  first. Every guarantee it held moved verbatim to `docs/GUARANTEES.md` (the claims protocol at
  each boundary, what the journals tolerate, the format, the command line, configuration in full,
  moving a repository, every limit with its measurement); the design decisions that changed while
  building to `docs/REVIEWING.md` §2; the tests' description to `CONTRIBUTING.md`. Nothing in
  bead changes.

- Commands reuse accepted-attempt directory listings within each locked phase while retaining
  every file's metadata check (roll-lqjj.12). Publishing reuses unchanged claims subtrees held by
  the merged tip (roll-lqjj.13), and fetched-head bookkeeping shares one `update-ref` transaction
  process with the later push (roll-dih8.14). The protocol, published bytes and store format are
  unchanged.

- `bead init --import FILE --dry-run` and `bead import FILE --dry-run` say what the import would
  do and write nothing, start no store and ask no remote (roll-dih8.26): how many issues would
  come in, by status, and how many are already here; every record it would be refused for, not
  only the first; the `in_progress` ones, which arrive with no owner; dependencies on keys the file
  lacks; and, in a git checkout, the branches with commits touching `.beads/issues.jsonl` that HEAD
  lacks and the worktrees with uncommitted changes to it. `--json` gives `importable` and the rest.
  `MIGRATING.md` starts with it, and names `br sync --flush-only` for a database ahead of its file.

- Each long text (description, design, acceptance criteria, notes) is held once, in its
  register, where it was also copied into the issue (roll-lqjj.10): a state and the projection's
  text file hold it once. A generated 49 MB export's text file falls from 75.0 MB to 43.3 MB, and
  the first sync that makes its projection from 245 MB on the heap to 191 MB. The projection's
  encoding and the text file's header change, so each store's projection is made again once.
  `cargo xtask timing --read N` and `--import N` record `projection_bytes` and `texts_bytes`.

- `bead close --recursive ID...` closes each issue and everything beneath it, at any depth,
  children first, in one write (roll-dih8.4). Issues beneath that are deleted or already closed
  are left as they are and counted in one line; another owner's claim anywhere in the subtree
  refuses the whole command, naming each, unless `--force`. Only `close` takes the flag.

- `docs/RUST-PORT.md`, the plan the Rust port followed, is removed (roll-dih8.24), with its test
  catalogues in `docs/rust-port/`. What of it was still in force moved: format 1 byte for byte and
  the design decisions to `docs/REVIEWING.md`, the rules every crate keeps and the tests' variables
  to `CONTRIBUTING.md`, the variables bead reads to the README's *Configuration*, the failpoints to
  `crates/beadroll-store/src/failpoint.rs`, and the gits each command starts to
  `crates/beadroll-git/AGENTS.md`. The plan and its measurements stay in git: `git show
  5a6ea8d:docs/RUST-PORT.md`.

- The command line is clap's (roll-dih8.23): the argparse port, kept for parity with the Python
  bead until its cutover, is gone. Commands read the same values and receipts sign the same bytes;
  a long option may still be shortened while unambiguous, and the last value of an option still
  wins. Errors are clap's and suggest what was meant (`unrecognized subcommand 'lis'; tip: some
  similar subcommands exist: 'lint', 'list'`), still `USAGE` with exit 2, with the usage after
  them for a human; a byte that is not UTF-8 still shows as `?`, or `\udcXX` in JSON. A positional
  value that starts with `-` goes after `--`. Help pages are clap's (`Usage: bead create
  [OPTIONS] [TITLE]`), with `--json`, `--actor` and `--color` under *Global options*, which must
  be written in full.

- Colour on a terminal: issue rows (status mark, id, priority; type and branch dimmed), `show`,
  `claims`, `log`, warnings, errors, the overview, `prime` and the help pages. `--color
  auto|always|never`; with `auto`, `NO_COLOR` turns it off, `CLICOLOR_FORCE` on, `CLICOLOR=0` and
  `TERM=dumb` off, else a stream is coloured when it is a terminal. Never into a pipe or a file,
  never with `--json`, never instead of escaping: what an issue says is escaped before anything is
  painted. `FORCE_COLOR` is not followed: agent harnesses set it, and their agents parse bead's
  output.

- `bead log` names who wrote each event as claims do: an event written with `--actor NAME` or
  `BEAD_ACTOR` shows that name, where it showed the agent and thread as if they had written it;
  `log --json` rows gain `name` (null without one). A `set` row names the fields it changed
  (`set title,priority`); the JSON `op` stays `set` (roll-dih8.19).

- A repeated `close`, `reopen` or `delete` writes nothing when the issue already has the status it
  gives, no claim or provisional claim is left for it to end, and that status is undisputed: it
  answers `✓ Already closed KEY: TITLE` (`unchanged` in JSON), exit 0, and keeps the reason the
  issue has, saying a reason given then is not recorded. Before, it wrote another event, replaced
  the reason and reported a fresh close. `update` with nothing to change writes nothing and
  answers `Unchanged`, where it said `Updated` (roll-dih8.20).

- Every command's help page describes: its line from the command list as its description, a help
  text for each argument, placeholders that say what they take (`ID`, `FILE`, `TEXT`, `N`, so
  argparse's errors name them too: `the following arguments are required: ID`), and for
  `create`, `dep` and `comments` their forms and what each dependency type does (`--deps
  blocks:X`: the new issue waits on X). `bead -h` describes bead instead of printing the Python
  module's docstring (roll-dih8.21).

- `bead`, `bead help` and `bead prime` are laid out for the terminal instead of printed as text
  wrapped at 100 to 120 columns, which a narrower terminal broke mid-word: paragraphs wrap at
  spaces; commands and their descriptions keep two columns, each description wrapping under its
  own, and stack one above the other when the terminal is too narrow for both. The width is
  `COLUMNS`, else the terminal's, at most 100; into a pipe, or with `--json`, nothing wraps, so an
  agent reads each line whole. The forms lists of the `create`, `dep` and `comments` pages wrap
  the same way at the page's width. Wrapping measures display width (`textwrap`), so a wide
  character counts twice (roll-dih8.22).

- `bead init` now acknowledges successful publication in human output, naming the remote and
  coordination ref. Offline runs, runs without a remote, failed publishes, and runs that only
  join or reuse a tracker do not claim a push; JSON output is unchanged (roll-dih8.18).

- Rust APIs now describe their behavior: `op::PyInt` is `op::IntegerField`, and `py_*` text
  helpers use parsing, trimming and casing names. `pathlib` and `pathlib_join` become
  `normalize_path` and `join_path`. Source comments describe current contracts
  instead of port history. Library callers must use the new names; stored data, command-line
  behavior and format-1 compatibility are unchanged. Test names and all `AGENTS.md` guides also
  describe current contracts; fixture provenance lives in the port-history document (roll-dih8.17).

One golden path for setting up a repository's tracker (roll-dih8.13): `bead init` once, from any
checkout, nothing committed; `bead sync` on every other checkout and machine.

- `bead init` asks the remote before it starts a tracker. A tracker the repository already
  publishes there is joined, with no second `init` event, and adopted as `sync` adopts it when it
  was published under another id (a moved repository, or a tracked `.beadroll.json` the clone
  lacks); a failure to join it fails `init`, which then starts nothing. `init --tracked` names
  the tracker the checkout has, joined just now or here already (a tracked id the remote
  publishes, one an earlier `init` adopted), minting a new id only where there is none, and
  refuses (`CONFIG`) to pin a URL-derived one, whose id would stop matching when the URL changed. A published tracker it cannot adopt is refused with
  `WORKSPACE_MISMATCH`. Before, `init` there started a second tracker, whose writes could never be
  published.

- `NOT_INITIALIZED` says to run `bead sync` first and `bead init` only to start a tracker the
  repository does not have; when the remote publishes none, `sync` says how to bring a moved
  repository's tracker along to that remote (`BEAD_WORKSPACE=ID bead sync --remote origin`, the
  remote as the sync chose it), naming this machine's tracker of the same name when it has one.

- `bead info` (`tracked_config_warning`) and `bead prime` warn while `.beadroll.json` is not
  committed, differs from its commit as JSON (even under `--skip-worktree`; line endings git
  converts are no difference), or is not on origin's default branch, also when the tree lacks the
  file and HEAD's copy is the one in use: the other checkouts and machines do not see it. A checkout with no
  `refs/remotes/origin/HEAD` is told it cannot check that (`git remote set-head origin --auto`).

- A coordination remote is kept as it was chosen: a configured remote's name (`--remote`,
  `$BEAD_REMOTE`, `.beadroll.json`; origin when none is chosen), or a URL or a path as given. A
  store remembers the name, which every command looks up again, so `git remote set-url` is
  followed for any remote as it always was for origin; it used to remember the URL the name led
  to then. A name means the URL it is configured with for `git push` (its first push URL, else its
  first URL), as origin itself did; it used to be the fetch URL as git expanded it, another place
  when the two differ. bead gives git that URL as a remote defined for the command, never as an
  argument git could take for another remote's name, so that git rewrites it as for `git fetch
  NAME` and `git push NAME` (a remote's own push URL without `pushInsteadOf`), from the top of the
  checkout; origin too, by the spelling it is configured with, which `info` and `sync` now show,
  where bead used to make a relative path absolute that a rule could then no longer rewrite. A
  tracked remote is the store's when it is the same name or the same URL, or, in a
  store an older bead left with a tracked name saved as git expanded it, where git's own answers
  agree; any other change is refused (`CONFIG`) until made deliberately. Origin's own URL given as
  a URL spelled as configured is origin, reached as origin is, its own push URL included, and
  `init --tracked --remote` stores it as `origin`, so the file follows origin when the repository moves. A relative path
  typed in a subdirectory is read from there, also by `init --tracked`, which stores it from the
  top of the checkout whether it exists or not; a path a rule rewrites is left to git as written.
  Going back to 0.2.0, a store whose tracked and remembered remote are one name is refused there
  until `bead sync --remote NAME`.

- `bead init --json` reports `unpublished` for the tracker it wrote to, also when that is not the
  checkout's own (a joined, adopted or tracked one).

- The README describes the golden path and moving a repository, and MIGRATING runs the import once.

- Concurrent bead processes of one store (agents in several worktrees) no longer fail a command
  with `REMOTE_UNREACHABLE` when one fetch finds another's fetch ref gone while git reads the refs
  (`fatal: bad object refs/beads/fetch/…`), or the directory of its own removed while git makes
  it: the fetch is tried again after a short wait, five attempts in all (roll-dih8.12).

- The protocol sweep fails a schedule whose fault never fired: the operation must end at its
  boundary (exit 98 or 99, a SIGKILL, or the remote's stalled push). The sync schedules record
  their local work offline, so the 48 at the push and timeout boundaries now push, where they
  passed with nothing to publish, and the default slice stops a sync at its push (roll-dih8.10).

- A publishing write (a sync, a claim, a published local write) hashes only the journal files that
  changed since bead last hashed them, once: the active segment, new attempt files and files whose
  length or identity changed, where it hashed every segment and attempt file of the store twice
  (roll-lqjj.1). It no longer reads every segment to count its lines or to compare it with the
  published copy, and checks a segment for a torn tail by its last byte. What it remembers of each
  file (its identity on disk, its lines, its blob) is a cache, `sync.git/journal-memo`: a file
  that keeps its identity is not hashed again, and its remembered blob goes into a tree only when
  it is sure to be in `sync.git`; the tree published is the one hashing every file makes.

- `cargo xtask timing --sync N [--ownership] [--rotate BYTES]` times a publishing `bead sync` on a
  scratch store of N events against a local bare remote, with the store's memo of its journal files
  and without it.

- Replay holds a fraction of the memory it did (roll-dih8.11): 200,000 events peak at 107 MB on
  the heap, and at 133 MB when about half of them are claims, takeovers and releases in attempt
  files; their resident peaks fell from 921 and 958 MB to 187 and 215 MB, and the replay itself
  from 0.26 and 0.33 s to 0.21 and 0.29 s. A journal keeps what replay reads of each event and a
  digest of its content, not its parsed JSON, and each text and value events repeat once; replay
  takes each event's operation from the journal as it applies it, and the state keeps who wrote
  each event and what each had seen, not the journal; an issue keeps its registers, tags and
  claims in vectors sized to them. Events the size of this repository's own take more: its
  journal repeated to 200,000 events peaks at 364 MB on the heap. In `beadroll-core`, `Value::Object` holds a boxed `Object`, `Issue::deps` is a view of the
  dependency tags (`Deps`), claim records share their names and give their times by method
  (`Claim::since`, `EndedClaim::ended`, `Provisional::since`, `LostClaim::at`), and `State` no
  longer holds the journal.

- `cargo xtask timing --replay N` records the heap peak beside the resident one
  (`heap_peak_bytes`, `journal_heap_peak_bytes`): xtask's allocator is the system's, with a count.

- Every command reads the store's projection, `<store>/projection`, instead of replaying the
  whole journal (roll-lqjj.2): the state the journal replays into, kept with the build of bead that
  made it and the identity of every journal file it was made from. Records appended since are
  read alone and applied after it, as replay would apply them; anything else (a file changed
  another way, events written concurrently with the store's, a takeover) has the journal replayed,
  and the projection written again. At 200,000 events a read takes 73–87 ms where it took 1.06 s,
  a sync that merges ten events another machine wrote 0.31 s where it took 1.31 s, and a
  publishing sync 0.38 s where it took 1.34 s; on a thousand issues that 50,000 or 200,000 events
  made, a read takes 18 ms either way. The projection is a cache: deleting it is always safe, and
  one another build wrote, or a damaged one, is never read.

- A merge appends what another machine's segment lacks instead of writing the segment again whole:
  the bytes are the same, and the projection takes in the merged lines alone.

- The unpublished count every command reports reads only the segments appended to since the
  store's memo saw them, where it read every segment whole.

- `cargo xtask timing --read N` times the reading commands with the projection and without it,
  `--merge N [--batch M]` a sync that merges another machine's events, and `--churn` shapes a
  history whose state stays its size; `--sync N` also records a sync without the projection.

- Each issue's texts (its long fields, their registers' values, its comments and its extras) are
  kept beside the projection, in `<store>/projection-texts`, apart from its state (roll-lqjj.6):
  `list`, `ready`, `blocked`, `claims`, `dep`, a write and a sync never read them, `show`, `comments`,
  `update`, `claim` and `takeover` read only those of the issues they print, and `export`,
  `search` and `lint` read them all. On a history whose texts are most of its state, at 200,000
  events a read takes 72–82 ms where it took 144–159 ms, a local write 92 ms where it took 188 ms,
  and the projection is 20 MB where it was 107 MB: what a history without texts takes. On the
  largest public Beads tracker (47 MB) a `list` takes 75 ms where it took 138 ms, holding 98 MB
  where it held 274 MB, and a published comment 0.35 s where it took 0.65 s. Texts that
  cannot be read as written have the journal replayed and both files written again; like the
  projection, the text file is a cache, and deleting it is always safe.

- `cargo xtask timing --text` shapes a history whose texts are most of its state, and `--read N`
  also times a local write.

- A sync, and every ownership command, makes 24 rounds against a moving remote before it gives up,
  twice as many as before, and waits between them any time up to `0.1 × 2^(attempt − 1)` seconds,
  at most two, where it waited `U(0.1, 0.4) × attempt` (roll-lqjj.7). With four machines each
  publishing every write at once, against a remote taking 0.3 s a fetch or a push, a write that
  lost race after race was left unpublished one time in a hundred; it now gets through, every
  machine converging, and the longest waits are shorter. `cargo xtask timing --contend M
  [--writes W] [--latency MS]` measures it.

- `cargo xtask timing --merge N --concurrent` times a merge of events written concurrently with the
  store's own, which replays the journal (roll-lqjj.8).

- `bead import` and `bead init --import` hold each form of an export once (roll-lqjj.9): the
  export's text until its records are read, the records until they are planned, and each event
  once, let go of as it is encoded into the journal's lines; `init` reads the tracker's prefix
  before it imports, so its state no longer takes in every imported event. Importing a generated
  49 MB export peaks at 165 MB on the heap where it peaked at 473 MB, and published at 245 MB: the
  projection its publish makes of the journal, what the first sync after an offline import and a
  new machine's first sync of the tracker take. A text file started again is written from its
  header and its texts as they are, not from a copy of them joined: the first sync after such an
  import holds 245 MB where it held 309 MB. An import stays all or nothing.

- `cargo xtask timing --import N` times importing a generated Beads export of N records, shaped as
  real trackers' are, and records the heap peak beside the resident one: `cargo xtask bead ARG...`
  runs bead in xtask's process, whose allocator counts.

## [0.2.0] — 2026-10-11

bead is now the Rust implementation: one binary, built from the Cargo workspace in `crates/`,
where 0.1.0 was a Python package. It reads and writes the same stores, journals, receipts and
published trees, so machines can be upgraded in any order and a store can go back to the last
Python bead, tagged `python-0.1.0`; and it keeps the commands, their flags, output, error codes and
exit statuses, help pages included. It differs only where Python's bead failed: where Python died
with a traceback (an unparsable setting such as `BEAD_NET_TIMEOUT`, a damaged store file) it
answers with a structured error (`docs/REVIEWING.md` §2), and on Linux under a locale that is not
UTF-8 it names a coordination ref by its UTF-8 bytes, as `.beadroll.json` holds it, where Python
used the locale's codeset (`roll-k2wy.18`). Each of the
Python suite's 470 tests was ported to Rust, the black-box ones passing against both
implementations, and fleets that mixed the two converged.

It is installed from source, with rustup: from a checkout's root, `cargo install --locked --force
--path crates/beadroll --root ~/.local`; without one, `cargo +1.99.0 install --locked --force
--config net.git-fetch-with-cli=true --git ssh://git@github.com/bpcakes/beadroll.git beadroll
--root ~/.local`. Either puts `bead` in `~/.local/bin` and upgrades it when run again; `uv tool
install` and the Python package are gone. It is faster: it starts in about 2 ms where Python took
about 40, replays 200,000 events in a quarter of a second, and a publishing write starts 14 gits
instead of 26 or 27.

Changes since 0.1.0, made before the cutover and carried over by it unchanged:

- Every bead reads a journal line the same way, whatever Python it runs on: as UTF-8 (a
  byte-order mark, UTF-16 or UTF-32 is no longer guessed from its first bytes), at most 512 levels
  deep, with NaN, Infinity and numbers too large for a float refused; ids and dependencies match
  the UUID shape whole, and the published manifest's format must be the integer 1. Key numbers,
  segment names and URL schemes are ASCII: a renamed copy of `x.٣` keeps its own name. Events
  that made replay or a query fail (or answer in a Python version's own words) are refused by
  name and reported under `integrity`, and an import fails whole on a record whose `created_at` is
  null or whose text holds a lone surrogate (`docs/FINDINGS.md`, U01–U11).
- `bead ready ID` offers only the ready issues in ID's subtree, at any depth and through every
  parent link; `--epic ID` now means the same, as in br (it used to match ID's direct children
  only, which `--parent ID` still does). A scope that names no issue is `ISSUE_NOT_FOUND`, in
  `list --parent` too, instead of an empty answer, and two scopes at once (a repeated `--parent`
  or `--epic` included) are a `USAGE` error.
- Verification fails on every selected protocol-sweep failure, including the previously
  exempt G12 scenarios; the default slice covers interrupted owner close/delete retries.
- `check.sh --real` reports failure when the round-trip driver fails, a bead command fails,
  or exported records differ, and preserves the full failing report. Imports stay offline
  in the scratch store.

## [0.1.0] — 2026-10-09

First release.

- One tracker per repository, shared by every worktree, clone and machine through one git ref
  (`refs/beads/journal`) on the repository's own remote; nothing on code branches.
- Append-only per-machine journals of causally ordered events (UUIDv7 ids), replayed into the
  current state; concurrent edits are kept and flagged, never merged by a clock.
- Claims settled online by a conditional update of the ref, persistent until release, close or
  takeover; every write published before the command returns, `--offline` to keep it local, and
  every command reporting what the remote still lacks.
- Beads JSONL import and export, checked against `br` on real trackers; custom statuses kept
  verbatim.
- `bead prime` and `bead onboard`: the workflow and the instruction block an agent needs.
- A private `sync.git` that bead maintains itself and rebuilds if damaged.
- Seven independent reviews closed; the ledger is `docs/FINDINGS.md`.

[Unreleased]: https://github.com/bpcakes/beadroll/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/bpcakes/beadroll/releases/tag/v0.2.0
[0.1.0]: https://github.com/bpcakes/beadroll/releases/tag/v0.1.0
