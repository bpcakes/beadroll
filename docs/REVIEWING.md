# Reviewing `bead`

A brief for an independent reviewer: an agent or a person who did not write this, whose job is
to find out whether the design holds, to break it if they can, and to report with
reproductions. Reviewers do not extend the tool.

Sixteen reviews have run; reviews 8 and 9 audit the Rust port plan, review 10 is Codex's, task by
task, of epic `roll-lqjj`, review 11 is Claude and Codex's review of its follow-up branch, and
review 12 is their review of the Rust names (roll-dih8.17); reviews 13 to 15 were reported inline
on branches before they landed, and review 16 is Claude's of the performance work of roll-lqjj.15
to .20.
Their reports are kept verbatim in `reviews/`; every finding and what became of it is in
`FINDINGS.md`. Start from what they did not cover.

## 1. What this is

`bead` is a proof-of-concept issue tracker for coding agents that work from many git worktrees and
several machines at once. It replaces the workflow around `br` (beads_rust), whose tracker state
lives in a file tracked on code branches: dirty checkouts, tracker-only commits, merge conflicts,
state split between worktrees and machines.

It is one binary, built from a Cargo workspace (Rust 1.99) whose crates each depend only on the
ones before them, which the compiler enforces:

- `beadroll-core`: Python-compatible JSON and text, event validation, journals and their settling,
  causal order, replay into state, readiness and the other queries, issue views, Beads import
  planning and export. No filesystem, processes, clock or environment.
- `beadroll-git`: the only code that starts git: the runner (its own session, timeouts, output to
  unlinked files), the environments of the checkout and of `sync.git`, remote URL rules, the
  classification of push outcomes and local damage.
- `beadroll-store`: this machine: `BEAD_HOME`, the store's files, modes and lock, segments and torn
  tails, attempt, pending and receipt files, identity, the workspace a checkout resolves to, the
  failpoint registry.
- `beadroll-sync`: transactions, the link to the coordination remote, `synchronise` and the
  ownership protocol.
- `beadroll`: the `bead` binary, `commands` and `cli` (parsing, rendering, output streams, exit
  codes); `beadroll-test-support`: the sandbox the black-box tests run it in.

`cargo build --package beadroll` builds the working copy as `target/debug/bead`; `README.md`'s
*Install* has `cargo install`. Until 0.2.0 bead was a Python package of seven modules, which the
Rust implementation ports with nothing changed in behaviour except where Python's failed: the
structured errors below where Python died with a traceback, and a coordination ref named by its
UTF-8 bytes under any locale (`roll-k2wy.18`). The last of the Python package is the tag
`python-0.1.0`, and its function names survive in the Rust ones below. The plan the port followed,
`docs/RUST-PORT.md`, was removed once nothing in it was in force that is not here, in
`CONTRIBUTING.md` or in a crate's `AGENTS.md` (roll-dih8.24); its measurements (Appendix A), its
ledger of CPython semantics (§5) and what reviews 8 and 9 corrected in it are in git:
`git show 5a6ea8d:docs/RUST-PORT.md`.

The design under test, in the order it is best read:

- **One journal per machine**, in append-only segment files
  (`$BEAD_HOME/<workspace>/events/<machine>/<segment>.jsonl`). A machine only appends to its own
  journal. A full segment is never rewritten.
- **Every event names the frontier its writer had seen** (`deps`; ids are UUIDv7). Current state
  is the replay of all journals in causal order (`causal_order`, `replay`, beadroll-core).
  Machines merge by exchanging files; nothing is text-merged.
- **One store per repository per machine**, outside every working tree (`find_workspace`,
  beadroll-store: tracked `.beadroll.json`, else a hash of the normalised origin URL, else a local
  id). Nothing is ever written inside a checkout.
- **Sync through one dedicated git ref** (`refs/beads/journal`) on one coordination remote,
  through a private bare repo (`sync.git`). The pushed history holds a manifest and the
  journals, never code. A push is a plain fast-forward whose parent is the fetched head, so the
  remote refuses it if anyone else published first (`Link::publish`, beadroll-sync).
- **Claims are persistent and settled online** (`coordinated`, beadroll-sync). Ownership events
  are not journal records: an *attempt* (the `claim`/`release`/`takeover` plus any edits made in
  the same command) is one file under `events/<machine>/claims/`, written first under `pending/`,
  where it counts for nothing anywhere until the remote holds it. A refused push drops the
  attempt; an unanswered one stays pending until a later contact finds it on the remote
  (confirmed) or fences it off with a commit on the same head (`settle_pending`). Attempt files
  are immutable and compared on sync; the accepted copy wins. A command whose push landed
  without an answer recognises its own accepted work (`settle_commands`) instead of redoing it.
  Ownership never expires; it ends only by `release`, `close` (by the owner) or
  `takeover --reason`. `--offline` claims are provisional and never displace a confirmed owner.
- **Concurrent writes are flagged, not merged**: each editable field keeps every value whose
  writer had not seen it replaced (`write_register`); the newest is shown and the alternatives
  appear under `conflicts`. A later write that has seen them all settles it. A disputed status
  makes an issue not ready, not claimable, and not "done" for dependents.
- **Labels and dependency edges are add-wins sets**: a removal removes only the additions its
  writer had seen.
- **Identity on every event**: opaque machine id, agent type and thread id (`detect_actor`,
  beadroll-store). **Place on every event**: branch and commit of the checkout the command ran in
  (`where`, `capture_where`).
- **Beads JSONL is an import/export format only** (`plan_import`, `export_records`,
  beadroll-core).

Read `README.md` and `docs/GUARANTEES.md`, then each crate's `AGENTS.md` and code in the order
above, then
`beadroll-test-support` and one test module.

**Format 1, byte for byte.** Format 1 is frozen (§2). These bytes are what every version of bead
reads, and the published tree is also read by `br`/`bv` through `bead export` and by studio:

| What | Written as | Notes |
|---|---|---|
| `$BEAD_HOME/replica.json` | `{"id": "<8 hex>", "alias": "<host>"}`, `", "`/`": "` separators, ASCII-escaped; created whole as `.replica.<pid>.tmp`, then `link(2)` | The first writer wins; the hostname lives only here |
| `aliases.json`, `moved.json` | `indent=1`, ASCII-escaped, no trailing newline | Unknown or invalid entries are ignored on read |
| `<ws>/lock` | Empty, 0600, `flock(2)` exclusive | |
| `<ws>/meta.json` | `indent=1`, ASCII-escaped; keys `remote` (the remote as it was chosen: a configured remote's name, or a URL), `name`, `last_sync`, `published` | origin is never stored |
| `<ws>/events/<machine>/NNNNNN.jsonl` | One event per line: `,`/`:` separators, raw UTF-8, keys in the order `v, id, replica, deps, actor, op, issue, data[, where]`; appended with `O_APPEND` and fsynced; a new segment starts at `BEAD_ROTATE_BYTES` | A torn tail is repaired before the next write and kept as `*.torn-<ns>` |
| `<ws>/events/<machine>/claims/<event-id>.json` | The attempt's events, compact lines, written atomically | Immutable; on a mismatch the local copy is kept as `*.fork-<ns>` |
| `<ws>/pending/<id>.json`, `<id>.command` | The pending attempt and its receipt | The receipt's signature is below |
| `<ws>/projection`, `<ws>/projection-texts` | A cache of one build of bead (§2) | No part of format 1: another build never reads them, and deleting them is always safe |
| `<ws>/sync.git` | `git init --bare --template= --shared=0600 --object-format=<the checkout's>`; git's maintenance off through the environment | Set aside as `sync.git.damaged-<ns>` when damaged |
| Temporary files | `.<name>.<pid>.tmp` beside the target | Readers and sync ignore them, and every name that is not `.json`/`.jsonl` |
| Modes | 0700 directories and 0600 files throughout the store, whatever the umask | Files written into a checkout (`.beadroll.json`, `export -o`) follow the umask |
| `.beadroll.json` | `indent=1` plus `"\n"`, ASCII-escaped; existing keys keep their order, new ones are appended | Committed in users' repositories, so a rewrite must not show a spurious diff |

The published tree on `refs/beads/journal` (or the tracked `ref`) holds exactly `manifest.json`
(`{"format": 1, "workspace": "<id>"}` and a newline: sorted keys, `", "` and `": "`), and byte
copies of the segments `events/<machine>/NNNNNN.jsonl` and accepted attempts
`events/<machine>/claims/<event-id>.json`. Commits are made with `commit-tree`, so no signing
configuration applies: subject `bead <claim|fence|sync> <12 hex>`, author and committer `bead
<bead@localhost>`, the fetched head as parent. They are pushed with `--porcelain --no-verify` as a
plain fast-forward. Each process fetches into its own `refs/beads/fetch/<pid>-<8 hex>` and records
the result in `refs/beads/remote`. The manifest's bytes matter beyond tidiness: a publish whose
tree is unchanged pushes nothing, so two spellings of one manifest would have two machines push
them in turn forever.

A command receipt (`pending/<id>.command`) signs its request as an object of `actor`, `args` and
`request`, written as Python's `json.dumps(..., sort_keys=True, default=str)` wrote it: sorted
keys, ASCII escapes, spaced separators, no trailing newline. `args` is the subcommand's whole
namespace, every destination with its value or default (`null`, `false`), `command` (the
subcommand's name) included; `--json` and `--actor` never reach it. `request` is `null` except for
`update`, where it is `{"fields": …, "status": …}` after normalisation, with file and stdin
contents resolved. `bead claim p-1` by `{"agent": "codex", "thread": "t"}` signs `{"actor":
{"agent": "codex", "thread": "t"}, "args": {"command": "claim", "ids": ["p-1"], "offline": false,
"remote": null}, "request": null}`. Only the coordinated commands write one: `claim`, `takeover`,
`release`, and `update` with `--claim` or `--status in_progress`; an owner's `close` and `delete`
are local writes.

The command line's contract: with `--json`, standard output holds exactly one JSON document
(`", "`/`": "` separators, raw UTF-8, keys in a fixed order, conditional keys only when set). On an
error standard output is empty and standard error holds `{"error": {"code": …, "message": …}}`
(ASCII-escaped) or `Error: …`. The exit status is 0 on success and when standard output is closed,
1 on an error, 2 for `USAGE`, and 98 and 99 for the crash hooks. Messages keep their texts, since
agents and tests match parts of them; `bead prime` lists the codes.

## 2. Decisions you must not report as findings

These were decided deliberately. Report problems *with* them, not their existence.

- Rust APIs describe their role rather than the implementation language bead replaced
  (roll-dih8.17): `op::IntegerField` retains the boolean or integer representation of a format-1
  field, and text and path helpers name their parsing, trimming, casing and normalization
  operations. These names do not relax the format or command-line contracts: old boolean integer
  fields, exact integers, Unicode text rules and receipt bytes still have their established
  meaning. Tests pin the reference values; where their fixtures came from is below.
- Claims never expire; a missing owner needs a human `takeover`. No heartbeats.
- The owner of a claim is the agent thread; a later session must `takeover` to continue.
- `update --status in_progress` is a claim, and `create --status in_progress` is refused;
  `update --status tombstone` is refused in favour of `delete`.
- `title`, `priority`, `issue_type` and `status` cannot be cleared.
- Recovery after a lost acknowledgement is per command (the receipt beside a pending attempt):
  a retry of the *same* command by the same actor is recognised; a different command after the
  crash is a new command and sees the accepted state.
- The Rust port preserves Python's receipt signature bytes and retirement rules while either
  implementation may use the store, including rollback. Sync settles attempts but can retain
  accepted `.command` receipts; an empty `pending_ownership` is not permission to discard them
  (the receipt's bytes are in §1).
- The display value for a conflicted field is the newest event id; no resolution command.
- Every local write (`create`, `update`, `comments add`, `dep`, `import`, `init`, and `close` by the
  owner) is published immediately, not as a conditional update; `--offline` keeps it local, and a
  remote that does not answer leaves it recorded and reported as unpublished (`unpublished` in every
  dict result and in `info`; a trailing line for humans) until a sync or an online command carries
  it. A close or release that names a claim superseded by a takeover is ignored and listed under
  `stale`, whichever was written first.
- A renumbered child key (`x.1` → `x.2` after two machines created children offline) changes
  under the machine that created it; events refer to issues by immutable id.
- One event with two different payloads is quarantined alone; nothing waits on it.
- The store lock is held while a sync or claim pushes (about a second against GitHub).
- Publishing is serialised on one ref; under heavy contention claims take seconds, and so do
  writes (roll-lqjj.7, the maintainer's decision of 2026-10-12: tune today's protocol rather than
  give each machine a ref of its own, correctness before speed; a write taking over a second is no
  defect). Four machines each publishing every write at once, against a remote taking 0.3 s a
  fetch or a push (`cargo xtask timing --contend 4`), converge every time; what the retries were
  tuned for is that a write losing race after race still gets through: a sync or an ownership
  command makes 24 rounds, twice Python's twelve, waiting between them any time up to
  `0.1 × 2^(attempt − 1)` seconds, at most two, where Python's linear wait would have reached two
  minutes over 24 rounds. Before, one write in a hundred, and one in four hundred, was left
  unpublished until a later command carried it; after, none in 650. Medians did not move beyond
  the runs' noise; the longest waits shrank (31 s to 14 s, 28 s to 23 s).
- No compaction beyond segment rotation. A persisted projection, a cache and never a source
  (roll-lqjj.2, step 3 of epic roll-lqjj, whose plan replaced the "no persisted projection" of
  earlier): `<store>/projection` keeps the state the journal replays into, with what it was made
  from: the build of bead that made it (`Settings::build`, the version and the executable's
  identity on disk), every journal file's identity as its bytes were read (compared as the memo
  compares files, and only once the file was settled when the projection was written, since an
  in-place rewrite that keeps length, times, inode and device is otherwise not seen, as git's
  index does not see it: Y01 below), and a sum of each file's bytes. A command whose files are the
  ones it was made from reads it, a reading command its state alone. A machine's last segment that
  grew, and still begins with the bytes summed, is read whole (at most what bead rotates at), and
  its new records and new attempt files' are applied after the projection when replay would apply
  them so: each new event has the whole frontier in its past, which a local write's always do and
  a merge's do when one writer worked at a time; they are then placed and applied with replay's
  own steps, and of what every event had seen the projection keeps only the events the state names
  (its state also keeps who wrote them, which conflicts show). Anything else replays the journal,
  as every command did: a file replaced, rewritten, cut or gone, an earlier segment grown, a
  takeover (which can turn an earlier end of its claim stale), a record not its machine's own or
  not new, a new machine's journal, a cause behind the frontier, or a journal with anything replay
  reports. The answers are replay's: a differential test checks every batch a store takes in
  against replay, on histories of machines that write, sync and race. It is written back
  atomically (renamed into place whole, not fsynced: the durability bullet below says why that
  suffices, roll-lqjj.27, X05) when every segment ended with a whole line, never into a store that
  is not there; a projection missing, cut, damaged (a 128-bit sum of each part) or another build's
  is never read, and a failed write is no failure. A state read from it knows, of its events, those
  it names: `State::saw`, `holds` and `events`, which no command reads, answer for those alone. Each
  issue's texts (its long fields, their registers' values, its comments and its extras) are kept
  apart (roll-lqjj.6), in `<store>/projection-texts`, each issue's as bytes of their own the state
  places and sums, the state keeping what a list and a later event need of them unread (the events
  behind each long field's register, which of those fields two writers set differently). A long
  field's value is kept once, in its register, which the field shows (roll-lqjj.10: the entry of the
  largest id, absent when that is a later write's null, null when the create left it so), never
  copied beside it, so the state holds each text once and the text file writes it once: a generated
  49 MB export's text file went from 75.0 MB to 43.3 MB, and the first sync that makes its
  projection from 245 MB on the heap to 191 MB (`cargo xtask timing --import 14000`, whose records
  now hold `projection_bytes` and `texts_bytes`, as `--read N` does). A command that prints no texts
  (`list`, `ready`, `blocked`, `claims`, `dep`, `info`, a transition's or a release's answer) never
  reads them; one that prints issues whole (`show`, `comments`, `update`, `claim`, `takeover`) reads
  in only theirs; `export`, `search` and `lint` read every text; and records applied after the
  projection have the texts of the issues they are about read in first, a change to texts not read
  in declining. Texts that cannot be read as written (a sum, a length, a generation, the events
  behind a register that differ) have the journal replayed and both files written again. Texts are
  only appended, under the text file's own lock, the projection written after them, so that a bead
  killed between the two leaves the last projection (a bead killed: after a crash of the machine the
  order promises nothing, which the durability bullet covers); a projection made whole, or whose
  file holds more texts no longer read than read (beyond a mebibyte), starts the file again, under
  the generation its texts give, so that the same texts make the same file. A merge that brings
  events written concurrently with the store's replays the journal, and stays so (roll-lqjj.8, by
  the maintainer's direction of 2026-10-12: correctness before speed): it costs what a replay does,
  0.45 s at 50,000 events and 1.0–2.0 s at 200,000 (`cargo xtask timing --merge N --concurrent`),
  and comes in about a merge in ten where two machines write. Taking such events in without
  replaying would mean reproducing what replay settles by the order it applies events in (the key
  two concurrent creates minted, the order of comments, claim history and issue order), the question
  a checkpoint's continuation faces too (roll-lqjj.4).
- `log`, `info` and `machines` answer from the projection (roll-lqjj.31, X09). `info` and
  `machines` read nothing else of the journal: the issues, the waiting events, the integrity
  problems and the renamed keys are its state's; the events (every id, an attempt file's and a
  stand-in's among them), each machine's journal length and the frontier its index's; the
  segments the files' listing; the attempts awaiting the remote this machine's pending files.
  `machines` also says when each machine last wrote, its greatest event id, which its last record
  need not have since ids are minted from a clock that can go back (V02): the index keeps it
  beside the journal length, taken from the journal's order by `project` as the lengths are and
  advanced with them by `extend` (`projection::ENCODING` 5). `log` reads the whole journal still,
  since it lists every event in causal order, but takes the keys it names issues by from the
  store's state (`view_outline`), read after the journal so that every issue an event names is
  there, where it replayed a clone of the journal; the two can come from two moments when a
  writer appends between them, which shows a newer key and nothing else. Every output is the
  same, byte for byte (`commands::history`, which passes on the bead before; and the bead before
  and after compared on three machines' tracker with renamed keys, an attempt file, a pending
  attempt and a damaged line, cold, warm, after a write and after a merge). At 200,000 events
  (`cargo xtask timing --read 200000`, plain, release, best of five, before and after back to back
  on a machine shared with ten other builds) `info` took 3.30 s and takes 0.23 s, `machines` 2.40
  and 0.23 s, `log --limit 1` 3.78 and 3.21 s, where `list` takes 0.21 s; at 20,000 issues (an
  imported forest, best of five) 1.16 and 0.21 s, 0.88 and 0.22 s, 1.37 and 1.24 s, at half the
  memory for the first two (213 to 114 MB resident). With no projection, as after an upgrade or
  with a damaged one, each makes it as every command does, replaying the journal and writing the
  projection back, so the first `machines` costs a replay it did not before (2.16 to 3.76 s at
  200,000, `info` 6.20 to 3.91 s), and the first `log` reads the journal twice (5.02 to 6.25 s).
- A write or a sync reads the store's state where it needs it, and no more (roll-lqjj.29, X06): a
  write once for its transaction, a sync once a round, by its merge, and `bead sync` once more when
  something comes in. An online write read it four or five times: for its transaction, for
  `update`'s answer, to count the keys renamed before the sync that publishes it, for the merge's
  check of this machine's own chain, and for the sync's report, which that sync threw away. A
  write's sync (`publish_soon`) now makes no report and counts nothing. A merge keeps the
  projection it reads for its check (`Link::merged`): the report of a sync that goes through is
  made of the state its last merge left, under the lock and before the publish rather than after
  it, which differs only by what another process writes in between, and an ownership round's
  transaction starts from it (`Tx::on`), since nothing between the merge and the transaction, under
  the same lock, writes the journal. A sync that reports (`bead sync`, `init` joining a tracker)
  counts the renames before it from a projection its first merge reads once the torn tails are cut
  and before it merges, recorded at once so that a merge that fails later keeps the count, and goes
  on with that projection when the merge writes no journal file (each of its journal writes marks
  the merge: `write_journal`, `wrote`). `update` answers from the state its transaction leaves
  (`writing_state`), this machine's pending attempts listed on it as `view` lists them, unless it
  publishes (a close of its own claim), whose merge may bring in more: then from the store. A
  command that reads the state while it emits borrows it beside an `Emitter` (`Tx::lend`) and
  reads it as it was before, as the copy it took did; the next state asked for has every event
  emitted, a state replayed being replayed again once more are. At 20,000 imported issues
  (`scripts/setup.sh` of review 16, release builds, best of three, a machine shared with ten
  other builds), the reads and seconds of an online comment, update, close, create, `dep add` and
  claim went from 4, 5, 4, 4, 4 and 3 to 2 each (1.05, 1.39, 1.06, 1.05, 1.24 and 0.97 s to 0.72,
  0.75, 0.78, 0.79, 0.81 and 0.72 s), and a sync that brings nothing from 3 to 1 (0.60 s to
  0.27 s). The `projection.read` failpoint counts them (`beadroll-sync`'s `reads::` tests).
- A projection extended finishes only what the batch changed (roll-lqjj.17, Z02). Replay finishes a
  whole journal by deriving each issue's labels from the tags that remain and building the
  parent-child index from every issue's edges; a projection that redid all of that for every batch
  cost the whole state each time, so a command that writes one event per issue and reads the state
  between them (`close`, `reopen` and `delete`, over ids or a subtree) cost the state once per
  issue. It now relabels the issues whose operations change labels (the three label operations, and
  a create, which carries an import's), and builds the index again only when an operation changes a
  dependency edge (`dep_add`, `dep_remove`, and a create, which brings an import's edges and gives
  the imports that waited for its key theirs). Each is listed one by one, so that an operation
  added says which it is. Each issue is relabelled once however many of the batch's events change
  its labels, since only the labels it holds once they have all applied are derived from (V01). The
  ids the projection holds are a set, so a batch's go in where they belong at a cost of their own
  rather than the whole being sorted again: ids minted in one millisecond sort by their random
  bits, and a machine whose clock rolled back, or a peer whose clock ran ahead, leaves new ids
  below every id held, where a merge into a vector would have moved its whole tail on every write
  (V02). The bytes are unchanged — the set is written and read as the ascending sequence it always
  was, and bytes that are not ascending are still refused rather than quietly sorted. The state,
  the order of everything derived and format 1 are unchanged, which the differential test against
  replay keeps.
  One batch that changes an edge still has the whole index built again, proportional to the state
  and not to the batch: that is one pass a command, as no command writes many dependency events
  while reading the state between them (an import plans and writes its batch whole), and taking an
  edge out of the index without rebuilding it would mean keeping each issue's parents beside it to
  know which bucket to leave. `cargo xtask timing --extend 8000` measures it on 8,000 issues, each
  with two labels and a tenth of them a parent of nine: 8,000 closes one event at a time took
  17.97 s before and 0.048 s after, against the same closes in one batch, 0.039 s before and
  0.037 s after; 8,000 label additions one at a time, 18.68 s before and 0.050 s after; 8,000
  dependency additions one at a time, 18.59 s before and 5.33 s after, which is the index built
  again 8,000 times and what the closes no longer pay. It measures the two shapes a batch takes
  when it is not one event per issue in clock order too: 8,000 label additions to one issue in a
  single batch, 6.32 s before the labels were derived once an issue and 0.129 s after, and 8,000
  closes one at a time whose ids all sort before every id held, 0.235 s with the merged vector and
  0.048 s with the set, which is what the same closes in clock order cost, so where the new ids
  sort no longer matters. The fastest of three runs each, 2026-10-14, Rust 1.99.0, 16 cores. End to
  end, `bead close` naming all 8,000 of those issues went from 19.8 s to 0.7 s, the best of three
  runs each.
- An imported dependency on a key no issue has yet is found by that key (roll-lqjj.18, Z03). Replay
  kept the issues that waited for keys and, on every create, visited each of them and read its whole
  list of waiting dependencies, so N imports that each waited for a key cost N² however few keys
  ever came, and one issue waiting for N keys had its list read again as each came. The state now
  lists every waiting entry by the key it names, its issue and a number fixed when its list began to
  wait, its place then (`state::Awaiting`, ordered sets, so no hash order reaches anything): a
  create takes the entries its own key settles and visits nothing else, and a removal by key finds
  an issue's entries for that key the same way. An entry taken out leaves null in its place while
  events apply; replay's finishing pass, and a projection's for its batch, make each such list whole
  again in its order and drop the numbers of the entries taken, and the entries left keep theirs, so
  none is listed again (W01: listed by place, each batch had every entry behind the first one taken
  listed anew). Nothing outside replay sees a null: the one reader inside it, the claim on a key
  (`claimed_keys`, or `State::claims_key` for a copy renamed mid-batch), finds no key in one, as it
  found none in an entry removed. Binding is what
  it was — the key actually assigned after a rename, an issue's own key left waiting, the type a
  falsy one defaults to, the import's metadata and its create's tag, and removals by key and type or
  by target with their causal rules — and so are the order of edges and of what still waits; the
  scenarios in `projection::tests::awaiting` and the digest of 30 seeded histories pass unchanged on
  the implementation before it. The index is derived, like the parent-child index: a projection's
  bytes no longer carry the issues that waited (`projection::ENCODING` 4), and a state read back
  lists its entries again from the issues' lists, numbered from each list's first entry, so that one
  read back, one extended and one replayed list the same entries at the same places. `cargo xtask
  timing --dangling N` measures replay of N creates, at 8,000 and 16,000 before and after, run back
  to back: none waiting (the control), 0.021 s and 0.045 s before, 0.019 s and 0.046 s after; each
  waiting for a key that never comes, 4.50 s and 30.28 s before, 0.044 s and 0.093 s after; the
  first half waiting for keys the second half brings, 2.02 s and 7.94 s before, 0.038 s and 0.084 s
  after; one issue waiting for every other's key, which come one create at a time, 1.57 s and 9.05 s
  before, 0.24 s and 0.78 s after. That last still grows with the square of N, and its control says
  why: one issue given the same N edges at once, by an import naming keys that have their issues,
  takes 0.145 s and 0.500 s before and 0.141 s and 0.498 s after. An issue's edges are a vector
  searched in order (`state::small`, roll-dih8.11), so its N-th edge costs N whichever way it comes,
  `dep_add` included; with that insertion left out, as an experiment in a quieter run that morning
  (12.41 s before and 0.056 s after for keys that never come at 16,000), the shape took 0.034 s and
  0.066 s. It measures a projection taking that issue's keys one create a batch too, as a store
  takes them a command at a time: 1.26 s and 5.08 s before, 10.13 s and 50.68 s with entries listed
  by place (W01), and 0.38 s and 1.52 s after, against 0.137 s and 0.450 s for the same creates with
  nothing waiting, which is the parent-child index built again each batch; what remains is the
  issue's growing edges and a copy of its list's tail once a batch. The fastest of three runs each,
  2026-10-15, Rust 1.99.0, 64 cores.
- A copy two machines minted alike is renamed by asking of each key it tries (roll-lqjj.25, X03).
  `State::free_key` collected `claimed_keys()`, every issue's key and every key an import still
  waits for, once for each copy replay renamed, so that N copies cost N times the tracker: two
  machines importing one export of 8,000 records offline and syncing renamed 8,000 copies, and the
  second machine's merging sync took 6.6 s and a cold replay 4.7 s, against 1.15 s for the same
  creates with nothing renamed. It now asks `State::claims_key` of each candidate, which is `by_key`
  holding it or the awaiting index naming it (`Awaiting::names`, one range lookup in the set
  `arrived` and `retract` already keep): the keys tried and the key chosen are what they were.
  `claimed_keys` is public and keeps its answer, and `mint_key`, which asks once a command, still
  collects it. The index lists what `claimed_keys` reads in the issues' lists: an entry is out of
  the index the moment it is taken, as null in a list names no key, so a copy renamed in the batch
  an entry is taken out of, before the finishing pass makes the list whole, gets the same answers.
  In this crate's tests, `claims_key` checks every answer it gives against `claimed_keys` and keeps
  each ask (`state::asked`): replay and a projection extended through the scenarios of
  `projection::tests::awaiting` and the 30 seeded histories ask 92,277 keys, 484 of them claimed
  only by an import that waits and 87,431 while an entry taken out holds its place (replay makes
  the lists whole once, at its end); the digest of those histories and every rename test pass unchanged on the implementation before.
  `cargo xtask timing --renames N` replays N copies renamed against 2N creates renamed by nobody,
  and each again with the first machine's issues importing a key that never comes; at 2,000, 4,000
  and 8,000, before and after, run back to back and the fastest of three: 0.159 s, 0.608 s and
  2.50 s before, 0.012 s, 0.028 s and 0.058 s after, against 0.006 s, 0.015 s and 0.035 s for the
  control; with the imports waiting, 0.51 s, 2.14 s and 13.9 s before, 0.017 s, 0.033 s and
  0.071 s after, against 0.010 s, 0.024 s and 0.050 s for their control. The commands, end to
  end: two machines import the same 8,000-record Beads export offline, each minting every key, and
  the first syncs; the second's merging sync renames all 8,000 copies, 7.0 s before and 3.0 s
  after, and a cold `list` (its projection deleted, so that it replays the journal) takes 5.0 s
  before and 1.2 s after; `cargo xtask timing --renames` measures the same replay in process.
  The same 16,000 creates with nothing renamed replay cold in 1.15 s. `list --all --json` and
  `export` are byte for byte the same (3,000 copies, both bead builds on one store). Measured on a
  machine shared with ten other builds, so every figure is inflated; those compared were taken
  together.
- Readiness is judged once for every issue (roll-lqjj.19, Z04). `bead ready`, `bead blocked` and
  `bead prime` asked of each issue whether anything holds it, and the answer was the blocker list
  `open_blockers` builds — the issue's own prerequisites, an epic's unfinished children, and the
  prerequisites of every ancestor that is not done, found by walking up its parents — tested for
  emptiness, so a chain of N issues under one held root walked N²/2 ancestors and sorted N lists to
  say that everything in it waits, and the CLI's `ready` asked the same way on a path of its own.
  `State::readiness` now judges them all in one pass: the issues that are not done and have a
  prerequisite of their own are found, then every issue that is not done and reaches one of them
  through parents that are not done, by one walk down the parent-child index from them, and an
  issue is held when it has such a prerequisite, is an epic with an unfinished child, or has a
  parent so reached; each issue, edge and child is visited once, a merged cycle once, and nothing
  recurses. `State::ready` and the CLI's `ready`, `blocked` and `prime` ask it once a command. The
  rule is unchanged — a `blocks` target counts only while it is not done, a disputed status is
  never done, only an epic waits for its children and nothing beneath it inherits that wait, a done
  ancestor's prerequisites are not inherited and the walk does not pass it, every parent link
  counts, and a scope restricts what is listed, never what holds — and three answers must agree:
  judged with every issue, asked of one issue by the first thing found (`State::is_blocked`, what
  `bead show` asks of its one issue, which no longer builds the list either), and the list itself,
  which `replay::readiness` checks on deep chains, shared and second parents, merged parent cycles,
  done and disputed ancestors and targets, epics beside plain parents, claims, scopes and 80 random
  graphs, and `commands::shared_readiness` across the four commands. The lists were still built
  where they are printed then, and the details of several issues still judged `blocked` issue by
  issue; both are made once for all of them since (roll-lqjj.24, below). `cargo xtask timing
  --ready N` measured four ways on a chain of N issues held by its root's prerequisite,
  the same chain with nothing waiting, and a lattice of rows of four each the child of two in the
  row above, replay left out: judged for every issue at once, the held chain takes 0.9, 1.8, 4.3
  and 11.2 ms at 2,000, 4,000, 8,000 and 16,000 issues, against 0.43, 1.74, 7.12 and 33.3 s by the
  lists, which is what every reader built before (the review's figures of 2026-10-13 were 0.41 and
  1.68 s at 2,000 and 4,000), and 0.26, 1.00, 4.12 and 20.1 s asked issue by issue; the free
  chain, where everything is ready and the answer is sorted, 0.4, 0.8, 2.3 and 10.4 ms for `ready`
  against 0.2, 0.5, 0.9 and 4.6 ms for the analysis alone, the rest the sort of every issue by
  priority, creation time and key; the lattice, 1.0, 2.0, 5.9 and 13.7 ms against 0.63, 2.62, 12.6
  and 90.2 s by the lists, where every issue of a row shares every ancestor above it. End to end,
  `bead ready` on a store holding the chain of 4,000 took 2.05 s and takes 0.037 s, and
  `bead blocked`, which prints the lists, 4.05 s and 2.14 s; on the chain of 16,000,
  `ready` took 37.0 s and takes 0.149 s; on the lattice of 4,000, 3.59 s and
  0.046 s. The fastest of three runs each, 2026-10-15, Rust 1.99.0, 64 cores; the end-to-end
  figures the best of five (of three at 16,000), the commands reading the store's projection.
- The issues that depend on each of several issues shown in full are found once for all of them
  (roll-lqjj.20, Z05). A detail lists as `dependents` every issue with an edge to its issue, and
  only the edges say which those are, so `State::detail` read every issue's edges, collected and
  sorted each one's, and kept those that name its issue's uid; `bead show`, `bead update`,
  `bead claim` and `bead takeover` answer with a detail for every issue named, so N issues read
  and sorted every edge N times. `State::dependents_of` finds them for all the issues named in one
  pass over every issue's edges, keeping only the edges that name one of them — by the text of its
  uid, as an edge names it, so a copy renamed for a key two machines minted is never taken for the
  other — and sorting only those; `State::detail_with` lists each one's from it, and the four
  commands make it once a command, for the issues named. It is a view made for one batch of details
  and dropped with it, not an index the state keeps: a command needs the dependents of the issues
  it shows and of no others, an index of every issue's would have to be built for every state a
  command reads, or kept up to date by replay and the projection (and, stored, change the
  projection's encoding), to save a pass the commands that show issues make once, and that pass
  for all 8,000 issues below takes 8 ms. One issue alone (`State::detail`, which `bead show` of
  one issue, `create`, `comments` and `dep` ask) is still one pass, comparing each edge's target
  with its uid rather than hashing it, and costs less than the pass it replaces, since an issue's
  edges are sorted only when one of them names it. What is listed and its order are unchanged —
  the issues in replay order, each one's edges to it in sorted order, once for each type, every
  type and every status, never a removed edge, nothing for a dependency that names no issue — as
  are the order of the details, an issue named twice shown twice, and the error a key that names
  no issue gives, where it stands among the others; a lookup made in another state is never used,
  and an issue it was not made for finds its own. `tests/core/replay/dependents.rs` keeps each
  detail so made equal to the issue's detail alone and its dependents to what every detail found
  before, member for member, and `commands::shared_dependents` each command's answer to `bead show`
  of each issue alone. `blocked` in each detail was judged issue by issue then
  (`State::is_blocked`, roll-lqjj.19; once for all of them since, roll-lqjj.24, below), which on a
  graph without parents reads the issue's own edges alone. `cargo xtask timing --details N` measures it on a sparse graph without parent-child
  edges — issue n waits for issue n/2, and every third is related to the one before as well —
  replay left out: all the details, their dependents found once, take 20, 49 and 95 ms at 4,000,
  8,000 and 16,000 issues, against 0.49, 1.92 and 8.87 s with each detail finding its own; the
  lookup alone for all of them 3.0, 8.1 and 17.0 ms, against 0.67, 3.42 and 18.4 s as every
  detail made it before (the review's figures of 2026-10-13 for all the details were 0.54 and
  2.33 s at 4,000 and 8,000); for one issue, the lookup 0.11, 0.22 and 1.2 ms against 0.22, 0.40
  and 1.7 ms before, and its whole detail 0.12, 0.23 and 1.3 ms. End to end, `bead show` of all
  4,000 issues of such a tracker took 0.85 s and takes 0.13 s, of all 8,000 3.26 s and 0.28 s,
  printing the same bytes, and of one issue 34 and 69 ms before and after alike, which is reading
  the state. The fastest of three runs each, 2026-10-15, Rust 1.99.0, 64 cores; the end-to-end
  figures the best of five, the commands reading the store's projection.
- Every held issue's blockers are listed once for all of them, and the details of several issues
  judge `blocked` once for all of them (roll-lqjj.24, X01, X02, X12). Z04 judged readiness once
  but left two askers walking ancestors: `bead blocked` listed each held issue's blockers with
  `State::open_blockers`, and `bead show`, `update`, `claim` and `takeover` asked
  `State::is_blocked` of each detail's issue, both a walk up the issue's ancestors, so a chain of
  N issues under one held root cost N²/2 steps in each (Z04's and Z05's probes had no parents, or
  no lists, and did not see it). `State::blocker_lists` makes every list in one pass: the issues
  that are not done are grouped as they reach each other up their parents through issues that are
  not done (Tarjan's components without recursion, a merged parent cycle being one group), each
  group found after every group above it, and what an issue inherits is a set of direct blockers
  kept once: a group that adds nothing of its own to the one set above it shares that set, and a
  union that equals one of the sets above is that set, so a chain's N lists are one set and a
  lattice's grow only by what its rows add. An issue that is done is not passed through, as in
  `open_blockers`: it lists its own direct blockers and what each parent that is not done
  inherits. An epic adds its unfinished children, which nothing beneath it inherits.
  `BlockerLists::of` gives the list, sorted by key, equal for every issue to `open_blockers`
  (`replay::readiness` checks it on every issue of the chain, a lattice that gathers distinct
  gates, merged cycles, done and disputed ancestors and targets, epics and 80 random graphs), and
  `bead blocked` prints it for the issues whose list is not empty, which are the issues
  `State::readiness` holds; `open_blockers` stays the reference for one issue. The details of
  several issues share what they share in a batch type, `Details` (`State::details_of`): the
  dependents `State::dependents_of` finds (Z05) and, when several distinct issues are asked
  about, one `State::readiness` for their `blocked`; one issue's detail keeps walking its own
  ancestors, which costs less than judging every issue, as its dependents are found by comparing
  edges rather than hashing. `State::detail`, `detail_with`, `dependents_of`, `readiness`,
  `open_blockers` and `is_blocked` keep their signatures and answers for pancake-studio and
  everyone else: `detail_with` judges its issue alone, and a caller that shows several issues makes
  `details_of` and asks `Details::of`. Readiness was not put inside `Dependents`, as the
  prototype did, because that type's name and its `detail_with` argument say they are the
  dependents. The sort `bead list` and `bead ready` list by (`sort_for_listing`) compares the
  creation times it holds as borrowed text rather than copying one per comparison. What is
  printed, its order, the order of the details and the errors are unchanged: `bead blocked` and
  `bead show` of every issue of the stores below print the same bytes before and after, as text
  and as JSON. `replay::readiness`, `replay::dependents` and `commands::shared_readiness` keep the
  answers; `cargo xtask timing --ready N` measures the lists made one issue at a time and all at
  once, on the chain, the lattice and a lattice whose first sixteen rows wait for gates of their
  own, so that every issue below lists up to 64 of them, replay left out: the held chain's lists
  take 0.5, 1.1, 2.4 and 7.0 ms at 2,000, 4,000, 8,000 and 16,000 issues against 0.45, 1.83, 8.26
  and 46.2 s one by one; the lattice's 0.6, 1.3, 6.3 and 11 ms against 0.65, 2.73, 20.6 and 85.3 s;
  the gated lattice's 4.1, 8.9, 23 and 43 ms (121,600, 249,600, 505,600 and 1,017,600 entries
  listed) against 0.63, 2.66, 18.8 and 86.4 s; and the `blocked` of every detail, judged for every
  issue in 1.0, 2.5, 6.0 and 15.8 ms on the chain, takes 0.25, 1.04, 4.51 and 28.3 s asked of one
  issue at a time (the lattices 14 s at 16,000). End to end on scratch stores of imported issues,
  `bead blocked` and `bead show` of every issue, before and after: a held chain of 4,000, 2.13 s
  and 1.52 s become 0.068 s and 0.152 s; of 16,000, 39.2 s and 27.1 s become 0.28 s and 0.62 s; the
  gated lattice of 4,000, 3.63 s and 0.77 s become 0.139 s and 0.209 s; of 16,000, 81.3 s and
  16.9 s become 0.45 s and 0.70 s; what remains is the output (7.8 MB of text for the last
  `blocked`) and reading the state. Sorting 20,000 issues for a listing takes 0.95 ms where it
  took 2.1 ms. The fastest of three runs each (of forty for the sort), 2026-10-15, Rust 1.99.0, on
  a 64-core machine ten other builds and measurements shared, so the absolute figures are inflated
  and each pair was taken back to back.
- What roll-lqjj.15 to .20 made grow with its input is guarded by counting its work, and by time
  only where a count cannot see it (roll-lqjj.34, X15). Their tests compare answers, which a
  quadratic implementation gives as well, so beadroll-core counts the work where every way of doing
  it passes (`work::tally`, compiled into this crate's own tests alone, `cfg(test)`, so no build of
  bead carries it): an issue's edges when they are handed out, a parent's children, an issue
  relabelled, the parent-child index built, a waiting import's key read and the entries the waiting
  index hands out, examines or makes whole, and an import's record and dependency keys read. A
  guard for each algorithm (`work::tests::import`, `finished`, `awaiting`, `edges`) counts its work
  on the shape its `timing` probe measures, at 250 and at 2,000, and fails above sixteen times as
  much: twice what linear work does, a quarter of what quadratic work does. Counting rather than
  timing makes the guards exact, fast (0.25 s for all of them, debug) and the same on every machine,
  so they run on every push. Fourteen quadratic versions — `Readiness` and `ready` asking
  `is_blocked` of each issue, a detail finding its own dependents, `dependents_of` passing once per
  issue asked about, `finish_some` finishing the whole state, building the index for every batch or
  relabelling once per event, a create reading every waiting list, a removal by key reading its
  issue's whole list, every waiting list made whole each batch, import planning, its dry run and a
  re-import scanning the batch — each fail their guard, with 16 to 64 times the work where the code
  on main does 8. A count sees only work that passes the place it is kept: a lookup turned back into
  a scan of a vector reads nothing counted. Those are timed: `xtask`'s growth tests time planning an
  import, its dry run, a re-import, replaying imports that wait for keys and a projection taking
  ids that sort before every id it holds, the fastest of five runs at a size and at eight times it,
  and fail when the larger takes more than 24 times as long, about three times what linear work
  takes and well under the 64 of quadratic. They are the one exception to "timings never gate"
  (`xtask/AGENTS.md`): ignored by every other run, so that no pull request or `./check.sh` waits on a
  timing, and run by the nightly workflow in release, one at a time (`ci::x15_growth_guards` keeps
  the step). On main they measure 8.4 to 13.6 times; the scans of roll-lqjj.15 restored measure
  30 to 60, a create that reads every waiting list's keys inline 84, and every id moved on each
  batch, as the merged vector of V02 moved them, 59 (64 cores, shared with ten other builds,
  2026-10-15). Not guarded, since they grow faster than their input by design, as the bullets
  above say: a batch that changes an edge builds the parent-child index again, and one issue that
  gains N edges, or waits for N keys that come one batch at a time, searches its own edges and
  copies its list's tail. What X01 to X03 found still quadratic on main (`bead blocked`'s lists,
  `blocked` in each of several details, a renamed copy's free key) gets its guards, which fail on
  main, once the changes that remove it land; roll-lqjj.34 records them.
- No history boundary (roll-lqjj.3, step 4 of epic roll-lqjj; the maintainer's decision of
  2026-10-12, on the evidence in the port plan's Appendix A, "roll-lqjj.3", which git keeps, §1): a
  machine's work merges by itself however old it is, as every merge has so far. The alternatives
  were a supported window (in events, segments or days) behind which a machine refreshes and
  reconciles by hand, its pending work kept but not merged, as Microsoft's Sync Framework treats
  a replica behind what it has forgotten; and leaving the question to the checkpoints of step 5.
  Work that old is real: bead
  published a tracker's first events 45 h after they were written, and in Beads trackers kept in
  git a branch's tracker changes waited up to 24 days on the maintainer's and up to 258 days in
  public repositories; a window short enough to save anything would send such work down the manual
  path, and pruning causal history without one (CouchDB's revision limit, Riak's clock pruning)
  turns it into spurious conflicts. What a window would save is small: answering whether a late
  event saw an old one takes an index of every event's id and place, 22–30 bytes an event in the
  projection on real histories (1–2% of their journals; 5 MB at 200,000 events). What it fixes for
  step 5: a checkpoint carries that index over all history and the accepted attempt ids (a retry
  is recognised by its file in `claims/`), and an archive keeps every raw record for the fork,
  quarantine and chain-break checks. A tracker rolled over to checkpoints is format 2, which ends
  the python-0.1.0 rollback for it, boundary or not. Continuation from a
  checkpoint then becomes the reference for late concurrent events, not full replay: the key two
  concurrent creates minted
  (`same_child_number_on_two_machines_is_renumbered_identically` pins the rename today), the order
  of comments, claim history and issue order follow the order events are applied in. So step 5
  tests that every machine continuing from one checkpoint agrees, and that continuation equals
  replay on every history without late concurrent events. Those are common where several machines
  write (a merge in ten on a tracker two machines write) and conflicting edits of one field rare
  (status and comments). Until then nothing changes, and the stress models' partitioned machines
  (`six_machine_2400_operation_reference_model`,
  `six_machine_3000_concurrent_commands_with_partitions`) and the protocol sweep keep pinning that
  every machine converges whatever it missed. Retention (step 6) is a separate question: forgetting
  closed issues changes what `show`, `log` and `export` return and breaks the Beads round trip for
  them, so it would be a policy a tracker sets explicitly, with a command that lists what it forgot
  and the archive keeping it; step 5 leaves room for it by keeping the index and the archive apart
  from the state. Closed issues hold 27–100% of the largest public trackers' bytes; with texts kept
  out of what most commands read (roll-lqjj.6), what retention would still save is disk, a fresh
  machine's fetch, and the commands that read every text (`export`, `search`, `lint`).
- An import holds no more than a cold start does (roll-lqjj.9; the maintainer's budget of
  2026-10-12): its heap peak stays within what making the projection of the imported journal
  takes, which a published import does in its own publish, an offline one in the first command
  after it, and every new machine in its first sync of the tracker. A generated 49 MB export
  (14,000 records) peaks at 165 MB offline and at 191 MB published, the projection's making, where
  it peaked at 473 MB, and at 245 MB before roll-lqjj.10 kept each long text once (`cargo xtask
  timing --import`). Each form of the
  export is let go of once the next is made, the transaction keeps each event once and lets go of
  it as it is encoded, `init` reads the prefix before it imports, so that its state never takes
  the imported events in, and a text file is written from its parts. A tighter budget was not
  chosen, nor its work: the planner still copies the records into its plan, and the transaction
  holds each event as a JSON value until it is appended. An import's events reach the journal in
  one write, once every record is checked and every event emitted, never as they are made: a torn
  append keeps every whole line it holds, so events written as they were made would let a crash
  keep part of an import. bead's heap is counted by running bead in xtask (`cargo xtask bead`,
  with xtask's counting allocator, below), the reason xtask depends on the `beadroll`
  library.
- A command keeps the state it answered from until its process exits, and a write frees its
  projection after the store lock (roll-lqjj.32, X10). A reading command decoded the state, printed
  from it and freed every issue of it just before the process ended: at 200,000 events decoding
  takes about 100 ms and freeing 50 to 85 ms, work nobody waits for once the answer is written.
  `Outcome::answered_from` hands a command's last state to the command line, and `run`, the
  process's own run, leaks it (`cli::ended_with`) rather than freeing it. Only the state a command
  answers from is handed over, the last it makes, so that no command holds two because of it: a run
  that publishes what the command wrote (`wrote()`, not `--offline`) frees it first, as a publish
  reads a state of its own, and `cargo xtask timing --import 14000` peaks at the same heap before
  and after (165,555,895 and 165,554,920 bytes offline, 191,873,888 and 191,872,068 published). A
  state a command makes and does not answer from (`transition`'s check for a superseded claim, the
  state `still_held` reads) is freed as before. Libraries never leak: `run_with`, which `cargo
  xtask bead` and in-process tests call, frees the state when the run ends, so a test that runs
  bead twice in a process accumulates nothing, and `writing`, `coordinated`, `view` and
  pancake-studio's use of `beadroll-core` keep their ordinary lifetimes. `log` and `machines`,
  which load the journal beside the state they replay, are not given the rule here (X09 changes
  them). The other half is the write's: `writing` and each round of `coordinated` held their
  transaction, which owns the store's whole projection (83 ms to free at 200,000 events), inside
  `locked`, so every command waiting for the store lock waited for the free too; both now keep the
  transaction outside the closure and drop it after the lock is let go of, and it is freed, not
  leaked, there, since a library does not know its process ends. Nothing observable changed: the
  tests pass on both implementations, and the probe is the evidence (`cargo xtask timing --read N`,
  best of three runs of five each, run back to back on a shared machine: at 200,000 events `list`
  0.203 s before and 0.150 s after, `show` 0.203 s and 0.158 s, `ready` 0.261 s and 0.200 s, and a
  local write 0.281 s and 0.290 s, which still frees its projection before the process ends; at
  20,000 events `list` 23.0 and 21.1 ms, `show` 23.3 and 20.7 ms, `ready` 25.7 and 24.2 ms, and a
  write 52.9 and 52.7 ms). The time other commands wait for the lock is not in those figures: it is
  the free that moved out of it.
- What each event had seen is one clock per event over chains, the reachability index of
  `state/seen.rs`: every machine's journal is a chain, and each event of an
  attempt file joins a chain whose last event is among its ancestors, or starts one, so there are
  about as many chains as machines that write attempts. A clock holds the furthest position the
  event saw on each chain, and `saw` is `clock[chain] >= position`. Memory grows with events ×
  chains, not with events × ownership events as Python's bitsets did: 200,000 events, 55% of them
  in attempt files, replay in 0.29 s holding at most 133 MB on the heap, 116 MB of it the loaded
  journal (roll-dih8.11). The clocks live in one arena of positions alone: a clock names the set
  of chains it has seen into, and each set is kept once.
- Exported comment ids for comments made here come from a 48-bit hash range; a collision is a
  one-in-10^14 event and is then resolved by the ids alone.
- `dep cycles` stops after 200 cycles.
- `ready ID` and `ready --epic ID` are one scope, ID's subtree, recursive as br's `--epic` (the
  maintainer's decision of 2026-10-09; until then `--epic` was another name for `--parent`): every
  issue beneath ID through parent-child edges, at any depth and through every parent link, whatever
  lies in between; ID itself is never part of it. Readiness is judged on the whole tracker.
  `--parent ID` stays ID's direct children, as in br and `list --parent`. One scope at a time, each
  option given once (`USAGE`, as br refuses both), resolved before anything is filtered.
- A repository with neither a remote nor `.beadroll.json` gets a local tracker that cannot sync.
- Setting a tracker up has one path (the maintainer's decision of 2026-10-11, roll-dih8.13): the
  URL-derived workspace, nothing committed; a tracked `.beadroll.json` only for a coordination
  remote other than origin or for `"where": false`, committed on the default branch. `bead init`
  asks the remote first (`git ls-remote`, as a link asks, from the checkout, writing nothing in
  the store): a tracker already published there is joined, adopted as `sync` adopts it when it
  was published under another id, and once the remote has said it publishes one, any failure to
  join it fails `init`, which then starts nothing. One it cannot adopt is refused
  (`WORKSPACE_MISMATCH`), even under an explicit `BEAD_WORKSPACE`, since that tracker could never
  publish there. `init --tracked` names the tracker this checkout has, joined just now or here
  already (a tracked id the remote publishes, one an earlier `init` adopted), and mints a new id
  only where there is none, since a new id beside it would be a second tracker; it never pins a
  URL-derived one (`CONFIG`): that id would stop matching when the URL changed, and the file
  would then be refused as another repository's. An unreachable remote, one that publishes
  nothing, `--offline` or no remote: `init` starts the tracker as before, and publishes it only
  to the remote it chose, never again after it through `--remote` as typed. `info` and `prime`
  warn while `.beadroll.json` would not reach the other checkouts, comparing it with its commit as
  JSON, so line endings git converts are no change; where the tree lacks it and HEAD's copy is in
  use, that copy is checked against origin's default branch.
- `init` acknowledges a successful publication in human output with the remote URL and the
  coordination ref (roll-dih8.18). The acknowledgment follows a write and a successful publish
  to a nonempty remote; an offline run, a missing remote, a failed publish or simply joining or
  reusing a tracker does not claim a push. JSON output keeps its existing fields. The tracker
  golden's added line is an intentional output change.
- `log` names an event's actor as claims and `actor_label` do: the name `--actor` or
  `BEAD_ACTOR` gave it, else agent and thread, never the agent and thread alone when a name was
  given; `log --json` rows carry `name`, null without one. A `set` row's operation names the
  fields it set (`set notes,priority`), the JSON `op` staying the event's (roll-dih8.19).
- A `close`, `reopen` or `delete` writes no event when another would change nothing: the issue
  already has the status it gives (`closed`, `open`, `tombstone`), holds no claim or provisional
  claim for the event to end, and its status register is undisputed. It answers `unchanged`
  ("already closed"), exit 0, and the reason the issue has stays; a reason given then is reported
  as not recorded. The repeat is judged on this machine's view, as every write is: a status
  another machine changed unseen is not written over, so no conflict is made, and a disputed
  status still takes the repeat, which is the write that settles it. A repeat of an own close
  still publishes when that close ended a claim (`ended_by`), as before. `update` that names no
  field, status, label, parent or claim writes nothing and answers `unchanged`. Setting a field to
  the value it shows is still written: such a write settles a conflict, and comparing values
  would hide one (roll-dih8.20).
- `close --recursive ID...` closes each ID and every issue beneath it (roll-dih8.4): the scope
  `ready --epic` takes, through every parent link, so an issue that also has a parent outside is
  closed with the rest. One write, children before parents (siblings as they were created, roots
  as named); the order changes no state, only what `log` shows. An issue beneath that is deleted,
  or closed with nothing to end, is left as it is and counted in one line: closing a deleted
  issue by name brings it back as closed, and the subtree must not. A repeat still publishes this
  actor's close that ended a claim beneath, as a plain repeat does. A named ID answers as a plain
  `close` does. Someone else's claim anywhere in it refuses the whole command, naming every such
  issue, so that one look settles them, a deleted issue's too (a delete made without seeing a
  claim leaves it); `--force` closes them as it closes one. Only `close`
  takes the flag: `delete` and `reopen` keep acting on what they name.
- Help pages keep argparse's layout and formatter but no longer its empty texts: a
  command's page takes its line from the command list as its description, every argument has a
  help text, and `create`, `dep` and `comments` end with an epilog whose paragraphs wrap as the
  description does and whose indented lists of forms are kept line for line, as
  `RawDescriptionHelpFormatter` keeps them. Placeholders say what they take (`ID`, `FILE`,
  `TEXT`, `N`, `LABEL`), so argparse's errors name them too (`the following arguments are
  required: ID`); receipt signatures use destinations, which did not change. `bead -h` describes
  bead, where Python's module docstring had leaked in. The help golden's changes are intentional
  (roll-dih8.21). Since roll-dih8.23 clap lays the pages out (below); the texts stayed.
- bead's own texts are laid out when printed, not stored wrapped (roll-dih8.22): the overview,
  `prime` and the help pages' epilogs are paragraphs and rows of heading, term and description
  (`cli/texts.rs`), laid out by `cli/layout.rs` with `textwrap`, by display width, breaking only
  at spaces: never inside a word, after a hyphen, inside brackets, or inside one of a list of terms
  that fits a line. A table's term column fits its longest described term up to 30 columns and
  half the room, a longer term taking its line; with fewer than 20 columns left for descriptions
  the rows stack. The width is `COLUMNS`, else the terminal's on standard output, at most 100 for
  reading; with neither (a pipe, an agent) nothing wraps, and `--json` always carries the
  unwrapped text, so what an agent reads does not depend on a width. That makes these texts
  differ between a terminal and a pipe, as argparse's pages already did; the V8 contract that a
  terminal and a pipe print alike holds for the data commands. clap lays out the command pages
  (roll-dih8.23); an epilog is laid out at their width. `ONBOARD` stays verbatim: it is
  `AGENTS.md`'s text, kept equal by a test.
- The command line is clap's (roll-dih8.23, the maintainer's decision of 2026-10-12): the argparse
  port bought byte parity with the Python bead while both ran,
  and Python is retired. clap is built from the same table (`cli/spec.rs`), and its matches become
  the same namespace: every destination with its value or default, so commands read what they
  read and a receipt signs the same bytes (the parity receipts hold them). Kept from argparse: an
  unambiguous prefix of a long option names it, the last value of an option wins, an option's
  value may start with `-`, a scope is given once. Changed: a positional's value that starts with
  `-` goes after `--` (free text taking such values swallowed mistyped options, and `--offline`
  after a comment); positionals and options mix (`update A --status open B` updates both);
  errors are clap's, with its suggestions, `USAGE` and exit 2 as before, the usage after them for
  a human only; a quoted argument that is not UTF-8 keeps its bytes, `?` for a human and `\udcXX`
  in JSON. `--json`, `--actor` and `--color` are still read before parsing; clap lists them as
  global options and refuses them abbreviated, which it alone would see. Root help (`bead -h
  create`) is the overview.
- Colour (roll-dih8.23): commands give their lines as parts that say what they are (an id, a
  status, a warning: `commands::Line`), never how they look; `cli::paint` escapes each part, then
  paints it when the stream shows colour. Each stream decides alone: `--color always|never`, else
  `NO_COLOR` (not empty) off, `CLICOLOR_FORCE` (not empty or `0`) on, `CLICOLOR=0` or `TERM=dumb`
  off, else a terminal on. `--json` never; a pipe or a file never unless forced. `FORCE_COLOR` is
  deliberately not followed: agent harnesses and CI set it, and their agents parse bead's output.
  Escaping comes before painting, so the only escape sequences bead writes are its own SGR codes:
  G59's tests now hold that help on a terminal is coloured but never shows a code as text, and
  that neither a pipe under `FORCE_COLOR` nor JSON carries one; V8's hold a terminal to a pipe
  byte for byte with `NO_COLOR`, and with colour once bead's own codes are taken out. Test
  machines drop `NO_COLOR` and `CLICOLOR*` from the inherited environment.
- A remote is kept as it was chosen (G22; the maintainer's course correction of 2026-10-11,
  roll-dih8.13: rounds of findings came from resolving the choice to a URL at once and using that
  string for transport, memory, comparison and advice, which made bead re-implement git's remote
  semantics piece by piece). The choice is the name of a remote the checkout configures (origin
  when none is chosen), or a URL or a path as given; it is what `meta.json` remembers, what is
  compared, and what advice gives back (`bead sync --remote origin`). A name is read for its URL
  only to reach the remote: the URL `git push NAME` is configured with (its first push URL, else
  its first URL, a password dropped), given to git as the URL of a remote defined for the command
  (`remote.bead-<token>.url`, and `.pushurl` for a remote's own push URL, through
  `GIT_CONFIG_COUNT`), never as an argument git would read as a remote's name, so that git
  rewrites it as for `git fetch NAME` and `git push NAME` (`insteadOf`, and `pushInsteadOf` except
  to a remote's own push URL), from the top of the checkout. A configured remote's name wins over
  a same-named path, in a value remembered or tracked too, as in 0.2.0. A URL or a path is read
  once: a relative path from the top of the checkout when it is stored and from the cwd when it
  was typed just now, through its symbolic links, whether it exists or not, except a spelling an
  `insteadOf` or `pushInsteadOf` rule is a prefix of (as written or as resolved), which is git's
  as written.
- Two choices are the same remote only as chosen: the same name, or the same URL so read; never
  two spellings taken for one place (`x` and `x.git` are two repositories, and two spellings git
  rewrites to one place are two choices). A tracked remote that is not the one the store last
  went through is refused (`CONFIG`) until changed deliberately (`bead sync --remote`, `bead init
  --tracked --remote`), with one allowance for older versions, which saved a name as git expanded
  it: a tracked name and a saved URL are the same where git's own answers agree (`git remote
  get-url` with and without `--push` for the name, `git ls-remote --get-url` for the URL, every
  rule applied once), each read as the link reads it, a relative path from the top of the
  checkout through its symbolic links, a password dropped as from every URL. Going back to 0.2.0 or Python, a store whose tracked and
  remembered remote are one name is refused there until `bead sync --remote NAME`, as they refused
  a name their own `init --offline --remote` saved.
- Origin is the name `origin`, never remembered: it is looked up each time, so `git remote set-url
  origin` is followed, as a remote chosen by another name is, and reached as `git push origin`
  reaches it, by the URL it is configured with (a relative path, or a spelling a rule rewrites,
  left to git as written), whether it is named or nothing is; only the URL-derived id reads a
  relative path from the top of the checkout first. Origin's own URL (the one `git push
  origin` is configured with) given as a URL spelled as configured (a password dropped from each),
  which git reaches alike, rules and all, is origin, reached as origin is, its own push URL
  included: `init --tracked` stores it as `origin`, and a sync given it forgets it. Any other
  spelling of the same place is a URL of its own (an absolute path for origin's relative one,
  which a rule can send elsewhere), and so is a relative path typed just now, read from where it
  was typed. A remote of another name is never origin, whatever its URL. A remembered value is origin when it is
  that name or, as older versions saved it, origin's URL: spelled alike, or, for two URLs that are
  not local paths and that no rule rewrites, the same repository however spelled (G25: ssh or
  https, `.git`), as the URL-derived id takes them. A tracker published under another id is
  adopted only through origin. `init --tracked` stores a relative path from the top of the
  checkout, and one a rule rewrites as the very spelling that reads back as written (`.`, `..`
  and symbolic links kept).
- Output is UTF-8 regardless of locale; human output escapes control characters; `--json` always
  wins the representation (`create --silent --json` gives `{"id": …}`); a closed stdout exits 0
  quietly.
- Passwords in remote URLs are dropped, never stored or used; the store is private by its own
  file modes (0700/0600), files written into the checkout follow the caller's umask.
  git 2.46 to 2.48 send empty credentials instead of the password of a URL whose user is empty
  (`http://:TOKEN@host`; mended in 2.49): bead drops that password all the same, and the test of
  what git sends skips the shape under those versions, saying so.
- `--dry-run` of `init --import FILE` and of `import FILE` (roll-dih8.26) says what the import
  would do and writes nothing: it plans with the import's own steps (`read_records`, then
  `plan_import`) against the tracker this checkout has here, an empty one for `init`, read by
  replaying its journal (a view would keep the projection it makes), never starts a store and
  never asks the remote (a tracker the remote publishes would be joined, and
  only what it lacks imported: the answer says so). It exits 0 whatever its verdict, which
  `importable` gives, and lists every problem the import would be refused for, where the import
  stops at the first (`import::dry_run::problems`, empty exactly when the import goes through).
  In a git checkout it names each branch, local or remote-tracking, with commits touching
  `.beads/issues.jsonl` that HEAD does not have (`rev-list --count HEAD..REF -- :(top)FILE`, one
  git a branch, the file at the top wherever bead runs), and each other worktree with changes to
  that file not committed (`git --no-optional-locks status`, which never writes its index back): Beads keeps its
  tracker on every branch, and an import only adds issues, never another copy's newer edits. A
  name that is not UTF-8 is shown with `?` for each such byte.
- An import that contains one invalid record or dependency fails whole; nothing is written. A
  status bead does not set itself (`pinned`, a custom one) is kept verbatim, as Beads tools keep
  it; such an issue is neither ready nor done.
- An import resolves a dependency by looking its key up, never by scanning the batch
  (roll-lqjj.15). The batch's new keys are indexed by the number of the create that makes each
  one, and a known record's listed dependencies by `(key, type)`, that index built only for a
  record whose issue still holds unresolved ones. Planning and dry-run validation ask one
  function whether a key will have an issue, so that the payload a dry run checks is the payload
  the import writes. The hashes are only for lookups: creates and additions follow record order,
  retractions follow the stored dangling order, and diagnostics keep their validation phases and
  record numbers. Dependency lookup work is expected to grow with the records and dependencies
  examined, rather than their product; types and metadata keep their meaning.
  `cargo xtask timing --plan 16000` measures it: 16,000 records, each depending on a key at the
  end of the batch, took 0.896 s to plan and 0.452 s to validate for a dry run before, and
  0.049 s and 0.071 s after (validation reads the file itself, so its figure includes parsing).
  Re-importing one known issue with 16,000 unresolved dependencies still listed fell from
  0.205 s to 0.001 s. The fastest of five runs each, three runs of the probe on 2026-10-13,
  Rust 1.99.0, 16 cores; they measure the core calls, not an end-to-end import, which
  `timing --import N` measures.
- Text from files and stdin is UTF-8 regardless of locale; OS errors are structured (`IO`); a
  git child runs in a separate session without the controlling terminal. A timed-out fetch is
  killed with its process group; a timed-out push kills only the client so its uncertain result
  can be settled by the protocol.
  The runner's timeout starts after process creation. Its hygiene test checks timeout errors,
  descriptor and zombie cleanup, and surviving push helpers; aggregate process-startup speed is
  not a correctness assertion. Git's server-side ref-race diagnostics can say either
  `incorrect old value provided` or `failed to update ref`; the locale test accepts both while
  still requiring the ref-lock error, rejection status and bead's rejection classification.
- A command receipt survives a *failed* later command by the same actor and is dropped only when
  a later command by that actor goes through: the remote accepted it, or it was a local write that
  was appended. A recognised retry still reports the claim as it is now.
- A push's outcome is git's own status line (`--porcelain`); a `[remote rejected]` that is the
  remote's verdict (a hook, a rule) is `REMOTE_REFUSED` and is not retried; one the remote could not
  take (`unpacker error`, `missing necessary objects`) is retried after a fresh fetch; a
  remote that has lost objects is `REMOTE_DAMAGED`.
- `sync.git` is a cache: when git reports a local read error in it (an empty or corrupt object), bead
  sets it aside as `sync.git.damaged-<time>` and makes it again, once per command, under the store
  lock; the second time in one command is `SYNC_REPO_DAMAGED`. A line the remote relayed is never
  taken as local damage.
- `sync.git` hashes like the checkout bead runs in (G26), not like the remote: bead does not ask
  the remote for its format. git before 2.41 clones an *empty* SHA-256 repository in SHA-1; in
  such a checkout the user's own push and bead's are both refused (`REMOTE_UNREACHABLE` carrying
  git's "does not support this repository's hash algorithm"), events stay in the store, and once
  the checkout is SHA-256 bead makes `sync.git` again and publishes them.
- git's automatic maintenance is off in `sync.git` (`gc.auto=0`, no detached gc, `fetch.unpackLimit=1`
  so fetched objects stay packed); bead repacks it under the store lock when loose objects or packs
  pile up, and prunes only unreachable loose objects older than an hour (an in-flight fetch's are
  younger). The remote's own maintenance is the remote's business.
- Format 1 is frozen; changes are additive. An event an older bead cannot apply (a new operation,
  a new field, a newer `v`) is set aside, reported under `integrity`, carried through sync
  untouched and applied after an upgrade; a mixed fleet is safe. A breaking change, should one
  ever be needed, follows writers-lag-readers through the manifest; none is planned.
- Events nest at most 100 levels (`MAX_NESTING`); replay keeps looser type rules than writes
  (`write_problem`) so histories written before a rule mean the same on every machine.
- Integers have any size up to Python 3.11+'s default 4,300-digit decimal conversion limit on JSON
  input and output, not unlimited integer tokens. The oracle pinned that default until
  the cutover; a machine whose Python ran with another limit had to resolve its store's
  compatibility before it upgraded.
- What a journal line, attempt file or manifest means does not depend on the Python reading it
  (U01–U05 in `FINDINGS.md`): JSON is read from bytes as UTF-8 with surrogates
  passed (no byte-order mark, UTF-16 or UTF-32 is guessed), at most 512 levels deep (counted before
  `json.loads`, below every interpreter's own limit), with NaN, Infinity and numbers beyond a float
  refused; ids and dependencies match the UUID shape whole; `complete_prefix` and the manifest read
  as strictly as everything else, and the manifest's format must be the integer 1.
- Key numbers, segment names and URL schemes are ASCII (U06): `\d` and `(?i)[a-z]` are never
  Unicode-aware there, so a renamed copy of `x.٣` keeps its own name and `httpſ://` is not
  `https://`. This departs from the port plan's recommendation (keep), on the maintainer's decision.
- Shapes bead never writes, which made Python's replay or a query fail, or answer by interpreter
  version, are refused by validation (U07–U11): a `dep_add` meta that is not an
  object, a `dep_remove` key that is not a string, a create key whose number has 4,300 digits or
  more, an imported comment with a `uid` or a `where`, a create clearing `created_at`; a
  quarantined id's stand-in names its issue only by a string; an import refuses an escaped lone
  surrogate anywhere in a record. Replay has no last resort to reach.
- A renamed copy that finds every free key taken fails replay with `KEY_SPACE`, as in Python,
  whose replay raised it: beadroll-core's `replay` returns it.
- Where Python failed with a traceback, Rust answers with a structured error instead, each recorded
  as it lands. In beadroll-core: `mint_key` under a parent whose children's numbers have
  more than 4,300 digits, or would count on to 4,301, is `VALIDATION` (`MintError::NumberTooLong`).
  In the store, sync and command line (Phase 2a): a `BEAD_NET_TIMEOUT`, `BEAD_ROTATE_BYTES`,
  `BEAD_REPACK_LOOSE` or `BEAD_REPACK_PACKS` that Python's `float()` or `int()` cannot read is
  `CONFIG` when bead starts, `--version` included; the last segment of this machine's journal, when
  its name is not a number (or the next one's would have more than 4,300 digits) and bead appends
  after it, is `IO`; a `published` count in `meta.json` that is not a number, or counts that make
  the unpublished count longer than 4,300 digits, is `IO`; a `GIT_CONFIG_COUNT` that `int()` cannot read where Python reads it
  (it is not digits and nothing was carried over, or its digits are superscripts) is `CONFIG`; an
  answer standard output refuses (other than a closed pipe or a hung-up terminal) is an `IO` error
  on standard error, exit 1; a `ps` line that names no command ends the nearest-harness walk.
- The Rust bead keeps CPython's semantics wherever Python's bead had them, as the port plan's
  ledger of them decided (the rows not stated above): replay takes a boolean where an integer is due
  (`"v": true` is format 1; a `true` priority sorts as 1 and prints as `True`), as histories
  written before `write_problem` had it, while writes refuse one; a lone surrogate in a line,
  escaped or raw, reads as one U+FFFD
  and is reported; a duplicate key keeps its last value at its first key's place; whitespace,
  `strip()` and lower case are Python's Unicode ones where Python used them (`split_list`, comment
  emptiness, `search`, `normalize_remote`); a path is resolved as `os.path.realpath` resolves it,
  never by libc; `--estimate` and `--limit` read numbers as `int()` does (`" +5 "`, `5_000`);
  conflict values print as `repr()` prints them, and `True` as `True`; an OS error reads
  `"{strerror}: {path}"`, a rename's naming its target; the JSON of each output and file is the
  `json.dumps` dialect Python wrote it in; the command line was argparse's, its errors and help
  pages included, until clap replaced it (roll-dih8.23, above); SIGXFSZ is ignored, so a file-size limit is an `IO` error, and standard
  input closed at exec is no stream, `-` then an `IO` error; a sync or ownership command makes 12
  rounds with `U(0.1, 0.4) × attempt` seconds between them; the names of fetch refs, temporary
  files and set-aside copies are Python's.
- beadroll-core was proven equal to Python by `cargo xtask diff` until the cutover: replay reports
  and JSON, over the journals the Python tests built and generated histories, valid and hostile.
  A difference was fixed in Rust or decided in that ledger; the harness kept no list of expected
  differences. Since the cutover the goldens (`crates/beadroll/tests/golden/`) and the answers
  `parity` pins hold what Python's bead printed. Where the fixed fixtures came from: the golden
  outputs and receipt signatures were captured from `python-0.1.0` under CPython 3.14, and so
  were the actor and input fixtures (`parity/actors.rs`, `parity/inputs.rs`) and the unpublished
  counts (`COUNTS` in `parity/pending.rs`); the JSON parser's diagnostic fixtures take CPython
  3.14 as their reference; the non-printable table (`text/printable.rs`) was generated by
  `xtask/python/printable_table.py` with CPython 3.14.7's `str.isprintable()` under Unicode
  16.0.0; and the stress models' generator (`stress/seeded_random.rs`) follows CPython
  3.11–3.14's `Modules/_randommodule.c` and `Lib/random.py` (MT19937, integer seeding,
  `getrandbits`, `random`, `randrange`, `choice` and weighted `choices`), its recorded stream and
  draws taken from that implementation, so old stress seeds reproduce the same scenarios.
- The binary has failpoints in every build, activated only by `BEAD_FAILPOINTS` and the two
  crash hooks Python had (`BEAD_TEST_CRASH_BEFORE_PUSH`/`AFTER_PUSH`, which are aliases); a spec
  that cannot fire as written is `CONFIG` (the points and their actions are listed in
  `crates/beadroll-store/src/failpoint.rs`). The maintainer's decision of 2026-10-09: the binary
  shipped is the binary tested, as Python's hooks lived in the product, for the cost of one
  environment lookup; a cargo feature would have kept them out of release builds at the cost of a
  second build for every test command. In-crate tests pass the registry in `Settings`;
  `error(CODE)` at `ownership.after-confirm` is their in-process form of Q03's interruption.
- The Rust binary starts fewer gits than Python (Phase 4; `crates/beadroll-git/AGENTS.md` lists
  them): one combined `rev-parse` for the repository's top, common directory and format, and in
  `sync.git` a long-lived `cat-file --batch-command`, `hash-object --stdin-paths` and `mktree
  --batch`, and an `update-ref --stdin` for refs after fetches and pushes, with the listing and
  `TIP^{tree}` worked out in-process as git works them out. Whatever is not certain goes to the
  git Python runs for it, and its answer or failure is the one given; `sync.git` gets the same objects and refs. What can
  change while bead runs is asked by Python's own call where Python asks it: origin's URLs each
  time the workspace is found, the transport settings each time a link is made, HEAD and its
  branch (one `rev-parse` where its answer is certain) each time a transaction starts, under the
  store lock and so not with discovery's gits: a branch switched while a write waits for the lock
  is the one its events record. After a fetch, one transaction keeps the fetched head in
  `refs/beads/remote` and deletes the fetch's own ref (roll-dih8.14). It reuses the same writer
  after a push and falls back to the two standalone calls if it cannot answer; post-fetch
  bookkeeping stays best effort. The earlier transaction was reverted because deleting fetch refs
  sooner made concurrent fetches fail (G16); fetches now retry that race (roll-dih8.12). What
  still differs: which gits run (one more `rev-parse` where a combined one cannot answer: a bare
  repository or none at discovery, no commit yet or a branch name git finds ambiguous when a
  transaction starts); origin's `config` runs, read-only, beside the first `rev-parse`, so also in
  discovery's early-error paths where Python stops before asking for origin, and so does the
  `cat-file` of the committed `.beadroll.json` wherever the directory bead starts in does not show
  one in the tree (roll-lqjj.33: no `.git` above it holds a `.beadroll.json` beside it, it is
  inside a repository, no repository is found, or `GIT_DIR` or `GIT_WORK_TREE` are set): its
  answer is used only where git's own top has no `.beadroll.json`, a wrong guess costing a git
  whose answer is unused, or one started after the others as before, never another answer, warning
  or error, and a tree with a dangling link or a file that is not JSON, which discovery refuses
  first, starts none; and a SIGKILL while a blob is hashed can leave a file in the temporary
  directory (the attempt's or the manifest's content, never in the store).
- A publish hashes only the journal files it cannot vouch for (roll-lqjj.1, step 2 of epic
  roll-lqjj): the store remembers, in `sync.git/journal-memo`, each segment's and attempt file's
  identity on disk when it was hashed (length, modification and change times, inode, device: what
  git's index compares), its lines and its blob. A file that keeps that identity is not hashed: a
  merge compares the remembered blob with the published one as it is, and a publish names it in a
  tree only when it is sure to be in `sync.git`, because this command's link made it or the tree of
  the tip it merged holds it at that path; otherwise the file is hashed. The identity must also
  have been settled when the memo was written (Y01 below), or nothing it says of the file counts. A gc drops a blob no ref
  reaches (`prune_now_on_the_private_sync_repo_during_syncs_is_transient`), and one named in a
  tree would fail every `mktree` after. Keyed by more than the path and length the issue named,
  since a fork the published copy replaces, or a file rewritten in place, can keep its length. The
  memo is a cache, never a source: missing, unreadable or stale, it counts for nothing; one that
  cannot be written is left as it was; it goes with `sync.git`; and a publish whose tree cannot be
  made from remembered blobs, short of damage (an object lost while the published tree names it),
  hashes every file once more, as every publish did before (L02). The merge still asks git what
  Python asks, in the same order: fewer files to `hash-object`, then the published tree.
- A cache vouches for a file only once the file was settled when the cache itself was written
  (`FileKey::settled_before`, Y01): both the file's modification and change times are older than
  the cache file's own modification time. A kernel stamps an inode's times from a clock of its own
  granularity, so a rewrite in place that keeps a file's length keeps every part of its identity
  when it falls within one of that clock's ticks, and a command writes its memo and its projection
  in the tick of the files it has just written. What a cache says of such a file is therefore not
  trusted (git calls one racily clean): the memo forgets it and hashes or counts it again, and the
  projection reads it against the sum kept of its bytes, which attempt files carry as segments
  did, and replays the journal when nothing summed can answer. Each cache takes the bytes it
  holds and the identity it compares them against from one open file, since a writer under the
  lock can rename a newer cache over it between two looks at the path (M01). The cost is the files one command
  wrote, read once more by the next, and nothing else: `cargo xtask timing --read 20000` moved
  within its own noise (a `list` 0.030 s and a local write 0.069 s, against 0.028-0.044 s and
  0.055-0.074 s before, on 2026-10-13, Rust 1.99.0, 16 cores). What a publish hashes stays
  exactly measurable, with the memo's own time moved past its files first (`settled_round`). The
  shared attempt listing answers the same hazard the way an in-process cache can: every attempt
  bead writes forgets the names (`Workspace::forget_claims`), so what this process wrote is always
  listed, while a file another process adds within one tick of our listing is simply not there
  yet, which a listing taken a moment earlier would also have said. What remains is git's own
  residue: a rewrite in place, at the same length, between bead reading a file and writing the
  cache, inside one tick. Every later rewrite is seen.
- Accepted attempt names are shared within a command's locked phase (roll-lqjj.12). Each
  claims directory is enumerated once while its length, device, inode, modification and change times,
  and mode stay the same. A changed directory is read again, including after a merge or an
  attempt's confirmation, which forgets the names outright rather than trusting the directory's
  times (Y01); taking or releasing the store lock clears the listing, so a fetch
  outside the lock cannot carry old names into the next phase. Unlocked readers also validate
  each directory before reusing its names. Missing or unreadable directory metadata never
  authorizes reuse, and enumeration errors remain errors. This is an in-memory cache only;
  every file is still stat-ed for projection and blob checks, so an in-place rewrite with an
  unchanged directory is seen under the same rules as before. Neither the projection's layout
  nor the published membership map in `meta.json` changes.
- A publish reuses an unchanged claims subtree only when the last merged tip holds it
  (roll-lqjj.13). The desired entries' canonical tree object name, with mode 100644 and every
  filename and blob, must equal the subtree name at that path. SHA-1 and SHA-256 are checked
  against `mktree`; unusual names go through `mktree` as before. Every merge and repository
  rebuild forgets the old candidates, and a failed tree build's rehash fallback forgets them
  too. The published tree bytes stay identical. This removes unchanged claims from `mktree`'s
  input, not from the file identity checks, blob map or comparison against the merged tree;
  changed claims directories are rebuilt in full. The same reachability rule as blob reuse
  keeps a subtree a gc could have dropped out of the new tree.
- A fetch that loses to another process's fetch of the store is tried again (roll-dih8.12): git's
  checks of a fetch read every ref, and every process fetches into a ref of its own under
  `refs/beads/fetch/` that it deletes after, so git can find such a ref gone between listing it
  and reading it (`bad object refs/beads/fetch/…`), or the directory of its own removed while it
  makes it (`cannot lock ref 'refs/beads/fetch/…'`), and it fails the fetch though the remote
  answered. Only git's own lines count, never one the remote relayed. Five attempts in all, a
  wait of `U(10, 50) ms × attempt` before each new one, short because a claim fetches under the
  store lock; after the fifth the fetch fails as it did before (`REMOTE_UNREACHABLE`, git's
  words). Chosen over `transfer.hideRefs=refs/beads/fetch`, which keeps those refs out of the
  check only where git's fetch honours hidden refs (2.55 does, 2.40, the oldest supported, does
  not) and leaves the lock form as it is.
- A merge extends another machine's segment by appending the lines ours lacks, fsynced, as bead
  appends its own records (roll-lqjj.2), not by writing the published copy whole under a new name
  as Python did: the segment stays the file the projection was made from, which then takes in
  those lines alone, where a rewrite had the journal replayed after every such merge. The bytes are
  the same either way. A reader that comes upon the append skips a last line not yet whole, as it
  does for bead's own segments, and a merge cuts one an interrupted append left before it reads
  (`repair_tail`). A new segment, or one replaced because neither copy is a prefix of the other, is
  still written whole.
- A journal keeps what replay reads of each event, not its parsed JSON (roll-dih8.11): its id,
  causes, writer, actor, place and typed operation, each text and value events repeat held once,
  and a digest of its content, by which it tells two records of one id apart. The digest is two
  SipHash-1-3 hashes under keys drawn at random for each journal, over an encoding in which no two
  contents are alike (members in key order, lengths before texts and lists, integers by their
  digits, floats by their bits, as Python's `canonical` compares them). It is not a cryptographic
  hash and need not be: the keys never leave the process, so nobody can aim at a collision, and
  two contents share 128 bits by chance once in 2¹²⁸; a collision would apply the first of two
  contents of one id where both should be quarantined. Replay takes each event's operation from
  the journal as it applies it, and the state it builds keeps the rest (who wrote each event, what
  it had seen), not the journal: `State` has no `journal()`.
- The 150 MB budget for replaying 200,000 events is a heap budget
  (the maintainer's decision of 2026-10-12, roll-dih8.11): `cargo xtask timing --replay N` counts
  the most a fresh process that loads the journal and replays it holds on its heap at once, and
  records the resident peak beside it. The resident peak is higher, about the journal at load plus
  everything replay allocates, because the system allocator keeps most of what replay frees for
  sizes the state never asks for again; closing that gap is roll-dih8.15. Events the size of this
  repository's own take more than xtask's: memory grows with the bytes of the events, which the
  journal and the state both hold.
- xtask's global allocator is the system's with a count of what it hands out
  (`xtask/src/timing/heap.rs`), for the heap peak `timing --load` reports: the one unsafe code
  outside bead's two audited spots (`CONTRIBUTING.md`), since `GlobalAlloc` is an unsafe trait.
  Every call goes to the system allocator unchanged. Its counts are atomic, so the peak is exact
  in any process; counting makes a replay timed in xtask about 5% slower.
- `BEAD_HOME` must be absolute; the store's modes are 0700/0600 whatever the umask.
- Only directories bead creates get private modes; a user-supplied `BEAD_HOME` is chmodded 0700
  on first write (reviewer observation, kept).
- origin is never remembered as the coordination remote; only a remote chosen otherwise is: by
  its name, also one that shares origin's URL (its own push URL, or a `pushInsteadOf` rule, can
  send the two apart), or by a URL other than origin's (G25: in any spelling of its repository on
  a forge). A tracked config seeds a new store's remote but never silently moves an existing
  store's.
- bead's pushes skip the user's client hooks (`--no-verify`); the remote's hooks still apply.
- The remote string reaches git after `--`; a tracked workspace id shaped like a URL-derived id
  that is not this origin's is refused.
- beadroll-core reads and writes JSON with a small parser and writer of its own, not serde_json:
  journals need integers of any size within the 4,300-digit limit, objects in the order written,
  a lone surrogate read as one U+FFFD (serde_json refuses the escape, and a lossy decode makes
  three of the raw bytes), floats as Python's `repr()` printed them (`1e-05`, `1e+16`), and five
  dialects: compact for journals, attempts and export; spaced for results and, sorted, the
  manifest; spaced and ASCII-escaped for errors, `--version` and `replica.json`; `indent=1` and
  ASCII-escaped for `meta.json`, `aliases.json`, `moved.json` and `.beadroll.json`; sorted and
  compact to compare quarantined copies. serde_json's `arbitrary_precision` and `preserve_order`
  are features every crate in a build shares, so turning them on would change studio's own
  serde_json. The parser is iterative, so no depth overflows the stack.
- git is the git binary, never gix or libgit2: bead's contract is "as git reads it" (`insteadOf`,
  `includeIf`, credential helpers, `core.sshCommand`, translated messages, server hooks, SHA-256
  repositories, reftable), which a reimplementation would have to match, and the hardening tests
  would be the first to find where it did not; gix has no production push, and libgit2 brings a C
  build and only experimental SHA-256. Writing blobs, trees and commits in-process would save
  about three processes a publish and is not done: it needs zlib for git's loose objects, git's
  date and signing rules, and git's existence checks against packs.
- Durability is `fsync(2)`, never `F_FULLFSYNC`, which Rust's `File::sync_all` and `sync_data` are
  on Apple platforms: 5.3 ms a call against 0.03 ms on APFS, about 40 ms a write. The remote is
  the durable copy. Atomic writes are a temporary file, fsync and rename, cleaned up on failure;
  appends are `O_APPEND` and fsync; the store lock is `File::lock`.
  The two checksummed caches are the exception (roll-lqjj.27, X05): the projection and its text file
  are written whole and atomically but not fsynced (`fs::write_cache_parts`; the text file's appends
  too), because flushing them made the command after every write cost as much again as the write:
  its read 39 ms against 9.5 ms on 300 issues, 72 ms against 46 ms on 5,000 (medians of 30
  interleaved runs, a write followed by a `show` of another issue), and `timing --read 1000` 29 ms a
  write against 20 ms, a cold read 35 ms against 17 ms. Every other store file stays flushed:
  journal segments, attempt and pending files, receipts, `meta.json`, `replica.json`, and the
  journal memo, which has no sum to read it against. Why a crash can only cost a replay: nothing in
  either file is trusted by being there, only by what a reader checks of it, and a crash can only
  produce files that fail a check or are a whole file bead wrote at an earlier moment.
  1. The projection is a new file renamed over the old (the failpoints `atomic-write` and
     `atomic-written` fire as for every atomic write), so a reader sees the old file or the new,
     whole. After a crash the name holds the old file, the new one, or a new one whose blocks did
     not all reach the disk (cut, zeroed, a hole, a mix of old and zero): the header's lengths
     must add up to the file's size and each of its three parts must match its sum, and anything
     else is no projection (`projection::tests::a_damaged_projection_is_never_trusted`, every
     cut and a flipped byte every seventh). A whole file is one bead wrote, true of the journal
     as it was then, and is judged as a projection killed before its write is: its journal files'
     identities, settled by its own write time, against the files now, a segment grown read
     against the sum kept of it, anything else replayed. Which of the two files a crash kept
     changes nothing about that.
  2. A text is never read as a file's say-so. A projection places each issue's texts (offset,
     length, sum) in the text file of one generation (the sum of the texts it was started with,
     in the file's header and in the projection's `made`); a reader opens the file only if its
     header is the magic and that generation, and takes a text only if the bytes read at its
     place, to the full length, sum to what the projection kept (`State::read_texts`). A file
     lost or zeroed fails the header; one cut short fails the read; one whose blocks are zeros,
     old bytes, or other texts appended where lost ones were fails the sum; a file started again
     that the projection does not name is another generation. There is no state in which texts
     are read that are not the ones placed, with a 128-bit sum's confidence, and the texts that
     do match are the right ones whatever wrote them.
  3. The two files out of step are every pair of the above: a projection newer than its text
     file (the appended tail lost: the places are past its end, or overlapped by texts appended
     since), older (extra texts nothing places, which the `SLACK` rule starts the file again
     for), of another generation, or one without the other. Each is a failed header, read or
     sum for the texts that are asked for, and a command asking for none (`list`, `ready`,
     `blocked`, `claims`, `dep`, `info`) reads the projection's state alone, which stands on its
     own sums. A command that asks for a text it cannot read replays the journal, and one that
     reads every text writes both files anew; a write whose events are about issues whose texts
     cannot be read in replays for its state (`Tx::state`), and a projection to extend with
     records about such issues declines and has the journal replayed. A projection left placing
     texts past a text file's end for issues no command has asked about is not repaired on the spot,
     and is not wrong: the first command to ask about one of them finds it unreadable and replays.
  4. The cost is a replay, once, as for any projection missing: the answer of every command is
     the same as before, and so is the file written then. Neither file is a source of anything:
     the journal is, and each state read from them is checked against its files and the sums
     above. A write error that only appears when the kernel writes a file back (a full volume
     after the rename) leaves a file the checks above refuse, as one that failed at once leaves
     the last.
  Tests: `projection::tests` (a damaged or cut projection; a stale one; one the volume was full
  for; the text file damaged, cut, gone or another's), `projection::tests::texts` (the text file
  shorter than the projection places, with other texts appended where the lost ones were; every
  pairing of a projection and a text file as a crash can leave them, read by a listing, a show, an
  export and a write; both files past the failpoints), and the black-box
  `storage::projection` (a bead killed as it writes either file).
- The remote seam waits for its second implementation (roll-k2wy.14.1): the protocol
  (`synchronise`, `coordinated`, `settle_*`, `merge`) uses a link's fetch, tree reading and push,
  never `sync.git`'s internals, and merge works on (path → blob) maps; a `Remote` trait comes with
  the in-memory remote that will stand beside `Link`, never as a trait with one implementation.
- Nothing is published: every crate is `publish = false`, there are no prebuilt binaries or
  packages, and machines build from source (`README.md`, *Install*); public channels wait for the
  open-source milestone (roll-ydfw). pancake-studio takes `beadroll-core` as a git dependency on
  this public repository, pinned to a commit (the maintainer's decision of 2026-10-10,
  roll-k2wy.15), so none of its builds needs credentials. `infra/` stays stdlib Python driving any
  `bead`, until there is a reason to move it into xtask.

Decisions that changed while building, each with its reason:

| First design | Now | Why |
|---|---|---|
| Concurrent values need an explicit resolution operation | Flagged with every alternative; any later write that saw them settles it | Same information, no extra protocol for agents to learn |
| Fallback display is the smallest event id | The newest event id | Matches what people expect to see |
| Conditional push with `--force-with-lease` | Plain fast-forward push | Already conditional, and cannot truncate history |
| A corrupt duplicate holds back its dependents | Only the event itself is set aside | With a project-wide frontier, everything later depends on it |
| Heartbeat events | Last journal activity per owner | No extra traffic or permanent noise |
| Structured acceptance items | One text field | Round-trips through Beads JSONL |
| Checkpoints and pruning | Segment rotation only | The rest is not needed at this size |
| Owner's close is a conditional transition | Local write, published at once | The stale-claim rule makes a late one harmless |
| Ownership in the journal | Ownership files beside the journal, pending until the remote holds them | A refused or unanswered claim then needs no compensating record, and the journal stays append-only |

## 3. Baseline

Do not take the baseline from a document; run it:

```sh
./check.sh            # every test but the slow models, the doctests and the infra/ drivers' tests
./check.sh --stress   # also the randomised multi-machine models (several minutes)
./check.sh --real     # also import/export of the real tracker exports on this Mac
```

`check.sh` runs `cargo xtask check` (it needs rustup and cargo-nextest): it prints one line per
step, gives each a scratch `BEAD_HOME`, and exits non-zero on any failure, including a failed
round-trip command or differing exported records with `--real`. A failed step keeps its full
output in `target/check/`, and a failed round trip shows its whole report; its imports run
offline in the scratch store. The default sweep includes owner close/delete retries after an
interrupted publish; the full sweep (`BEAD_SWEEP=full`) has no expected-failure exemptions. Every
schedule must show that its fault fired (the operation's exit 98 or 99, its SIGKILL, or the
remote's stall hook started by its push), so none passes without reaching its boundary.
xtask's `check::check_script`, the sweep's `sweep_failure_reporting` and
`infra/test_roundtrip.py` exercise these failure paths with synthetic results in the fast
baseline. Record the baseline output in your report.
The real-infrastructure scripts (four machines, a throwaway GitHub repository) are
under `infra/`; their last results are summarised in `README.md`, *Status*.

## 4. Rules

- Work in a scratch directory. **Always set `BEAD_HOME`** to a scratch path before running `bead`;
  the default (`~/.local/state/beadroll`) holds the project's own real tracker (this repository
  dogfoods `bead`; its `.beadroll.json` names that workspace) and must not be touched. The tests
  give every machine a scratch `BEAD_HOME`; do it yourself for anything else.
- Run `bead` only inside repositories you created for the test, or with `BEAD_WORKSPACE` set.
  Never run it inside the real repositories under `~/Documents` (the origin URL would route you
  to a real workspace id). Reading their `.beads/issues.jsonl` files is fine; do not copy their
  contents into any remote.
- Local bare repositories are the default remote. The only real remote you may push to is the
  throwaway repository the maintainer provides in `$BEAD_TEST_REMOTE` (private, synthetic data).
  Do not create other repositories or refs anywhere, and do not push to `main` there.
- Do not modify bead's code (`crates/`), `README.md`, `docs/GUARANTEES.md`, `FINDINGS.md`,
  existing tests or earlier
  reports. Proposed fixes go in your report as unified diffs.
- Do not change SSH, firewall, git or shell configuration on any machine. Do not run `br` or
  `bv` inside real repositories (`br where` writes lock files).
- Starting Codex or Claude Code sessions from a test is for identity detection only: at most a
  couple, and say so in the report.
- Inject faults with failpoints (`BEAD_FAILPOINTS`, listed in
  `crates/beadroll-store/src/failpoint.rs`), never by patching bead or the clock:
  `backoff@always=skip` skips bead's wait between attempts, never a shortened sleep;
  `pause-until(PATH)` holds bead at a point until the test touches `PATH.go`; `exit(N)`,
  `sigkill`, `errno(...)` and `fail-reply` stop or fail it there; `clock`'s `offset` and `at` move
  its clock. A spec that cannot fire as written is `CONFIG`, so a fault never silently misses.
  Tests that call the crates in-process pass the registry in `Settings`.
- Report outcomes faithfully: a test you could not run is "not run", not "passed".

## 5. Checklist

Work through these; add your own. For each, say what you did, what you expected, what
happened, and attach the reproduction (command or test).

**Replay and causality**
- Same event set → same projection, conflicts, claims and readiness under every delivery
  order, duplicate delivery, and journal enumeration order (beadroll-core's `replay` tests permute
  three to six machines; try more, and thousands of events).
- Ancestry (`State::saw`) depends on each event depending, directly or not, on its predecessor
  in the same journal, and on the chains the events of attempt files join (`state/seen.rs`): an
  event joins a chain only when the chain's last event is among its ancestors, and its clock
  holds the furthest position it saw on every chain. Construct journals where that fails
  (hand-written events, ownership events whose ancestors cross chains) and see what replay does.
- Events whose causes are missing must wait (`pending`), never be treated as concurrent.
- Unknown `op`, unknown `v`, missing fields, non-UUID ids, deps that are not lists: is the
  damage scoped to the event?

**Journal integrity**
- Concurrent writers on one machine (`writing` uses `flock`); readers take no lock and must
  tolerate a torn tail and a half-written attempt file. Kill `-9` a writer at every boundary.
- Segment rotation (`active_segment`, `BEAD_ROTATE_BYTES`) under two local writers.
- Merge rule (`Link::merge`): one side must be a prefix of the other. Own-journal fork error,
  foreign-fork report, a remote truncated or rewritten (`git push --force` to the ref), an
  extra segment, a segment deleted locally.
- Duplicates: the same id in two journals with identical payloads; reused ids across
  workspaces.

**Claims**
- Try to produce two confirmed owners of one issue on any pair of machines, by any
  interleaving: simultaneous claims, claim during another's release, takeover during release,
  two takeovers at once, claim after a forced close, claim of a tombstoned issue, claim while
  the status is disputed, offline claim then online claim by the same thread, release of a
  provisional claim.
- `generation` must be monotonic on every machine after any interleaving, including a claimant
  whose fetch was stale.
- Lost acknowledgement at every point: `BEAD_TEST_CRASH_BEFORE_PUSH`, `BEAD_TEST_CRASH_AFTER_PUSH`,
  a push killed mid-flight, a push that times out (`BEAD_NET_TIMEOUT`) but was applied, a
  receive hook that stalls (see the `protocol` tests). Then: a retry of the same command, a
  different command, a `sync`, from the same and from a new process.
- The `superseded` pre-pass in `replay`: find a case where a takeover rejects a legitimate
  write, or where a hand-written takeover corrupts ownership.

**Fields, labels, dependencies**
- Concurrent writes to every register, including `None` (a clear) against a value.
- Label add/remove and dependency add/remove interleavings across three machines; a cycle
  formed by merge and then partially removed.
- Readiness against `br` on the real trackers (`infra/roundtrip.py` imports them; compare
  `ready`/`blocked`), especially disputed statuses, tombstoned blockers, multi-parent issues.

**Sync protocol**
- Interleave local writes with a slow push. Manifest and workspace checks. Ref validation.
  Retry exhaustion against a remote that always moves. Two machines sharing a machine id.

**Beads compatibility**
- Round-trip every real tracker export, including those `br` rejects. Export after mutations:
  comment ids, dependency metadata, tombstones, unknown fields in `extra`. Feed exports to
  `bv --robot-triage`, `--robot-plan`, `--robot-insights`.

**Identity and privacy**
- Grep every event ever written for absolute paths, hostnames, usernames, or profile
  directory names. Only opaque machine ids, thread ids, branch names and commit hashes should
  appear.
- `detect_actor`: `CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`/`CODEX_SESSION_ID`, nested
  harnesses (process tree), a human shell, `--actor`, `BEAD_AGENT`, a sandbox without `ps`.

**Performance and scale**
- Synthetic journals of 10k, 50k and 200k events, and ownership-heavy histories: replay time,
  memory, `show` latency. Growth of the remote over 1,000 syncs with and without `git gc`.

**CLI**
- `--json` and `--actor` anywhere in argv; multi-id commands; `--description-file -`; exit
  codes and the JSON error envelope on stderr; `list` filters and `--limit 0`.

## 6. Stress-test plan

Simulate machines locally: each is its own `BEAD_HOME` and its own clone of one local bare
repository. `beadroll-test-support` (`Sandbox`, `Machine`) shows the pattern;
`crates/beadroll/tests/cli/stress/` has two randomised models to extend.

1. **Randomised model check.** 6 machines, 2,000+ operations drawn from the real distribution
   (show 32%, update 12%, list 9%, ready 6%, close 5%, create 5%, dep 5%, comments 4%,
   claim/release/takeover, sync every few ops), with random partitions. Keep a reference model
   of ownership from the commands that were *accepted online*, and assert after every full sync
   round: identical exports and per-issue `show` everywhere; at most one confirmed owner per
   issue, equal to the model; every accepted claim visible everywhere; no `waiting_events`,
   `integrity` entries or `forked_journals`; readiness consistent with blockers.
2. **Fault injection.** `kill -9` during append, fetch, push and confirmation (the failpoints'
   `sigkill`, `torn-sigkill`, `exit(N)`); the crash hooks; disk full (`errno(ENOSPC)`); read-only
   store mid-run; clock skew (the `clock` failpoint's `offset`).
3. **Hostile remote.** A clone that force-pushes old or foreign commits to `refs/beads/journal`,
   deletes the ref, or replaces the manifest.
4. **Optional, with the real remote.** `infra/drive.py` and `infra/soak.py`; see their
   docstrings for the setup on the four machines and the reset procedure. Bursts of SSH
   connections to `github.com:22` from the LAN machines have been throttled before; if you see
   `REMOTE_UNREACHABLE` with connect timeouts, stop and report rather than switching endpoints.

## 7. Deliverables

1. **A report** at `reviews/<date>-<n>-<name>.md` (n = the next review number). Findings ranked
   by severity (data loss or divergence > wrong ownership > wrong readiness or conflicts >
   robustness > usability), each with an id of the form `<letter><nn>` using a letter not yet
   used in `FINDINGS.md`, a one-line claim, a reproduction and, where you can, a proposed diff.
   Design observations clearly separated from bugs, and from the accepted decisions in §2.
   Exactly which checks in §5 and §6 you ran, with numbers, and which you did not. Any side
   effects you caused anywhere, even if cleaned up. The `check.sh` output you started from.
2. **Tests**, in a new module `review<n>` of the `beadroll` crate's test binary
   (`crates/beadroll/tests/cli/review<n>.rs`, declared in its `main.rs`), built on
   `beadroll-test-support`: every bug as a failing test, every property you verified as a passing
   one, each test's doc comment naming the finding id. The maintainer folds them into the subject
   modules when the findings are closed; do not edit the existing modules.
3. **Proposed ledger rows** for `FINDINGS.md`, one per finding, at the end of the report.
4. Your scripts, kept runnable, in your scratch directory.
