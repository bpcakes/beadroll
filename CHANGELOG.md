# Changelog

All notable changes to beadroll. The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/). The on-disk and published format is
versioned separately (format 1, frozen; see *Format and upgrades* in `docs/GUARANTEES.md`).

## [Unreleased]

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
