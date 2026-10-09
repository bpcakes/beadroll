# Porting beadroll to Rust

*Plan, 2026-10-09. Baseline: `main` at 9867715 — beadroll 0.1.0, format 1, Python 3.11+. The
maintainer's decisions of 2026-10-09 are recorded in §10.*

This is the plan for rewriting `bead` in Rust as a Cargo workspace, porting every test, and moving
pancake-studio onto the new crates. It says what must not change, how the code is split and why,
how we prove that nothing regressed, and in what order the work lands. The per-test port catalogues
are in `docs/rust-port/`; the measurements behind the numbers are in Appendix A.

## Summary

- **The same product, byte for byte.** Format 1, the store layout, the published tree, the CLI
  contract, the error codes and every decision in `REVIEWING.md` §2 carry over unchanged. Python
  and Rust machines share a tracker during the rollout, so neither may write anything the other
  would read differently. The port adds nothing a user can see except speed. Behaviour changes
  wait until after the cutover.
- **Six crates and an xtask.** The split follows ownership, not layers. `beadroll-core` is pure:
  no filesystem, processes, clock or environment, and studio links only this crate.
  `beadroll-git` is the only code that starts git. `beadroll-store` holds this machine's files and
  identity. `beadroll-sync` holds the protocol. `beadroll` is the `bead` binary.
  `beadroll-test-support` is the Rust `tests/support.py`. Crate boundaries make the compiler
  enforce the rule that modules import only downwards (`AGENTS.md`).
- **Proof of no regressions comes from four directions:**
  1. The existing Python suite drives the Rust binary through `BEAD_UNDER_TEST`. About 330 of the
     458 tests run unchanged.
  2. Every test is ported to Rust. A ported black-box test counts only once it also passes against
     the Python binary.
  3. Replay and JSON are tested differentially against CPython.
  4. Mixed Python/Rust fleets must converge.
- **Speed.**
  - Read commands: 120–200 ms → about 15 ms.
  - Publishing writes against a local remote: 630–730 ms → about 180 ms with the same git calls,
    about 90 ms once calls are consolidated. Against GitHub, both are bound by the network round
    trip.
  - Fast test suite: about 20 minutes → target under 2 minutes on this Mac (16 cores).
  - Full ownership sweep: about 1.7 hours → target under 20 minutes.
- **Order of work:**
  1. Foundations.
  2. The pure core, proven differentially.
  3. Engine and CLI in vertical slices until the Python suite is green against Rust.
  4. Full test port, which runs in parallel from the start.
  5. Performance.
  6. Studio.
  7. Cutover.

## 1. What does not change

The port is a reimplementation of a frozen contract, not a redesign. Everything in this section is
asserted by tests today or is read by another machine, `br`/`bv`, or studio.

### 1.1 Bytes on disk and on the remote

| What | Written as | Notes |
|---|---|---|
| `$BEAD_HOME/replica.json` | `{"id": "<8 hex>", "alias": "<host>"}`, `", "`/`": "` separators, ASCII-escaped. Created whole as `.replica.<pid>.tmp`, then `link(2)` | The first writer wins; the hostname lives only here |
| `aliases.json`, `moved.json` | `indent=1`, ASCII-escaped, no trailing newline | Unknown or invalid entries are ignored on read |
| `<ws>/lock` | Empty, 0600, `flock(2)` exclusive | Rust's `File::lock` is `flock(LOCK_EX)` on Linux and macOS, and it excludes Python's `fcntl.flock` (tested on this Mac). Mixed processes on one store are safe |
| `<ws>/meta.json` | `indent=1`, ASCII-escaped. Keys: `remote`, `name`, `last_sync`, `published` | origin is never stored |
| `<ws>/events/<replica>/NNNNNN.jsonl` | One event per line: `,`/`:` separators, raw UTF-8, keys in the order `v, id, replica, deps, actor, op, issue, data[, where]`. Opened with `O_APPEND`, fsynced; a new segment starts at `BEAD_ROTATE_BYTES` | A torn tail is repaired before the next write and kept as `*.torn-<ns>` |
| `<ws>/events/<replica>/claims/<event-id>.json` | The attempt's events, compact lines, written atomically | Immutable. On mismatch the local copy is kept as `*.fork-<ns>` |
| `<ws>/pending/<id>.json`, `<id>.command` | The pending attempt and its receipt | The receipt format is private to the implementation (§5, row 21) |
| `<ws>/sync.git` | `git init --bare --template= --shared=0600 --object-format=<project's>`. Maintenance is off via the environment (`gc.auto=0` and the rest of `SYNC_REPO_CONFIG`) | Rebuilt as `sync.git.damaged-<ns>` when damaged |
| Temporary files | `.<name>.<pid>.tmp` beside the target | Readers and sync ignore them, along with every non-`.json`/`.jsonl` name |
| Modes | 0700 directories and 0600 files throughout the store, whatever the umask | Files written into a checkout (`.beadroll.json`, `export -o`) follow the umask |
| `.beadroll.json` (committed to users' repositories) | `indent=1` plus `"\n"`, ASCII-escaped. Existing keys keep their order; new keys are appended | A Rust `init --tracked` must produce the same bytes as Python, or users see spurious diffs |

The published tree on `refs/beads/journal` (or the tracked `ref`) holds exactly these entries:

```
manifest.json                            {"format": 1, "workspace": "<id>"}\n   (sorted keys, ", " and ": ")
events/<replica>/NNNNNN.jsonl            byte copies of the segments
events/<replica>/claims/<event-id>.json  byte copies of accepted attempts
```

Commits are made with `commit-tree`, so no signing config applies:
- subject `bead <claim|fence|sync> <12 hex>`;
- author and committer `bead <bead@localhost>`;
- parent: the fetched head.

They are pushed with `--porcelain --no-verify` as a plain fast-forward. Each process fetches into
its own `refs/beads/fetch/<pid>-<8 hex>` and records the result in `refs/beads/remote`.

The manifest bytes matter beyond tidiness. `publish` skips the push when the tree is unchanged. If
Rust spelled the manifest differently, every Python/Rust pair would push alternating manifests
forever.

### 1.2 The CLI contract

**Commands.** The 27 commands and their flags are unchanged:

| Group | Commands |
|---|---|
| Issues | `init`, `import`, `export`, `create`, `show`, `update` |
| Ownership | `claim`, `release`, `takeover`, `claims` |
| Lifecycle | `close`, `reopen`, `delete` |
| Queries | `list`, `ready`, `blocked`, `search`, `lint` |
| Relations | `dep`, `comments` |
| Sync and history | `sync`, `log`, `info`, `machines` |
| Agent help | `help`, `prime`, `onboard` |

**Global flags.**
- `--json` and `--actor NAME`/`--actor=NAME` are recognised anywhere before `--`.
- `--version`/`-V` and `-h`/`--help` work only when given alone.
- `bead` with no arguments prints the overview.

**Output.**
- With `--json`, stdout holds exactly one JSON document:
  - separators `", "`/`": "`;
  - raw UTF-8;
  - keys in Python's insertion order, with conditional keys present only when set.
- Human output is lines. Control characters print as lower-case `\xNN`/`\uNNNN`, including
  C0, DEL, C1, U+061C, U+2028/2029 and the bidi marks. PTY output equals pipe output.
- Help is plain text with no ESC bytes and starts `usage: bead <command>`.

**Errors and exit codes.**
- On error stdout is empty.
- stderr holds `{"error": {"code": …, "message": …}}` (ASCII-escaped) or `Error: …`.
- Exit codes:
  - 0: success, and also when stdout is closed;
  - 1: error;
  - 2: `USAGE`;
  - 98 and 99: the crash hooks.

**Error codes.** Every code `bead prime` lists, plus the rest the code raises: `CONFIG`,
`NO_WORKSPACE`, `NOT_INITIALIZED`, `NO_REMOTE`, `IDENTITY`, `KEY_SPACE`, `GIT_ERROR`,
`MANIFEST_MISSING`, `WORKSPACE_MISMATCH`, `FORMAT_UNSUPPORTED`, `JOURNAL_FORK`, `ATTEMPT_FORK`,
`USAGE`. Messages keep their Python text, because tests and agents match substrings of them.

**Environment.** The variables are in Appendix C. Their meanings, defaults and parsing (for
example `BEAD_NET_TIMEOUT` is a float in seconds, `BEAD_HOME` must be absolute after `~`
expansion) are unchanged.

**Static texts.** The overview, `prime` and `onboard` texts are copied verbatim. A test keeps the
onboard block in `AGENTS.md` equal to `bead onboard`.

### 1.3 Behaviour

Every decision in `REVIEWING.md` §2 stands as written. The protocol constants come across exactly:

- **Retries.** Sync and each ownership command make exactly 12 rounds, with
  `pause(U(0.1, 0.4) × attempt)` after every failed round. The sleep happens outside the store
  lock.
- **Timeouts.** On a fetch timeout bead kills git's whole process group; on a push timeout it kills
  only the client, so the outcome stays uncertain.
- **Push outcomes.** The outcome comes from git's porcelain status line for our ref, never from
  `remote:` text. The `refused`/`rejected`/`uncertain` classification uses the same tokens as now.
- **Repacking.** The thresholds are 1,000 loose objects or 40 packs, and `prune` removes only
  objects older than an hour.

The README's *What the journals tolerate* and *What else holds* sections are the behavioural
checklist the port is reviewed against.

### 1.4 What may change

- **Internals.** Anything internal can change, including how many git processes a command starts,
  as long as their observable effects are the same.
- **Two kinds of text.** Argument-parser error texts and per-command help layout come from clap,
  not argparse. No test pins them beyond the `usage: bead <command>` line and the help phrases the
  catalogue lists; the error code `USAGE` and exit 2 are what agents use.
- **Receipts.** The receipt signature format is private. Run `bead sync` before upgrading a machine
  so that no Python receipt is left pending (§8).
- **Hostile input.** Where Python's behaviour on hostile input is an accident, the port may differ.
  Each case needs a decision recorded in the ledger in §5, not a silent drift.

## 2. Architecture

### 2.1 Crates

```
beadroll-core ──┐
                ├──▶ beadroll-store ──▶ beadroll-sync ──▶ beadroll  (lib + `bead`)
beadroll-git  ──┘
beadroll-test-support (dev-dependency of every crate's tests)      xtask (dev tooling)
```

| Crate | From Python | Owns | Depends on | Consumers |
|---|---|---|---|---|
| `beadroll-core` | `model.py`, `queries.py`, parts of `base.py` | Python-compatible JSON; Python text semantics; event ids and times; event validation; journal settling (duplicates, quarantine, placeholders); causal order; replay; state; readiness, blockers, descendants, cycles; issue views; Beads import planning and export; key minting; reading a published tree. **No I/O, processes, clock or environment.** | `indexmap`, `sha2`, `thiserror` | studio; all crates |
| `beadroll-git` | `run_git` and the git half of `base`/`machine`/`sync` | **The only code that starts git.** The runner: own session, timeout with reap or no-reap, output captured to unlinked files, signal names. The environments for the checkout and for `sync.git`. Typed commands. Push-outcome and local-damage classification. Remote URL rules (`without_password`, `normalize_remote`, `is_local_path`). | `tempfile`, `rustix`, `libc`, `regex-lite`, `thiserror` | store, sync, optionally studio |
| `beadroll-store` | `machine.py`, the loaders in `model.py` | This machine: `BEAD_HOME`; the store's files and modes; lock; atomic writes; segments and torn tails; attempt, pending and receipt files; identity and aliases; workspace discovery and remote resolution; actor detection; `view`; the failpoint registry | core, git, `rustix`, `uuid`, `thiserror` | sync, `beadroll`, local read-only tools |
| `beadroll-sync` | `sync.py` | Transactions (`Tx`, `writing`); `Link` (fetch, merge, tree, publish, maintain, rebuild); `synchronise`; the ownership protocol (`coordinated`, receipts, fences, settling); `publish_soon`; unpublished counts | core, git, store, `uuid`, `fastrand`, `thiserror` | `beadroll` |
| `beadroll` | `commands.py`, `cli.py` | The `bead` binary. `commands/` maps typed requests to typed outcomes; `cli/` handles parsing, rendering, output streams and exit codes; `sys` holds the two audited unsafe spots (§2.3f). `main.rs` is one line. | all of the above, `clap`, `rustix`, `libc` | the development machines (`cargo install --path crates/beadroll`, §8) |
| `beadroll-test-support` | `tests/support.py` | `Sandbox`, `Machine`, git and hook fixtures, fake ssh/ps/git writers, failpoint and handshake helpers, event builders | core, `tempfile` | tests (`publish = false`) |
| `xtask` | `check.sh` and glue | `check`, `oracle`, `diff`, `inventory`, `timing`, `golden` (§4) | — | maintainers, CI (`publish = false`) |

**Why this split and not finer or coarser.**
- Each boundary is either a consumer boundary or an ownership rule this codebase already has:
  - studio needs the pure data crate without git, a filesystem or tokio;
  - the house rule is "exactly one crate runs git", following hushgit-git;
  - jig-state is the house precedent for a state crate that owns an append-only journal.
- Splitting store from sync lets a local read-only tool, such as a status line showing "3 ready,
  1 held", link without the protocol.
- `core` and `git` compile in parallel; the rest is a short chain.
- A finer split (JSON, queries or the failpoint registry as separate crates) would widen the
  published API without a consumer.
- Commands stay in the binary crate, where jig keeps them. No consumer needs an in-process command
  API yet, and its library target is documented as existing for the binary and its tests.
- All crates share one version from `[workspace.package]` and none is published (§8).

### 2.2 Rules that hold in every crate

- **No ambient state in libraries.** Only `beadroll`'s `cli` reads the process environment, the
  working directory and the clock offset. It builds explicit values: `Settings` (home, rotate
  bytes, net timeout, repack thresholds, overrides), `Actor` (from a pure `detect_actor(&EnvSnapshot)`)
  and the checkout path, then passes them down. This is what lets studio and in-process tests use
  the crates. It also matters because `std::env::set_var` is `unsafe` in edition 2024; tests set
  environments on child processes only.
- **Errors.**
  - Each crate has precise `thiserror` enums whose variants are only those its operations can
    produce (the runlimit rule).
  - Each error has `fn code(&self) -> Code`. `Code` is the closed enum of wire codes, and
    `Code::as_str()` gives the string.
  - `Display` is the Python message text.
  - `std::io::Error` is wrapped with its path at the call site. IO messages read
    `"{strerror}: {path}"`, without Rust's `(os error N)` suffix, and a failed rename names its
    target, as Python's `filename2` does.
- **No panics outside tests.**
  - Libraries deny `unwrap_used`, `expect_used`, `panic`, `todo` and `unimplemented`.
  - Indexing that cannot be shown safe uses `get`.
  - Replay is total. Every input is either applied or reported, never an abort; Python's "last
    resort" `except` becomes explicit cases.
- **Determinism.**
  - No `HashMap`/`HashSet` iteration order reaches output, a file or an event.
  - Issues are kept in insertion (replay) order, because `detail.dependents` and other outputs
    depend on it.
  - Objects keep Python dict order; anything else is sorted explicitly.
- **No `print!`/`println!`/`eprintln!` anywhere.** They panic on `EPIPE`/`EIO`. The CLI writes
  through one output module that turns a broken pipe or hung-up terminal into a quiet exit 0
  (§2.3f).
- **Unsafe code** is denied workspace-wide, with two documented exceptions and a `// SAFETY:` on
  each block: `beadroll::sys` (a pre-`main` stdio probe and ignoring SIGXFSZ) and
  `beadroll_git::runner` (resetting SIGXFSZ in git children).
- **House layout.**
  - No `mod.rs` files (`clippy::mod_module_files`).
  - Files warn at 500 lines and must stay under 800 (jig's file budget).
  - Each crate has an `AGENTS.md` with Purpose / Key entrypoints / Edit here for X / Invariants /
    Common commands.

### 2.3 Key decisions

**a. JSON is `beadroll-core::json`, Python-compatible, not serde_json.**

serde_json cannot keep the semantics journals depend on:
- **Integers.** Python integers are unbounded. serde_json turns `10**30` into a lossy float unless
  `arbitrary_precision` is enabled.
- **Key order.** Object order is Python dict order. serde_json needs `preserve_order` for that.
- **Lone surrogates.** An escaped `\ud800`, or the raw bytes ED A0 80, must each read as one U+FFFD
  and be reported. serde_json rejects the escape, and `from_utf8_lossy` gives three U+FFFD for the
  raw bytes.
- **Floats** must print as Python `repr()` prints them (`1e-05`, `1e+16`).
- **Five dialects are needed:**

  | Dialect | Python call | Used for |
  |---|---|---|
  | compact | `separators=(",", ":")` | journals, attempts, export |
  | spaced | default separators | CLI results; with `sort_keys`, the manifest |
  | ASCII-escaped spaced | default `ensure_ascii` | errors, `--version`, `replica.json` |
  | `indent=1` ASCII | `indent=1` | `meta.json`, `aliases.json`, `moved.json`, `.beadroll.json` |
  | canonical | `sort_keys=True`, compact | quarantine comparison |

- **Feature unification.** `arbitrary_precision` and `preserve_order` are workspace-global, so they
  would leak into studio's serde_json, which also enables `raw_value`. They would change its map
  order and number handling for every crate in the build. A library must not do that to its
  consumers.

So core has a small strict parser and writer:
- The parser is iterative, so depth can never overflow the stack. It rejects `NaN`/`Infinity` and
  non-finite numbers, keeps the last value of a duplicate key at the first key's position, and
  keeps big integers as digits.
- `Value` keeps objects in an `IndexMap`.
- The writer takes a `Format { style, ascii, sorted }` covering the dialects above.
- It is about 800 lines with its tests, proven against CPython by `xtask diff json` (§4.2).

**b. Events and state are typed. Raw JSON survives only where Python keeps arbitrary data.**
- **Events.**
  - A journal line parses to `Value`.
  - Settling validates it into `Event`: an `EventId(u128)` whose ordering equals Python's string
    ordering, `deps`, `actor`, `where`, `v`, and an `Op` enum with typed payloads (`Create`, `Set`,
    `Claim`, …, `Unknown(String)`).
  - The validation messages are the Python ones (`event_problem`, `payload_problem`,
    `write_problem`).
  - The raw `Value` is kept for canonical comparison and `log`.
- **Integers.** Fields that Python types as `int` use `PyInt` (`Bool | Small(i64) | Big(digits)`),
  because replay accepts `true` where an int is due. The looser replay rule exists for histories
  written before `write_problem`, and those histories must mean the same on every machine. `PyInt`
  compares, sorts and displays like Python (`[PTrue]`).
- **Open-ended data stays `Value`:** `extra`, dangling dependencies and dependency metadata.

**c. The replay data layout favours speed, but the algorithm does not change.**
- **Layout.**
  - Events are interned to `u32` indices, and replicas to small indices.
  - Causal order is Kahn's algorithm over a `BinaryHeap<Reverse<EventId>>`, which is identical to
    Python's `heapq` on id strings.
  - Version vectors are small per-event vectors.
  - Ancestry over ownership events stays a shared, deduplicated bitset per event.
- **Correctness first.** Phase 1 ports `saw`, the three replay phases, registers, add-wins sets
  and `materialise_dangling` as they are, and proves them equal to Python differentially.
- **Optimisation later.** A reachability index or roaring bitmaps (`README.md`, *Known limits*)
  come only in Phase 4, and only if measurements ask for them.

**d. git stays the git binary. Calls are consolidated later.**
- **Why the binary.** Every repository in the house runs git as a subprocess, and beadroll's
  contract is "as git reads it": `insteadOf`, `includeIf`, credential helpers, `core.sshCommand`,
  translated messages, server hooks, SHA-256 repositories and reftable. The hardening tests pin
  that behaviour. gix or libgit2 would reimplement it, and the tests would be the first to notice.
  - gix also lacks a production push.
  - libgit2 brings a C build and only experimental SHA-256 support.
- **Phase 2 parity.** Phase 2 issues the same commands as Python. The environment is built
  exactly as now:
  - repository-location variables stripped (`REPO_LOCATION_ENV`);
  - `GIT_DIR`, `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0` and the bead identity set;
  - the checkout's transport settings carried through `GIT_CONFIG_COUNT`, with resettable lists
    emptied first so that no header is sent twice;
  - the default `GIT_SSH_COMMAND` set only when the user has none;
  - `SYNC_REPO_CONFIG` last.
  - git is found on `PATH`; tests rely on PATH shims, so no new variable is needed.
- **Phase 4 consolidation (Appendix B).**
  - One `rev-parse` and one `config --get-regexp` for discovery.
  - No `check-ref-format` for the default ref.
  - One long-lived `cat-file --batch` for every object read.
  - `update-ref --stdin` transactions.
  - Hashing each local file once per command.
  - This takes a publishing write from 26–27 git processes to about 8.
- **Optional in-process objects.** Writing blob, tree and commit objects in-process (loose objects,
  SHA-1/SHA-256) would save about five more processes. It is done only if Phase 4 numbers justify
  it, and every sandbox is `git fsck --strict`-checked in tests.

**e. Durability and locking follow Python's trade-offs.**
- **fsync.** Rust's `File::sync_all`/`sync_data` on Apple platforms is `F_FULLFSYNC`. Measured here
  it costs 5.3 ms per call, against 0.03 ms for Python's `os.fsync`; a write with about eight
  syncs would gain 40 ms. bead calls `rustix::fs::fsync`, the same `fsync(2)` Python uses. The
  remote is the durable copy.
- **Atomic writes** use temp file, fsync, rename and cleanup on failure, with Python's temp names.
  Appends use `O_APPEND` and fsync.
- **Locking** uses `std::fs::File::lock` (Rust 1.89+), as hushgit does.

**f. Processes, signals and streams get explicit code where CPython supplied defaults.**
- **git children.**
  - Each child gets its own session (`process_group(0)`).
  - stdout and stderr go to unlinked temp files. bead waits for the child, not for its pipes,
    because helpers may hold stderr open. Output is read with `read_at` from offset 0.
  - stdin, when given, comes from a temp file.
  - The timeout is a waiter thread plus `recv_timeout`. On expiry: `killpg` for a fetch, `kill` of
    the client alone for a push.
  - A signal death is reported as `killed by SIGNAME`.
- **SIGXFSZ.** CPython ignores it, so a file-size limit becomes `EFBIG` and then an `IO` error.
  bead ignores SIGXFSZ at start-up, and the runner resets it to default in git children through
  `pre_exec`, so git still dies "killed by SIGXFSZ".
- **SIGPIPE.** Rust already ignores it.
  - Every write goes through the output module: `EPIPE`, or `EIO` on a character device, means
    exit 0 quietly.
  - When stderr fails, the command's status is kept: 1, or 2 for `USAGE`.
- **Closed descriptors.** Rust reopens closed fds 0–2 on `/dev/null` before `main`. Python sees
  `None`, and `import -` with a closed stdin must be an `IO` error.
  - A constructor in `.init_array`/`__mod_init_func` records whether fd 0 was open at exec. This
    probe was tested on this Mac with Rust 1.99: it tells `0<&-` apart from `< /dev/null`.
  - Closed stdout and stderr keep std's `/dev/null` behaviour, which matches Python's.
- **Arguments** are read with `args_os`. Text arguments stay `OsString` until they are used, so
  non-UTF-8 input gives exactly `{"code": "VALIDATION", "message": "text is not valid UTF-8"}`
  with exit 1, not a clap error.

**g. Fault injection uses failpoints compiled into every build and activated by the environment.**
- **Syntax.** `BEAD_FAILPOINTS='push.after-ok=exit(99);backoff=skip'` is parsed once at start-up.
  A malformed spec is a `CONFIG` error, so a test can never pass because its fault silently failed
  to fire.
- **Legacy hooks.** `BEAD_TEST_CRASH_BEFORE_PUSH`/`AFTER_PUSH` remain aliases for
  `ownership.before-publish=exit(98)` and `ownership.after-publish-ok=exit(99)`. The Python
  oracle, `infra/drive.py` and reviewers use those names against normal builds.
- **Why every build.** The shipped binary is the tested binary. It keeps Python's model, where the
  hooks live in the product. The cost is one environment lookup.
- **Decided on 2026-10-09: every build.** A `failpoints` cargo feature would have kept them out of
  release builds, at the cost of a second build for every test command and for the oracle. §4.5
  lists the points and actions.

**h. The CLI uses clap, configured for argparse parity.**
- **Parsing.**
  - clap 4.5 with derive, as in jig, hushgit and sealtask.
  - `infer_long_args(true)`: argparse accepts unambiguous prefixes.
  - `args_override_self(true)`: argparse keeps the last value.
  - Explicit "given more than once" errors for `--parent`/`--epic`.
  - `ready`'s mutually exclusive scope (`ID`, `--epic` or `--parent`).
  - `ColorChoice::Never`, a help template that starts with lower-case `usage: bead <command>`, and
    `disable_help_subcommand` (`help` is a bead command).
  - Every clap error maps to `USAGE` with exit 2.
- **Pre-parse.** `--json`/`--actor` extraction and `--` handling stay as in Python, before clap
  runs.
- **Rendering.**
  - Each command returns a typed outcome.
  - The outcome renders to an ordered `Value` (Python key order, conditional keys).
  - The outcome also renders to human lines, through `visible()` escaping and a port of Python's
    `repr()` for conflict values.

**i. The remote seam waits for its second implementation.**
- **Shape now.** `Link` is written so that the protocol functions (`synchronise`, `coordinated`,
  `settle_*`, `merge`) depend on fetch, read-tree and push operations, not on `sync.git`
  internals. Merge works on `(path → blob)` maps, which also makes it unit-testable without git.
- **Trait later.** A `Remote` trait is introduced only with its second implementation: the
  in-memory simulated remote of Phase 5 (§4.8). This follows the house rules against inventing an
  abstraction before its slice and against a trait for a single implementation.

### 2.4 The library API for consumers

`beadroll-core` is the stable surface. The sketch below gives the shape; names settle in Phase 1:

```rust
use beadroll_core::{published::Tree, replay::replay, query::ReadyScope};

// Entries of a refs/beads/journal commit, read by the caller: (path, bytes).
let tree = Tree::from_entries(entries)?;           // checks manifest.json (format 1), workspace id
let state = replay(tree.journal());                // immutable; Send + Sync; wrap in Arc to share
for issue in state.issues_by_key() {               // bytewise key order
    let _ = (issue.key(), issue.title(), issue.status(), issue.priority(), issue.issue_type());
}
let ready = state.ready(ReadyScope::Subtree(epic_key), &Default::default()); // br's rule
let blockers = state.open_blockers(issue);
let cycles = state.cycles(beadroll_core::query::Edges::Blocking);
let jsonl = state.export_beads();                  // br 0.5.7-compatible records, for bridges
```

- **Reading a local store.** `beadroll-store` adds `Workspace::discover(checkout, &Settings)` and
  `view()`. The first starts at most three git processes; the second reads files only.
- **Construction.** Every constructor that checks an invariant returns `Result`.
- **Errors** carry codes, keys, event ids and origins, never issue text. This is studio's redaction
  rule and already true of Python's messages.
- **Runtime.** There is no async anywhere: consumers on tokio call through `spawn_blocking`, as
  studio already does for its graph.

## 3. Toolchain, dependencies, lints, profiles

```toml
# rust-toolchain.toml — exact compiler for local builds and CI; Cargo.toml's rust-version is the minimum.
[toolchain]
channel = "1.99.0"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

```toml
# Cargo.toml (workspace root, beside the Python package until the cutover)
[workspace]
resolver = "3"
members = ["crates/*", "xtask"]           # fuzz/ is its own workspace (cargo-fuzz)

[workspace.package]
version = "0.2.0"
edition = "2024"
rust-version = "1.99"
license = "MIT OR Apache-2.0"
repository = "https://github.com/bpcakes/beadroll"
publish = false                             # private repository: nothing goes to crates.io (§8)

[workspace.dependencies]
beadroll-core  = { path = "crates/beadroll-core" }    # one version for every crate, above
beadroll-git   = { path = "crates/beadroll-git" }
beadroll-store = { path = "crates/beadroll-store" }
beadroll-sync  = { path = "crates/beadroll-sync" }
beadroll-test-support = { path = "crates/beadroll-test-support" }
clap = { version = "4.5", features = ["derive"] }
fastrand = "2"
indexmap = "2"
libc = "0.2"
proptest = "1"
regex-lite = "0.1"                          # git message classifiers, transliterated from sync.py
rustix = { version = "1", features = ["fs", "process", "system"] }
sha2 = "0.10"                               # the version studio already builds
tempfile = "3"
thiserror = "2"
uuid = { version = "1", features = ["v4", "v7"] }

[workspace.lints.rust]
unsafe_code = "deny"                        # two documented exceptions, §2.2
missing_docs = "warn"
unreachable_pub = "warn"
rust_2018_idioms = { level = "warn", priority = -1 }

[workspace.lints.clippy]
cognitive_complexity = "warn"
mod_module_files = "warn"                   # CI adds -D clippy::mod_module_files
too_many_lines = "warn"
redundant_clone = "deny"
large_stack_arrays = "deny"
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
todo = "deny"
unimplemented = "deny"
dbg_macro = "deny"
print_stdout = "deny"                       # EPIPE-safe output only, §2.2
print_stderr = "deny"
allow_attributes_without_reason = "deny"
undocumented_unsafe_blocks = "deny"

[profile.dev]
debug = "line-tables-only"                  # test builds are dominated by linking (studio's note)
[profile.dev.package."*"]
debug = false
[profile.dev.package.beadroll-core]
opt-level = 2                               # replay-heavy tests run in-process; keeps the debug suite fast
```

Member manifests take `version.workspace = true`, `publish.workspace = true` and so on, and end
with `[lints] workspace = true`.

**clippy.toml.**

```toml
cognitive-complexity-threshold = 20
too-many-lines-threshold = 100
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
```

**rustfmt** uses the defaults (style edition 2024). **cargo-deny** uses runledger's `deny.toml`
(licences, advisories, sources).

**What the dependency list leaves out:**

| Left out | Instead |
|---|---|
| `serde`/`serde_json` in the product | §2.3a |
| tokio | Nothing is async |
| chrono/jiff | Event times come from UUIDv7 milliseconds through a 30-line civil-date function, checked against Python over random inputs |
| `regex` | Hand-written matchers for UUIDs, status shapes and URLs |
| anyhow | Every error carries a code |
| `wait-timeout` and `ctrlc` | A waiter thread is enough, and Python installs no signal handlers bead relies on |

`fastrand` exists only for backoff jitter.

**Recent Rust the design uses** (each compiled and checked on 1.99.0, edition 2024):
- let chains for the many validation ladders;
- `File::lock`/`try_lock` for the store lock;
- `LazyLock` for tables;
- `#[expect(lint, reason)]`;
- `Vec::extract_if` for draining pending lists;
- `Option::is_none_or`;
- `cfg_select!` for the per-platform pre-`main` probe;
- `CommandExt::process_group` and `FileExt::read_at` for the git runner;
- unsafe attributes (`#[unsafe(link_section)]`);
- `std::env::set_var` being `unsafe` in 2024, which is why libraries take explicit settings;
- resolver 3, which is MSRV-aware.

**Studio's toolchain.** Studio pins 1.97.1 with MSRV 1.94. When it links `beadroll-core`, its pin
moves to 1.99, in the same change (§7).

## 4. Tests

### 4.1 What there is

The suite has 458 tests in ten modules: 12,938 lines with `support.py`. They are almost all
black-box: about 1,600 call sites run `bead` as a subprocess, and in-process patching is rare
(`support.patched` appears 11 times).

**The fast baseline takes about 20 minutes.**
- Measured module by module, with the modules running in parallel:

  | Module | Time |
  |---|---|
  | `test_hardening` | 631 s |
  | `test_commands` | 259 s |
  | `test_protocol` | 216 s |
  | `test_replay` | 54 s |
  | `test_compat` | 43 s |
  | `test_storage` | 20 s |
  | the rest | under 1 s |

- The 12 tests over 15 s account for 372 s:
  - real backoff sleeps and stalled hooks: 50 s, 49 s, 17 s;
  - `BEAD_NET_TIMEOUT` waits: 50 s;
  - pure-Python replay over 720 delivery orders of 3,000 events: 43 s;
  - `br`/`bv` over real exports: 41 s;
  - git maintenance loops: 25 s, 22 s, 18 s;
  - 50 concurrent creates racing rotation: 23 s;
  - a rerun of `test_storage` under Python 3.12: 17 s;
  - thirteen sandboxes for the global git settings: 15 s.
- The rest is mostly the cost of each `bead` call:
  - Python start-up is 60 ms;
  - a publishing write starts 26–27 git processes and takes 630–730 ms.

**The slow modules.** `test_stress` takes 415–1,047 s. The full sweep takes about 1.7 hours,
extrapolated from review 7.

**Port classes.** From the three catalogues in `docs/rust-port/`:

| Class | Meaning | Count |
|---|---|---|
| A | black-box, runs unchanged against any binary | 308 (14 of them need the two crash hooks) |
| C-env | black-box once Rust implements an environment-triggered hook | 20 |
| B | pure logic, becomes Rust unit tests | 76 |
| C | needs an injection seam or an in-crate API | 34 |
| D | Python-specific; rewritten as the Rust property it stands for | 20 |

### 4.2 How we prove that nothing regressed

1. **The Python suite is the oracle.** `cargo xtask oracle` runs the Python modules with
   `BEAD_UNDER_TEST=target/debug/bead`. It skips, from a checked-in list derived from the
   catalogues:
   - the driver tests, which `runpy` the launcher and cannot target a binary;
   - the in-process-only tests, which would only re-test Python.

   About 330 tests run unchanged.
   - **Exit criterion for the engine phase:** 100% of them pass on Linux and macOS.
   - **Format checks for free:** the mixed tests that write with the binary and then inspect with
     Python's `replay` check that Rust-written stores are well formed.
2. **Every test is ported.** The cross-check is what makes the test port trustworthy, not just the
   implementation port:
   - The Rust `Machine` honours `BEAD_UNDER_TEST` exactly as `support.py` does.
   - Every ported black-box test must pass against the **Python** binary before it counts as
     ported. A ported test that fails against Python is a porting mistake in the test.
   - Tests that need new failpoints are marked `rust_only` and reviewed against the Python test
     they replace.
   - `cargo xtask inventory` maps every Python test id to a Rust test with the same name, kept
     under its module path with the finding id, e.g.
     `hardening::g12_retried_end_of_claim_is_published::…`.
   - The inventory fails on any unmapped test. D-class replacements are listed explicitly, so
     `FINDINGS.md` keeps pointing at real tests.
3. **Differential testing against CPython.**
   - `cargo xtask diff replay` builds journals:
     - every fixture the Python tests construct;
     - plus proptest-generated histories: valid ones, and hostile mutations of envelopes,
       payloads, duplicates, chains and ownership.
   - A stdlib-only Python driver (`xtask/python/replay_dump.py`) and the Rust core dump the same
     canonical report: issues with every field, integrity, pending, renames, chain breaks,
     conflicts, readiness and export. The two reports must be equal.
   - `cargo xtask diff json` does the same for parsing (values and error classes) and for every
     writer dialect against `json.loads`/`json.dumps`.
   - These run in CI with a bounded seed count until the cutover, and nightly at volume.
   - Any difference is either fixed in Rust or recorded as a decision in §5.
4. **Mixed fleets converge.** `Machine` takes a per-machine binary. The mixed-fleet suite runs the
   protocol scenarios and the first stress model with Python and Rust machines on one remote. It
   requires identical exports, `show` and `claims` everywhere, and no integrity rows. That is the
   rollout, rehearsed.
5. **Golden outputs, for what tests do not fully pin.** Examples: the whole human `show`, every
   help page, `info` keys.
   - `cargo xtask golden` captures them from the Python binary over scripted scenarios, with ids,
     times and paths normalised.
   - Goldens are generated from Python only, never from Rust. Regenerating a golden to make Rust
     pass is exactly what the house rules forbid.

### 4.3 Where each Python module goes

| Python | Tests | Rust | How |
|---|---|---|---|
| `test_replay` | 60 | `crates/beadroll-core/tests/replay.rs` (+ `tests/replay/*.rs`) | In-process against the core API (`Journal::add` takes any `Value` and records problems); `find_cycles` generic over an edge function for the four-node oracle; 2 CLI tests move to `beadroll` |
| `test_storage` | 9 | `crates/beadroll/tests/cli/storage.rs` | 5 black-box; 4 via failpoint handshakes (`pause-until`) |
| `test_compat` | 4 | core tests (2) + `cli/compat.rs` (2) | The real-export/`br`/`bv` test is `#[ignore = "needs br, bv and real Beads exports"]`, also run by `xtask check --real` |
| `test_commands` | 111 | `crates/beadroll/tests/cli/commands/*.rs` | 95 black-box, 10 crash-hook, 4 core, 1 `beadroll-sync` in-crate, 1 rewritten |
| `test_protocol` | 54 | `cli/protocol/*.rs` + `crates/beadroll-sync/tests/` | 32 black-box, 10 failpoint, 11 in-crate API tests (fence wins, `Link` outcomes, per-`Link` timeout), 1 core |
| `test_hardening` | 207 | `cli/hardening/{g01…g64,v1…v9}.rs` + unit tests in git/store/core | 172 black-box, 15 failpoint/API, 11 unit (the 15 `without_password` pairs, `normalize_remote`, pinned workspace ids, runner timeouts, `Workspace.remote`), 9 rewritten (§4.6) |
| `test_stress` | 2 | `cli/stress.rs`, `#[ignore = "stress: randomised models"]` | Same seeds and oracles; schedules run in parallel |
| `test_sweep` | 1 | `cli/sweep.rs` | Failpoints replace the `runpy` wrapper; the 12-schedule slice runs by default, `BEAD_SWEEP=full` runs all 660; schedules in parallel |
| `test_check` | 9 | `xtask` tests | The verification tooling must fail when a selected check fails |
| `test_hygiene` | 1 | `xtask` test | Same patterns over `git ls-files` |

**Test binaries.** Each crate has one integration-test binary: `tests/cli.rs` declares `mod
commands;` and so on, with no `mod.rs` files. That keeps link time down, which pancake-studio-beads
does the same way. Slow tests use `#[ignore = "reason"]`, the house convention.

### 4.4 `beadroll-test-support`

A direct port of `support.py`:

- `Sandbox`: a temp root, a bare `origin.git` and a seeded `main`.
- `Machine`: its own home, clone and identity.
  - `run`, `bead` (with `--json`), `show`, `ready`, `export`, `store`.
  - The environment is stripped of `CLAUDE*`, `CODEX*`, `BEAD_*` and `GIT_*` before
    `BEAD_HOME`/`BEAD_AGENT`/`BEAD_THREAD` are set.
- `BEAD_UNDER_TEST` support. The default is `env!("CARGO_BIN_EXE_bead")`, with a per-machine
  override for mixed fleets.
- Fixtures:
  - writers for pre-receive hooks (stall, gate, refuse, slow post-receive);
  - fake `ssh`, `ps` and `git` shims;
  - an HTTP git server driver for the transport tests;
  - a PTY helper;
  - a `ulimit` runner.
- Failpoint helpers: environment builders, and the `ready`/`go` file handshake.
- `json!`-style event builders for core tests.

**What changes from Python.**
- "No traceback" assertions become "no `panicked at`, exit status other than 101, no
  SIGABRT/SIGSEGV".
- `ulimit -n` values are recalibrated for Rust's descriptor use.
- Python-based skip probes are removed. They would silently skip, or pass wrongly, against Rust.

### 4.5 Seams: the failpoint list

The table merges the three catalogues: the sweep's seven boundaries, and the hooks the hardening
and protocol tests inject through patching or drivers today. Actions:

| Action | Effect |
|---|---|
| `exit(N)` | Exit with status N |
| `sigkill` | A real SIGKILL. The judge expects -9; an abort is a failure |
| `torn-sigkill` | Write half, fsync, then SIGKILL |
| `errno(ENOSPC\|EFBIG\|…)` | Fail the write with that error |
| `fail-reply` | Report a push that succeeded as failed: the lost acknowledgement |
| `error(CODE)` | Raise that error code |
| `pause-until(PATH)` | Touch `PATH.ready`, then wait for `PATH.go` |
| `sleep(MS)` | Sleep for MS milliseconds |
| `skip` | Skip the step |
| `offset(±DUR)` / `at(TIME)` | Shift or set the clock |

Each action takes an optional `nth(k)`; the default is the first hit.

| Point | Where | Replaces |
|---|---|---|
| `ownership.before-publish` / `ownership.after-publish-ok` | `coordinated`, around the attempt's publish | `BEAD_TEST_CRASH_BEFORE_PUSH` (98) / `AFTER_PUSH` (99), which stay as aliases |
| `push.before` / `push.after-ok` | Every push, including `publish_soon`, sync and fences | The sweep's `run_git` wrapper; G12 ("crash before *any* push") |
| `push.reply` | After git push returns | `run_git` patched to report a failure after success (lost-ack tests) |
| `publish.result` | `Link::publish` | `mock.patch.object(Link, "publish", side_effect=…)` |
| `ownership.after-confirm` | After `confirm_ownership`, before pending cleanup | The sweep's kill-confirm; q03; G54 |
| `atomic-write:<path-part>` | `write_atomic`, matched on the target path | kill-write-pending/-confirm; `V5_KillAtRename`; ENOSPC on `meta.json` (G29); the reader-sees-all-or-none handshake |
| `append.mid` | Inside a segment append | The kill-mid-append driver |
| `backoff` | The unlocked wait between rounds | `patched("pause", …)`; G11 t2; V2's "unlocked sleep"; retry-exhaustion speed |
| `claim.before-push` | Between fetch and push of a claim | G09's competing publish |
| `pending.after-list` | Between listing and reading `pending/` | G28 t2 |
| `journal.after-enumerate`, `view.before-load-pending`, `identity.before-link`, `publish.before` | Reader/writer races | test_storage drivers |
| `clock` | The UUIDv7 clock and torn/fork stamps | Drivers patching `time.time_ns` (f14 ≈ 2110, −48 h, −1 day); G47 t3 stays unique on equal stamps |

The tests that run Rust's in-crate APIs instead of the binary take the same registry through
`Settings`, not the environment.

### 4.6 Python-specific tests, and what each becomes

- **G13:** `.beadroll.json` is UTF-8 whatever the locale. It stays black-box across ISO-8859-1,
  US-ASCII and C, without the `python3` probe.
- **G30 t4:** nesting is capped at 100 on writes and imports, and a line a million levels deep is
  one integrity problem, never a stack overflow.
- **G52 t5:** `chmod 0` on the store gives `IO`. The test runs unconditionally.
- **G59:** help contains no ESC byte on a PTY, with `FORCE_COLOR=1`, or under `--json`, and it
  says `usage: bead create`.
- **V8_Python312:** becomes the CI matrix: Linux and macOS, and the oldest supported git (2.40)
  beside the newest.
- **Checks kept in other classes:**
  - G48 (SIGXFSZ) keeps its property, using the code in §2.3f.
  - The "no Traceback" and "Exception ignored" checks become the panic and signal checks of §4.4.

### 4.7 Speed

**Targets** (measured and recorded as evidence by `cargo xtask timing`, following jig's rule that
timing is evidence, not a test gate):

| Suite | Rust target | Python now |
|---|---|---|
| Fast suite, this Mac (16 cores) | ≤ 2 min | ≈ 20 min |
| Fast suite, a 4-core CI runner | ≤ 6 min | |
| Stress | ≤ 10 min | 7–17 min |
| Full sweep | ≤ 20 min | ≈ 1.7 h |

**How.**
- **nextest**, process-per-test across all cores, with the version pinned in
  `.config/nextest.toml` as in jig. Process-per-test also isolates environments.
- **Test groups** that limit concurrency for the git-maintenance and descriptor-limit tests.
- **`slow-timeout`** so that hangs fail, not stall.
- **`retries = 0`.** A flaky protocol test is a bug.
- **Cheap `bead` calls:**
  - start-up is about 1 ms instead of 60;
  - replay is in-process and fast;
  - `opt-level = 2` for core in test builds.
- **No real waits for synchronisation.** Interleavings that Python got from real backoff sleeps
  use `pause-until` handshakes, which are deterministic and instant. Retry-heavy tests use
  `backoff=skip`. Timeout tests keep real but short timeouts: `BEAD_NET_TIMEOUT` of 0.25–1 s, with
  hooks stalling for twice that.
- **CPU-heavy oracles run in-process in core.** The 720 orders of 3,000 events are pure-Python CPU
  today: 43 s, against an expected sub-second.
- **The sweep and stress models run schedules in parallel**, each in its own sandbox.

### 4.8 Stress, sweep, simulation, fuzzing

- **Stress.** Both models port as seeded black-box runs with the same oracles:
  - the reference model of accepted claims, with generation +1;
  - convergence of exports, `show` and ready sets;
  - receipt- and blocker-based oracles for the concurrent model.

  The oracle reads state through `beadroll-core` instead of Python's in-process replay.
- **Sweep.** The 55 operation/boundary cells that apply (of nine operations × seven boundaries),
  each with 12 ordered pairs of follow-ups, give 660 schedules, driven by failpoints instead of the
  `runpy` wrapper. The judging is the same:
  excused exits 98, 99 and -9, and the codes `UNRESOLVED`, `REMOTE_UNREACHABLE` and
  `SYNC_CONFLICT`. The slice runs in the fast suite; the full sweep runs nightly.
- **Simulation (Phase 5, recommended).** Introduce the `Remote` trait together with an in-memory
  remote that has fault schedules: lost acknowledgements, stalls, refusals, crashes at every
  boundary, partitions. The real store files sit in temp directories. This lets the ownership
  protocol be explored over 10⁵–10⁶ schedules in minutes, with the same invariants as the sweep:
  at most one confirmed owner, monotone generations, every accepted claim visible. The real-git
  sweep stays the end-to-end check.
- **Fuzzing.** proptest is the house tool and drives the differential and property tests:
  - replay is invariant under delivery order;
  - JSON round-trips in every dialect.

  `cargo-fuzz` (installed here) covers the JSON parser, the segment reader and the Beads importer
  nightly with a time budget. Its corpus is seeded from real exports, which are not checked in.

### 4.9 CI

The workflow follows the house shape:
- `contents: read`, concurrency groups, `merge_group`, and actions pinned by SHA;
- `setup-rust-toolchain` with `rust-cache`;
- nextest installed with `taiki-e/install-action`.

| Job | When | What |
|---|---|---|
| fmt | every push and PR | `cargo fmt --all -- --check` |
| clippy | every push and PR | `--workspace --all-targets --all-features --locked -- -D warnings -D clippy::mod_module_files` |
| test | every push and PR | nextest on ubuntu-latest and macos-latest; `cargo test --doc` |
| deny | every push and PR | cargo-deny |
| oracle | every push and PR, until the cutover | Python 3.14 + `xtask oracle` against the built binary |
| diff | every push and PR, until the cutover | bounded `xtask diff replay` and `xtask diff json` |
| mixed-fleet | every push and PR, until the cutover | the mixed Python/Rust scenarios |
| nightly | schedule or dispatch | stress; full sweep; volume differential runs; cargo-fuzz smoke; git-version matrix (2.40 and latest) |

`./check.sh` survives as a thin wrapper around `cargo xtask check [--stress] [--real]`, because
`AGENTS.md`, `CONTRIBUTING.md` and `REVIEWING.md` all point at it.

## 5. Python semantics the port keeps, or decides not to

These are the places where "the same as Python" means CPython's semantics, not Rust's defaults.
Each is implemented in `beadroll-core::text` or `json` and covered by the differential tests. Rows
3 and 6 were decided on 2026-10-09: reject. Every row that departs from Python goes into
`REVIEWING.md` §2 when the Rust implementation lands.

| # | Python behaviour | Where it shows | Rust | |
|---|---|---|---|---|
| 1 | `isinstance(True, int)`: replay accepts booleans as `priority`, `estimated_minutes` and `generation`, and `"v": true` equals 1 | Replay of histories that predate `write_problem` | `PyInt::Bool`; writes still refuse booleans | keep |
| 2 | Integers are unbounded | Extras, imports, generations | Integers beyond `i64` kept as digit strings | keep |
| 3 | Floats print as `repr()`; `1e400` parses to infinity, and export then raises `ValueError` (`allow_nan=False`) | Extras from imports | `repr`-compatible formatting; non-finite is "not a JSON number" | reject (decided) |
| 4 | Lone surrogates, escaped or raw `ED A0 80`, read as one U+FFFD each, with an integrity row | Journal and attempt lines | In the parser | keep |
| 5 | A duplicate key keeps its last value at the first key's position | All JSON | Same | keep |
| 6 | `UUID_RE.match` with `$` accepts a trailing newline in ids and deps | Hostile events only | Reject: ids and deps must match the UUID shape exactly | reject (decided) |
| 7 | Readable nesting depends on the interpreter (about 1k–10k levels); writes and imports are capped at 100 and 98 | Parsing | Iterative parser with a 10,000-level ceiling; deeper is an unreadable line; caps kept | keep |
| 8 | `str.strip()`/`isspace()` use Python's Unicode set (U+001C–001F count as space); `bytes.strip()` uses ASCII | `split_list`, comment emptiness, blank JSONL lines | `text::py_strip` and friends | keep |
| 9 | `str.lower()` is full Unicode case mapping | `search`, `--title-contains` | `to_lowercase`, confirmed by `diff` | keep |
| 10 | `os.path.realpath` is lenient and pure Python, with no case folding | Remote resolution and comparison | Port the algorithm; never libc `realpath` | keep |
| 11 | `repr()` of values in `show`'s conflict lines: quote choice, escapes, printability | Human output | Port of the `repr` rules; printability table generated from CPython | keep |
| 12 | `int()` accepts `" +5 "`, `"5_000"` and non-ASCII digits | `--estimate`, `--limit` | `text::py_int` | keep |
| 13 | argparse: prefix abbreviations, last value wins, its own error texts and help layout | CLI | clap configured as in §2.3h; texts differ where nothing pins them | keep behaviour |
| 14 | `str(True)` is `True` in human lines (`[PTrue]`) | `list`, `ready`, `show` | `PyInt: Display` | keep |
| 15 | `OSError` reads `"{strerror}: {filename}"`, and a rename names its target | IO errors | Paths attached at every call; libc `strerror` text | keep |
| 16 | `json.dumps` dialects | Everywhere | `json::Format` | keep |
| 17 | CPython ignores SIGXFSZ; children get the default | File-size limits | §2.3f | keep |
| 18 | Closed fds at exec give `None` streams; `-` with no stdin is an error | stdio | §2.3f | keep |
| 19 | 12 rounds; `U(0.1, 0.4) × attempt` | sync and ownership | Same | keep |
| 20 | Names: `refs/beads/fetch/<pid>-<hex8>`, `.NAME.PID.tmp`, `*.torn-<ns>`, `*.fork-<ns>`, `sync.git.damaged-<ns>` | Store, `sync.git` | Same | keep |
| 21 | The command receipt is `json.dumps` of argparse's `Namespace` | `pending/*.command` | Rust's own signature. A Python receipt is not recognised after the upgrade, so the retry runs as a new command and sees the accepted state | change (documented; sync before upgrading) |

## 6. Phases

Two tracks run side by side from Phase 0:
- **implementation**, Phases 1–2;
- **the test port**, Phase 3. Ported tests are validated against the Python binary first, so they
  are ready as the spec before the code they test exists.

Sizes are relative: S, M or L. The whole port is roughly 9–11k lines of Rust for the
implementation and 15–18k for the tests.

**Phase 0 — Foundations (S)**
- **Deliverables:**
  - the workspace and the empty crates, each with its `AGENTS.md`;
  - the toolchain pin, lints, `deny.toml` and `.config/nextest.toml`;
  - the CI jobs;
  - `beadroll-test-support`;
  - xtask with `oracle`, `inventory` and `timing`;
  - the oracle skip list;
  - the test inventory seeded from the catalogues.
- **Exit:** CI is green on the skeleton; the first ported tests pass against Python's `./bead`;
  `xtask oracle` runs the Python suite against any binary.

**Phase 1 — Core (L)**
- **Deliverables:**
  - `json` and `text` (§2.3a, §5);
  - ids and times; event validation;
  - the journal; causal order; replay; state;
  - queries: readiness, blockers, descendants, Tarjan + Johnson cycles;
  - views; Beads import planning and export (comment ids from 2⁴⁷); key minting;
  - published trees;
  - the two rejections of §5 (rows 3 and 6) applied to the Python implementation too: ids and
    deps matched in full against `UUID_RE`, and `loads_strict` refusing non-finite numbers, each
    with a test and a `FINDINGS.md` row. A mixed fleet then agrees even on hostile input, and the
    differential harness needs no list of expected differences.
- **Exit:**
  - all B-class tests are ported and green;
  - `xtask diff replay`/`json` are clean on every Python fixture and on at least 100k generated
    cases;
  - replay of 10k events stays within its budget (Appendix A).

**Phase 2 — Engine and CLI, in vertical slices (L).** Each slice raises the oracle's pass count,
which is the progress measure.

| Slice | Scope |
|---|---|
| 2a | The git runner; store layout, files, lock, identity, workspace discovery; `Tx`/`writing`; `Link` fetch/merge/tree/publish; `synchronise` and `publish_soon`; `init`, `create`, `show`, `list`, `ready`, `sync`; JSON and human rendering; the output module; exit codes |
| 2b | Every other non-ownership verb, `import`/`export`, `info`/`log`/`machines`/`claims`, `help`/`prime`/`onboard`, unpublished counts |
| 2c | The ownership protocol (`claim`, `release`, `takeover`, `update --claim`, owner `close`/`delete`), receipts, fences, settling, the failpoint registry, maintenance, rebuild |
| 2d | The hardening surface: URLs and credentials, transport config, damaged `sync.git`, refusal classification, stdio, signals, locales, PTY, modes, ENOSPC/EFBIG |

- **Exit:** `xtask oracle` is 100% green on Linux and macOS, and the mixed-fleet smoke passes.

**Phase 3 — Test port complete (L; runs from Phase 0)**
- **Deliverables:**
  - every module ported (§4.3);
  - failpoint-based C tests;
  - D-class replacements;
  - stress and sweep, slice and full;
  - the mixed-fleet suite;
  - `--real` compatibility with `br`/`bv` on this Mac's real exports.
- **Exit:**
  - `xtask inventory` reports zero unmapped tests;
  - every ported black-box test passes against both binaries;
  - the fast-suite timing is recorded against §4.7.

**Phase 4 — Performance (M)**
- **Deliverables:**
  - git consolidation (Appendix B);
  - replay profiling at 10k, 50k and 200k events, including ownership-heavy histories;
  - the reachability index only if the numbers demand it;
  - budgets recorded by `xtask timing`.
- **Exit:** every suite and the oracle stay green, and each budget in Appendix A is met or
  explained.

**Phase 5 — Reliability beyond parity (M; may follow the cutover)**
- **Deliverables:** the `Remote` seam and the simulated remote (§4.8), nightly fuzzing, a coverage
  report (`cargo llvm-cov`).

**Phase 6 — Studio (M):** §7.

**Phase 7 — Cutover (S)**
- **Deliverables:**
  - a read-only differential on a **copy** of the real store. Run `show`, `list`, `ready`,
    `export`, `info`, `log` and `claims` under a scratch `BEAD_HOME` holding that copy, against
    both binaries. Never a write: the copy still routes to the real remote.
  - `infra/drive.py` and `soak.py` with the Rust binary on the four machines and the throwaway
    repository;
  - documentation (§8);
  - the last Python commit tagged, then Python removed;
  - version 0.2.0 tagged, and the development machines upgraded (§8).
- **Exit:** the definition of done in `AGENTS.md`: the fast baseline is green, the docs are
  updated, and the change is on `main`.

## 7. pancake-studio

Today studio reads the committed `.beads/issues.jsonl` from a shallow bare clone of an
operator-approved branch:
- `pancake-studio-beads` parses it into an immutable graph;
- the worker stores a normalised projection in Postgres;
- the HTTP side queries it inside `spawn_blocking`;
- refreshes poll the branch tip every 120 s.

beadroll keeps nothing on branches, so the source becomes `refs/beads/journal`.

1. **Bridge.**
   - Studio's reader fetches the coordination ref. This needs a policy change: `CanonicalRef`
     currently allows only `refs/heads/*`.
   - The ref comes from the branch's `.beadroll.json` `ref` key when present, and is
     `refs/beads/journal` otherwise.
   - Every commit's tree holds the complete journals, so `--depth=1` suffices. The ref only
     fast-forwards, fences included, so studio's ancestry check still holds.
   - The reader passes the tree's entries to `beadroll_core::published::Tree`, replays it, and
     feeds `state.export_beads()` to the existing `pancake-studio-beads` parser.
   - Nothing downstream of the adapter changes. Its br-0.5.7 fixtures already describe that
     format.
2. **Native.** Studio maps `State` directly to its normalised graph and drops the JSONL round trip.
   It keeps its own rules:
   - limits;
   - all-or-nothing validation;
   - privacy (design, notes, comments, labels and assignee never reach member output);
   - canonical agreement and plan digests.

   beadroll provides typed, exact text. The digests stay studio's.
3. **Build.** Studio adds `beadroll-core` as a rev-pinned git dependency, as it does for `batter`,
   and moves its toolchain pin from 1.97.1 to 1.99. `sha2` 0.10, `thiserror` 2 and `uuid` 1 are
   already in its graph. This repository is private, so every environment that builds studio
   needs read access to it: local machines, its CI runners and the container image builder. That
   means a deploy key or token, with `net.git-fetch-with-cli = true` so that cargo fetches through
   git's own credentials.

For the execution-runs branch: agents in devbox worktrees run `bead` instead of `br`. bead is built
for worktrees, and every write reaches the remote at once. Studio's 30-second tracker delta then
comes from the published ref, with each change attributed to an agent thread and a branch.

## 8. Distribution and cutover

**Where the code lives.** The port lives in this repository, beside the Python package, until the
cutover:
- the history, `FINDINGS.md` and the dogfooded tracker stay continuous;
- the tracker is keyed by this repository's origin URL.

**Distribution.** The repository is private and nothing has been released publicly; bead runs only
on the maintainer's development machines. The port therefore needs no public channel: no
crates.io, no prebuilt binaries, no Homebrew, and no Python packaging (`uv tool install` is
dropped). Every crate inherits `publish = false`, so nothing can reach crates.io by accident.
Public channels wait for the open-source milestone, with the deferred backlog.

| Machine | Install, or upgrade with the same command |
|---|---|
| With a checkout | `cargo install --locked --force --path crates/beadroll --root ~/.local`. The checkout's `rust-toolchain.toml` selects 1.99.0. |
| Without one | `cargo install --locked --force --git git@github.com:bpcakes/beadroll.git beadroll --root ~/.local`. This needs a toolchain of at least 1.99 and read access to the repository. |

Both put `bead` in `~/.local/bin`, where `infra/` expects it. The Rust implementation is version
0.2.0; Python's was 0.1.0. Each machine needs rustup; it already has git.

**Upgrading a machine.**
1. `bead sync`, which settles pending attempts (§5, row 21).
2. Install the Rust binary as above.
3. `bead info`: expect `integrity: []` and `pending_ownership: []`.

Machines can upgrade in any order. The mixed-fleet suite is the evidence.

**Documentation at the cutover.**
- `README.md`: Install (from a checkout, as above), the "small Python package" paragraph,
  *Tests and reviews*, start-up time.
- `AGENTS.md` and `CONTRIBUTING.md`:
  - layout and setup;
  - `support.patched` becomes failpoints and test-support;
  - the upgrade line ("rerun its install");
  - *Releasing* becomes: `CHANGELOG.md`, bump `[workspace.package] version`, `./check.sh --stress`,
    CI green on `main`, tag, and reinstall on each machine;
  - the onboard pointer, which still names `beadroll.py`.
- `REVIEWING.md`:
  - §1 becomes a map in crate terms;
  - §4 says "use failpoints" instead of "patch `pause`";
  - §2 gains the §5 decisions.
- `FINDINGS.md`: the Tests column points at Rust test paths, which `xtask inventory` can generate.
- `MIGRATING.md` (its install line) and `CHANGELOG.md`.

**Removal.**
- First tag the last Python commit (for example `python-0.1.0`; the repository has no tags yet),
  so the Python implementation stays one checkout away.
- Then delete the Python package, `tests/`, `pyproject.toml`, the `./bead` launcher and the
  Python CI workflow.
- `infra/` stays as stdlib-Python drivers of any `bead` binary, and can later move into xtask.

## 9. Risks

| Risk | Mitigation |
|---|---|
| Replay drifts from Python on some history | Differential testing (§4.2.3), the §5 ledger, and the mixed-fleet suite; no silent tightening |
| CLI or output drift (help, argparse quirks, key order) | The oracle, goldens from Python, and the exact-text lists in the catalogues |
| Process, signal and stream edge cases | §2.3f, the two audited unsafe spots, and the hardening tests that already pin each case |
| git fidelity (environment, transport config, porcelain, translations) | Keep the binary and the exact environment; the git-version matrix |
| Test-port fidelity | Ported tests must pass against Python; `xtask inventory` requires completeness |
| Hidden slowness (F_FULLFSYNC, debug builds, real sleeps) | §2.3e, the profiles, handshakes, `xtask timing` |
| Scope creep | Parity first. Improvements go to the tracker as issues for after the cutover. |
| Upgrade with pending attempts | `sync` first; `bead info` shows `pending_ownership` |
| Studio's builds cannot fetch the private repository | Read access for every environment that builds studio (§7) |

## 10. Decisions

Decided by the maintainer on 2026-10-09:

1. **This repository.** Rust lives beside Python until the cutover.
2. **Failpoints in every build**, activated by the environment (§2.3g).
3. **Reject both quirks:** non-finite numbers and ids with a trailing newline (§5 rows 3 and 6).
   Python is aligned in Phase 1.
4. **No public release.** The repository is private and bead runs only on the development
   machines. There is no crates.io, no binaries, no Homebrew, and `uv tool install` is dropped.
   Every crate has `publish = false`, and machines install from source (§8).
5. **MSRV 1.99** workspace-wide (`rust-version = "1.99"`, toolchain pinned to 1.99.0). Studio's pin
   follows when it links `beadroll-core`.

Still open, with a recommended default:

6. **`infra/`** stays stdlib Python, driving any `bead` binary, until there is a reason to move it
   into xtask.

---

## Appendix A. Measurements (this Mac, 2026-10-09)

- **Python per-command cost** (`bead` against a local bare remote, git 2.55, Python 3.14):

  | Command | Time | git processes |
  |---|---|---|
  | `--version` | 60 ms | 0 |
  | `ready`/`list`/`show` | 121–202 ms | 3 |
  | `info` | 130 ms | 4 |
  | `sync` | 437 ms | 21 |
  | `init` | 568 ms | 19 |
  | `create`/`comments add`/`close` | 633–732 ms | 26–27 |
  | `claim` | 733 ms | 27 |

- **git floor:**

  | Operation | Time |
  |---|---|
  | `fork`/`exec` | 1.7 ms |
  | a trivial git command | 4.7–4.9 ms |
  | `git fetch` from a local bare remote | 29.7 ms |
  | no-op `git push` | 13.6 ms |
  | `git ls-remote` | 14.0 ms |

- **Rust estimates from those numbers.** These are targets for Phase 4 to confirm.
  - Reads: about 15 ms.
  - Publishing writes: about 180 ms with the same calls, about 90 ms consolidated.
  - Replay of 10k events: ≤ 20 ms in a release build. Python takes 0.7 s.
  - 200k events: ≤ 0.5 s and ≤ 150 MB. Python takes 14 s and 412 MB.
- **fsync on APFS:** `File::sync_all` 5.26 ms, `sync_data` 5.64 ms (both `F_FULLFSYNC`), Python
  `os.fsync` 0.03 ms.
- **Locking:** a Rust `File::lock` holder blocks Python's `fcntl.flock(LOCK_EX | LOCK_NB)`.
- **Test suite:** the module times and the slowest tests are in §4.1. The sum of per-test durations
  is about 1,220 s.

## Appendix B. git invocations, and their consolidation

**Checkout side** (the user's cwd and environment):

| Python call | Purpose | After Phase 4 |
|---|---|---|
| `rev-parse --show-toplevel` | Repository root | One combined `rev-parse` (toplevel, absolute common dir, object format; `HEAD` and `--abbrev-ref HEAD` for `where`), falling back to single calls when the combined one fails |
| `rev-parse --path-format=absolute --git-common-dir` | Local id, without an origin | (combined) |
| `rev-parse --show-object-format` | The project's format | (combined) |
| `rev-parse HEAD`, `rev-parse --abbrev-ref HEAD` | `where` | (combined) |
| `config --get-regexp ^remote\.origin\.(push)?url$` and `config --null --get-regexp <transport keys>` | origin; carried transport settings | One `config --null --get-regexp` over both patterns |
| `cat-file --batch` (`HEAD:` and `origin/HEAD:.beadroll.json`) | Committed config | Unchanged; only when the tree has no `.beadroll.json` |
| `remote get-url NAME` | A remote given by name | Unchanged |
| `check-ref-format REF` | A tracked ref | Skipped for the default ref |
| `symbolic-ref`, `rev-parse --verify`, `merge-base --is-ancestor` | `show`'s "merged into the default branch" | Unchanged; only for closed issues |

**`sync.git` side:**

| Python call | Purpose | After Phase 4 |
|---|---|---|
| `init --bare …` | Created once | Unchanged |
| `rev-parse --show-object-format` | Format check | Read `extensions.objectformat` once |
| `fetch … +REF:refs/beads/fetch/<pid>-<hex>` | Fetch | Unchanged |
| `rev-parse <fetch ref>`, `cat-file blob TIP:manifest.json`, `ls-tree -r -z TIP`, `cat-file blob SHA` ×N, `rev-parse TIP^{tree}` | Reading the remote tree | One long-lived `cat-file --batch`; trees parsed in-process |
| `update-ref refs/beads/remote TIP`, `update-ref -d <fetch ref>`, `update-ref refs/beads/remote COMMIT` | Ref bookkeeping | `update-ref --stdin` transactions |
| `hash-object -w --no-filters --stdin-paths` (twice per sync), `hash-object -w --stdin` | Blobs | Once per command |
| `mktree` × (replicas + claims folders + 2) | Trees | `mktree --batch`, or in-process objects if Phase 4 justifies them |
| `commit-tree` | The commit | Unchanged |
| `push --porcelain --no-verify` | The push | Unchanged |
| `repack -a -d -q`, `prune --expire=1.hour.ago` | Maintenance | Unchanged |

## Appendix C. Environment variables

| Variable | Meaning |
|---|---|
| `BEAD_HOME` | Store root; absolute after `~` expansion; default `~/.local/state/beadroll` |
| `BEAD_WORKSPACE` | Overrides the workspace id (and bypasses the derived-id check) |
| `BEAD_REMOTE` | Coordination remote for this call |
| `BEAD_AGENT`, `BEAD_THREAD`, `BEAD_ACTOR` | Identity overrides; `--actor` wins |
| `CLAUDE_CODE_SESSION_ID`, `CLAUDECODE`, `CODEX_THREAD_ID`, `CODEX_SESSION_ID` | Harness detection; with both present, the nearest harness in the process tree (`ps`, tolerant of non-UTF-8) |
| `BEAD_NET_TIMEOUT` | Seconds (a float) for one fetch or push; default 45 |
| `BEAD_ROTATE_BYTES` | Segment rotation size; default 262,144 |
| `BEAD_REPACK_LOOSE`, `BEAD_REPACK_PACKS` | Maintenance thresholds; defaults 1,000 and 40 |
| `BEAD_TEST_CRASH_BEFORE_PUSH`, `BEAD_TEST_CRASH_AFTER_PUSH` | Crash hooks, exits 98 and 99; aliases of failpoints |
| `BEAD_FAILPOINTS` | New: §4.5 |
| `BEAD_UNDER_TEST` | Read by test harnesses only: the binary under test |
| `BEAD_SWEEP` | Read by the sweep only; `full` runs all 660 schedules |
| `BEAD_TEST_REMOTE`, `BEAD_DRIVE_HOSTS`, `BEAD_TEST_LOCAL_REPO` | Read by `infra/` only |

These are read only by the CLI crate and translated into `Settings`, never read inside a library.
