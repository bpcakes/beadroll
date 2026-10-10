# Porting beadroll to Rust

*Plan, 2026-10-09, revised after reviews 8 and 9. Baseline: `main` at 9867715 — beadroll 0.1.0,
format 1, Python 3.11+. The maintainer's decisions of 2026-10-09 are recorded in §10.*

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
  1. The existing Python suite drives the Rust binary through `BEAD_UNDER_TEST`. 338 of the 473
     tests run unchanged.
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
| `<ws>/pending/<id>.json`, `<id>.command` | The pending attempt and its receipt | Python-compatible receipt signatures throughout the port and rollback period (§2.3j) |
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
- **Receipts.** Request identity and receipt retirement rules stay compatible across Python and
  Rust processes on the same store, including rollback (§2.3j). Sync settles pending attempts;
  accepted command receipts can remain afterwards.

The README's *What the journals tolerate* and *What else holds* sections are the behavioural
checklist the port is reviewed against.

### 1.4 What may change

- **Internals.** Anything internal can change, including how many git processes a command starts,
  as long as their observable effects are the same.
- **Two kinds of text.** Argument-parser error texts and per-command help layout may differ from
  Python's: no test pins them beyond the `usage: bead <command>` line and the help phrases the
  catalogue lists, and the error code `USAGE` and exit 2 are what agents use. Phase 2b ported
  CPython 3.14's argparse all the same (§2.3h), so they do not differ, and the goldens hold every
  help page whole (§4.2).
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
| `beadroll` | `commands.py`, `cli.py` | The `bead` binary. `commands/` maps typed requests to typed outcomes; `cli/` handles parsing, rendering, output streams and exit codes; `sys` holds the two audited unsafe spots (§2.3f). `main.rs` is one line. | all of the above, `rustix` (with `termios`, for the width help pages wrap at), `libc`, `nix` (the password database, for `~NAME` in `BEAD_HOME`, without unsafe code) | the development machines (`cargo install --path crates/beadroll`, §8) |
| `beadroll-test-support` | `tests/support.py` | `Sandbox`, `Machine`, git and hook fixtures, fake ssh/ps/git writers, failpoint and handshake helpers, event builders | core, `tempfile` | tests (`publish = false`) |
| `xtask` | `check.sh` and glue | `check`, `oracle`, `diff`, `inventory`, `timing`, `golden` (§4) | — | maintainers, CI (`publish = false`) |

**Why this split and not finer or coarser.**
- Each boundary is either a consumer boundary or an ownership rule this codebase already has:
  - studio needs the pure data crate without git, a filesystem or tokio;
  - exactly one crate runs git: beadroll's own rule, modelled on hushgit-git, which owns
    hushgit's transport git calls (hushgit's CLI still runs git directly);
  - jig-state is the house precedent for a state crate that owns an append-only journal.
- Splitting store from sync lets a local read-only tool, such as a status line showing "3 ready,
  1 held", link without the protocol.
- `core` and `git` compile in parallel; the rest is a short chain.
- A finer split (JSON, queries or the failpoint registry as separate crates) would widen the
  published API without a consumer.
- Commands stay in the binary crate, as jig keeps its argument types and runners in its binary
  package. No consumer needs an in-process command API yet; the library target exists for the
  binary and its tests.
- All crates share one version from `[workspace.package]` and none is published (§8).

### 2.2 Rules that hold in every crate

- **No ambient state in libraries.** Only `beadroll`'s `cli` reads the process environment, the
  working directory and the clock offset. It builds explicit values: `Settings` (home, rotate
  bytes, net timeout, repack thresholds, overrides), `Actor` (from `detect_actor(&EnvSnapshot)`),
  the checkout path, and the environment git children inherit, from which `Link` builds
  `sync.git`'s (§2.3d); then it passes them down. This is what lets studio and in-process tests use
  the crates. It also matters because `std::env::set_var` is `unsafe` in edition 2024; tests set
  environments on child processes only. `detect_actor` is pure except in one case: with both a
  Claude and a Codex session variable set, Python walks the process tree with
  `ps -o ppid=,args= -p PID` from `PATH`, up to 12 levels. Rust does the same through a function
  the CLI passes in, so a fake `ps` drives that test against either binary.
- **Errors.**
  - Each crate has precise `thiserror` enums whose variants are only those its operations can
    produce (the runlimit rule, kept in batter's `runlimit/AGENTS.md`).
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
- **No `print!`/`println!`/`eprintln!` in the product crates.** They panic on `EPIPE`/`EIO`. The
  CLI writes through one output module that turns a broken pipe or hung-up terminal into a quiet
  exit 0 (§2.3f). Tests may print (`allow-print-in-tests`); xtask, a developer tool, allows
  `print_stdout` and `print_stderr` at its crate root.
- **Exemptions are crate-level attributes with a reason.** Cargo refuses `[lints]` entries beside
  `workspace = true` in a member. Crate-wide policy uses `allow`, not `expect`, because a
  crate-level `#![expect]` fails once the lint stops firing. `beadroll-test-support` is a library,
  so the `allow-*-in-tests` settings do not reach it: its root allows `unwrap_used`, `expect_used`,
  `panic` and `print_stderr` ("a failed fixture is a failed test").
- **Every crate root has `//!` docs**, binaries and test roots included, because `missing_docs`
  fires on them. Each `tests/<name>/main.rs` starts with `#![cfg(test)]`, so that clippy treats its
  helpers as test code, and helpers shared between test modules are `pub(crate)`
  (`unreachable_pub`).
- **Unsafe code** is denied workspace-wide, with two documented exceptions and a `// SAFETY:` on
  each block: `beadroll::sys` (a pre-`main` stdio probe and ignoring SIGXFSZ) and
  `beadroll_git::runner` (detaching the child session and resetting SIGXFSZ before exec).
- **House layout.**
  - No `mod.rs` files (`clippy::mod_module_files`).
  - Files warn above 500 lines and fail above 800 (jig's file budget, which studio and batter
    share).
  - Each crate has an `AGENTS.md` with jig's crate-guide sections: Purpose / Key entrypoints /
    Edit here for X / Invariants / Common commands. jig makes them optional; beadroll requires
    them.

### 2.3 Key decisions

**a. JSON is `beadroll-core::json`, Python-compatible, not serde_json.**

serde_json cannot keep the semantics journals depend on:
- **Integers.** Python integers have arbitrary precision, but Python 3.11+ defaults to a
  4,300-digit decimal conversion limit, inherited by `json.loads`/`json.dumps`. The port preserves
  that default boundary, including integers in extras; arbitrary precision does not mean an
  unlimited JSON token. serde_json turns `10**30` into a lossy float unless `arbitrary_precision`
  is enabled.
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
  keeps big integers as digits within the 4,300-digit decimal limit (sign excluded). A larger
  integer token is unreadable JSON, with the same line/attempt scoping as Python. Writers also
  check the limit, including values constructed in memory, before any store mutation; an import
  containing one over-limit integer fails whole. Floating-point tokens keep their separate rules.
- `Value` keeps objects in an `IndexMap`.
- The writer takes a `Format { style, ascii, sorted }` covering the dialects above.
- It is about 800 lines with its tests, proven against CPython by `xtask diff json` (§4.2).

**b. Events and state are typed. Raw JSON survives only where Python keeps arbitrary data.**
- **Events.**
  - A journal line parses to `Value`.
  - Settling validates it into `Event`: an `EventId(u128)` whose ordering equals Python's string
    ordering, `deps`, and an `Op` enum with typed payloads (`Create`, `Set`, `Claim`, …,
    `Unknown(String)`), read once. The payload is read leniently, because an event
    `payload_problem` refuses is never applied. `actor` and `where`, which Python keeps whole on
    issues, claims and comments, and `v`, read once for its check, come from the kept `Value`.
  - The validation messages are the Python ones (`event_problem`, `payload_problem`,
    `write_problem`).
  - The raw `Value` is kept for canonical comparison and `log`.
- **Integers.** Fields that Python types as `int` use `PyInt` (`Bool | Int`, an `Int` being
  `Small(i64) | Big(digits)`), because replay accepts `true` where an int is due. The looser replay
  rule exists for histories written before `write_problem`, and those histories must mean the same
  on every machine. `PyInt` compares, sorts and displays like Python (`[PTrue]`).
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
  - Each child gets its own session via `setsid` in the audited `pre_exec` path, matching
    `start_new_session=True` in `base.run_git`. Do not call `process_group(0)` first: `setpgid`
    retains the controlling terminal and making the child a group leader prevents `setsid`.
    Stable Rust 1.99's builder does not provide a stable `setsid` method; use the existing
    `rustix`/`libc` boundary with async-signal-safe calls only.
  - stdout and stderr go to unlinked temp files. bead waits for the child, not for its pipes,
    because helpers may hold stderr open. Output is read with `read_at` from offset 0.
  - stdin, when given, comes from a temp file.
  - No other descriptor reaches git: Python starts it with `close_fds=True`. bead marks every
    descriptor it was started with, beyond the standard three, close-on-exec once at start-up
    (`beadroll::sys`), so neither git nor `ps` inherits one; those bead opens itself are
    close-on-exec already. Linux does it in one `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)`, as
    CPython closes them with `close_range`; elsewhere, or on a kernel before 5.11, bead marks each
    descriptor `/dev/fd` or `/proc/self/fd` lists, or failing both every number below the soft
    limit on open files, as CPython's fallback does.
  - The timeout is a waiter thread plus `recv_timeout`. On expiry: `killpg` for a fetch, `kill` of
    the client alone for a push.
  - A signal death is reported as `killed by SIGNAME`.
  - The runner gate includes `V8_SyncUnderAControllingTerminalCannotPrompt`: a fake transport
    cannot open `/dev/tty` from a controlling PTY. Keep the fetch group-kill and uncertain-push
    tests as well; session detachment must not change their timeout outcomes.
- **SIGXFSZ.** CPython ignores it, so a file-size limit becomes `EFBIG` and then an `IO` error.
  bead ignores SIGXFSZ at start-up, and the runner resets it to default in git children through
  `pre_exec`, so git still dies "killed by SIGXFSZ".
  - An append opens the segment with `O_APPEND` and then seeks to its end, as CPython's append mode
    does. Darwin checks the limit against the descriptor's offset, not the end the write goes to,
    so a fresh descriptor at offset 0 would let an append run past the limit by up to the limit
    itself; Python's segment stops exactly at the limit, and so does Rust's (Slice 2d).
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
  non-UTF-8 input fails where Python's `surrogateescape` strings fail: `{"code": "VALIDATION",
  "message": "text is not valid UTF-8"}` with exit 1 where it would be written, `ISSUE_NOT_FOUND`
  where it names an issue, never as a parser error.

**g. Fault injection uses failpoints compiled into every build and activated by the environment.**
- **Syntax.** `BEAD_FAILPOINTS='push.after-ok=exit(99);backoff@always=skip'` is parsed once at start-up.
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

**h. The CLI is argparse, ported.**
- **Why not clap.** The plan chose clap, configured for argparse parity, as in jig, hushgit and
  sealtask. Phase 2b found that no configuration keeps argparse's behaviour (§5 row 13), which
  classes every argument as an option or a value before it assigns any, and takes positionals
  greedily up to the next option: `update A --status open B` leaves `B` unrecognised; `-d "- item"`
  and `-1x` are values; `--silent=yes` is "ignored explicit argument"; `-as open` is `-a -s open`;
  `--acc` is ambiguous between `--acceptance-criteria` and its alias. clap accepts or refuses each
  of these differently, so the port follows argparse's algorithm instead, and has no clap.
- **One table.** `cli/spec.rs` holds every command's arguments as `build_parser` declares them:
  option strings in order, destination, metavar, `nargs`, action (`store`, `store_true`,
  `append`, bead's `Once`), type and help text. The parser, the help pages and, from Phase 2c, the
  receipt signature (argparse's namespace, every destination with its default) are all read from
  it.
- **Parsing.** `cli/parse.rs` ports CPython 3.14's `_parse_known_args` for the actions the table
  uses, with the subparsers action: unambiguous prefixes, the last value of a repeated option,
  combined short flags and attached values, `--`, "given more than once" for `--parent`/`--epic`,
  `ready`'s mutually exclusive scope, and argparse's own error texts. Every error is `USAGE` with
  exit 2. Arguments stay the OS's bytes until a command reads them, as `surrogateescape` keeps
  them: bytes that are not UTF-8 match no key and fail only where they would be written, and an
  error that quotes them prints `?` for each, or `\udcXX` in JSON.
- **Help.** `cli/help.rs` ports `HelpFormatter` (the usage line and its wrapping, the sections,
  the help column) and `cli/wrap.rs` `textwrap.wrap`, at the width `shutil.get_terminal_size`
  reads (`COLUMNS`, else the terminal on standard output, else 80) less two. Never coloured.
- **Pre-parse.** `--json`/`--actor` extraction and `--` handling stay as in Python, before the
  parser runs.
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
  in-memory simulated remote of Phase 5 (§4.8). This follows pancake-studio-core's rule against
  inventing an abstraction before its implementation slice, and the general advice against a
  trait with one implementation.

**j. Command receipts remain Python-compatible.**
- Receipts belong to the `coordinated` commands only: `claim`, `takeover`, `release`, and `update`
  with `--claim` or `--status in_progress`. An owner's `close` and `delete` (and `update --status
  closed`) are local writes through `writing`: they name the claim they end, publish through
  `publish_soon`, and write no receipt.
- Preserve `sync.command_signature` exactly: the object has `actor`, `args` and `request`. `args`
  is the subcommand's whole argparse namespace: every destination with its default (`null`,
  `false`), including `command` (the subcommand's name), excluding `func` and callable values.
  `--json` and `--actor` never reach it, because they are taken out before parsing. `request` is
  `null` except for `update`, where it is `{"fields": …, "status": …}` after normalisation, with
  file and stdin contents resolved. `bead claim p-1` by `{"agent": "codex", "thread": "t"}` signs
  `{"actor": {"agent": "codex", "thread": "t"}, "args": {"command": "claim", "ids": ["p-1"],
  "offline": false, "remote": null}, "request": null}`. Clap's internal representation is
  translated into this shape, not serialised directly.
- The signature uses Python's `json.dumps(..., sort_keys=True, default=str)`: sorted keys,
  ASCII escapes, spaced separators, no trailing newline. Capture fixtures from Python for each
  coordinated verb and its defaults/aliases. Rust must read and write the same signature bytes
  so either binary can recognise the other's accepted request.
- Port the complete lifecycle in `writing`, `Link.merge`, `settle_commands`, `drop_pending` and
  `forget_receipts`. The receipt is durable before the pending attempt. Confirmation removes the
  pending JSON but may leave its accepted receipt; sync and read-only commands do not consume that
  receipt merely because `pending_ownership` is empty. A recognised retry does not reapply edits;
  it reports the current ownership state. Failed later commands and other actors preserve the
  receipt; retirement still follows Python's successful-command rules.
- No receipt migration or blanket deletion is part of this port. A later incompatible format
  needs a separate migration design. The shared-store upgrade/rollback cases in §4.2 are required
  before the ownership slice and cutover can pass.

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
- **Workspace binding.** A consumer compares the tree manifest's workspace to its approved
  expected workspace before using the projection; a well-formed but foreign manifest is refused.
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
fastrand = "2"
indexmap = "2"
libc = "0.2"
nix = { version = "0.31", default-features = false, features = ["user"] }  # ~NAME in BEAD_HOME
proptest = "1"
regex-lite = "0.1"                          # git message classifiers, transliterated from sync.py
rustix = { version = "1", features = ["fs", "process", "system", "termios"] }
sha2 = "0.10"                               # the version studio already builds
tempfile = "3"
thiserror = "2"
uuid = { version = "1", features = ["v4", "v7"] }

[workspace.lints.rust]
unsafe_code = "deny"                        # two documented exceptions, §2.2
missing_docs = "warn"
unreachable_pub = "warn"
rust_2018_idioms = { level = "warn", priority = -1 }
unknown_lints = { level = "deny", priority = -10 }              # -D warnings does not catch a stale
renamed_and_removed_lints = { level = "deny", priority = -10 }  # name in these tables; these do

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
allow-print-in-tests = true
```

This configuration was checked on a skeleton of all seven members with Rust 1.99.0 (review 9):
with the crate-level exemptions of §2.2, clippy (`-D warnings -D clippy::mod_module_files`),
nextest, doctests and cargo-deny pass with no warning, and so does rustfmt (run with 1.98.1's,
because this Mac's 1.99.0 has no rustfmt until the toolchain file installs it).

**rustfmt** uses the defaults (style edition 2024). **cargo-deny** 0.19.7 copies the `deny.toml`
of runledger (archived into batter, which has none): no advisory ignores; licences MIT,
Apache-2.0 (also WITH LLVM-exception), BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0 and Zlib;
duplicate versions warn; crates.io only.

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
- `CommandExt::pre_exec` with `setsid`, and `FileExt::read_at`, for the git runner;
- unsafe attributes (`#[unsafe(link_section)]`);
- `std::env::set_var` being `unsafe` in 2024, which is why libraries take explicit settings;
- resolver 3, which is MSRV-aware.

**Studio's toolchain.** Studio pins 1.97.1 with MSRV 1.94. When it links `beadroll-core`, its pin
moves to 1.99 in the same change (§7), and so does its image builder's `FROM rust:1.97.1` base,
which refuses a compiler that differs from the pin.

## 4. Tests

### 4.1 What there is

The suite has 473 tests in ten modules: 13,242 lines with `support.py` (458 and 12,938 when this
plan was written; Phase 1 added fifteen). They are almost all
black-box: about 1,890 call sites run `bead` as a subprocess, and in-process patching is rare
(`support.patched` is called 14 times in 11 tests, `mock.patch.object` 10 times); 20 tests run
through `runpy` or loader drivers.

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
| A | black-box, runs unchanged against any binary | 308: 14 in `test_hardening` need the two crash hooks, which Python has; 2 need Phase 0's harness fixes (§4.2); V1 t1 runs only git |
| C-env | black-box once Rust implements an environment-triggered hook | 20: 14 need only the crash hooks, so they pass against Python today (with the 14 in A, 28 tests rely on the hooks alone); 6 need new failpoints, are drivers today, and go with the failpoint tests |
| B | pure logic, becomes Rust unit tests | 76: 60 exercise only `beadroll-core`; 16 need git, the store or sync and land with Phase 2a (listed in §6, Phase 1) |
| C | needs an injection seam or an in-crate API | 34: 22 through failpoints, 12 through an in-crate API (C-api) |
| D | Python-specific; rewritten as the Rust property it stands for | 20: the 9 of §4.6, `HelpAndPrime.test_every_command_has_help`, and `test_check`'s 9 and `test_hygiene`'s 1, which become xtask tests |

Review 9 recounted every row from the test sources: the totals hold; the splits above are what
each count means.

### 4.2 How we prove that nothing regressed

1. **The Python suite is the oracle.** `cargo xtask oracle` runs the Python modules with
   `BEAD_UNDER_TEST=target/debug/bead`. It skips, from a checked-in list derived from the
   catalogues (`xtask/oracle-skip.txt`):
   - the driver tests, which `runpy` the launcher and cannot target a binary (20);
   - the in-process-only tests, which would only re-test Python (85);
   - the CLI tests whose verdict comes from patching in-process Python (19);
   - the tests that run no `bead` at all (11: `test_check`, `test_hygiene`, V1 t1).

   A test that does not honour `BEAD_UNDER_TEST` is fixed in Phase 0, not skipped. Two do not
   today: the real-export test in `test_compat` looks for `infra/compat_real.py` beside the
   binary, and that driver runs `./bead` itself; `test_prime_and_onboard…` in `test_commands`
   reads `AGENTS.md` beside the binary. Phase 0 makes the harness select the binary under test
   everywhere, in the test modules and in the `infra/` drivers they run, and changes no Python
   behaviour. The oracle also makes a bypass fail rather than pass against Python: while it
   targets another binary, it sets a variable under which Python bead refuses to start
   (`BEADROLL_ORACLE_REFUSALS`, naming the file where each refused start is recorded, so that the
   test fails even if it expected bead to fail). The check lives in `cli.main`, not only in
   `./bead`, because `V8_Python312` runs a copy of the launcher.

   338 tests run unchanged: 323 drive only the CLI, and 15 also inspect the store with Python's
   replay. 336 of them are in the fast modules, and 27 need Rust's crash-hook aliases. The
   real-export test joined them when Phase 0 fixed its helper, and Phase 1 added five black-box
   tests.
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
   - `cargo xtask inventory` maps every Python test id to a Rust test by one mechanical rule:
     `<module without test_>::<snake_case(Class)>::<method without test_>`, e.g.
     `test_hardening.G12_RetriedEndOfClaimIsPublished.test_g12_retried_close_…` becomes
     `hardening::g12_retried_end_of_claim_is_published::g12_retried_close_…`. Over all 473 tests
     the paths are unique and valid identifiers. A Rust test matches when its path holds the
     class and method segments in that order, so files may group a finding's classes or split a
     long class under the file budget (`hardening::g12::g12_retried_end_of_claim_is_published::…`).
     Finding ids sit in every hardening class name and in some method names elsewhere; keeping
     the class segment keeps `FINDINGS.md`'s references valid. `sweep::sweep` needs an `allow` of
     `clippy::module_inception` with its reason.
   - The inventory (`xtask/inventory.txt`) fails on any unmapped test. D-class replacements are
     listed explicitly, so `FINDINGS.md` keeps pointing at real tests. Hardening's catalogue gives
     some classes one row for several tests ("t1", "t2" in prose); seeding the inventory splits
     them per method.
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
     The Python oracle explicitly uses `PYTHONINTMAXSTRDIGITS=4300` and verifies
     `sys.get_int_max_str_digits() == 4300`. Include positive and negative integers at 4,300 and
     4,301 digits in parser, writer, import and full journal replay cases. The larger token must
     not let Rust publish an event that the default Python reader would skip.
   - These run in CI with a bounded seed count until the cutover, and nightly at volume.
   - Any difference is either fixed in Rust or recorded as a decision in §5.
4. **Mixed fleets converge.** `Machine` takes a per-machine binary. The mixed-fleet suite runs the
   protocol scenarios and the first stress model with Python and Rust machines on one remote. It
   requires identical exports, `show` and `claims` everywhere, and no integrity rows. That is the
   rollout, rehearsed. Also switch binaries on the **same home, replica and actor**: Python → Rust
   and Rust → Python. Leave an accepted lost-ack `update --claim` receipt, let another machine
   edit the issue, sync, and verify that `pending_ownership=[]` still leaves a usable `.command`.
   Retrying under the other binary must recognise the request without overwriting the newer edit
   or duplicating its events. Cover changed file/stdin requests, failed later commands, actor
   isolation, moved ownership and crashes before/after confirmation; verify receipt bytes in
   both directions. These are upgrade/rollback tests in addition to separate-machine convergence.
5. **Golden outputs, for what tests do not fully pin.** Every help page whole (the port follows
   argparse's layout, §2.3h), the whole human `show` and the other reading commands' human
   output over one scripted tracker, and `info`'s keys (`crates/beadroll/tests/golden/`).
   - `cargo xtask golden` captures them from the Python binary over scripted scenarios, with ids,
     times and paths normalised: it runs the golden tests (`tests/cli/golden.rs`) in capture
     mode against `./bead`, and they refuse to capture from any other binary; run normally, the
     same tests compare the `bead` under test with the files. They are CPython 3.14's, the
     reference interpreter (§5): against an older Python's launcher, argparse's help layout
     differs.
   - Goldens are generated from Python only, never from Rust. Regenerating a golden to make Rust
     pass is exactly what the house rules forbid.

### 4.3 Where each Python module goes

| Python | Tests | Rust | How |
|---|---|---|---|
| `test_replay` | 69 | `crates/beadroll-core/tests/core/main.rs` (+ `tests/core/replay.rs`, `tests/core/replay/*.rs`) | Ported in Phase 1: 64 in-process against the core API, the nine `PortAlignment` tests among them (`Journal::add` takes any `Value` and records problems); `find_cycles` generic over an edge function for the four-node oracle; 2 CLI tests move to `beadroll`; 3 B tests need git or the store (`normalize_remote`, `Workspace.ref`, two workspaces on disk) and land with Phase 2a |
| `test_storage` | 9 | `crates/beadroll/tests/cli/storage.rs` | 5 black-box; 4 via failpoint handshakes (`pause-until`) |
| `test_compat` | 4 | a core test (comment-id export) and a store test (ref validation, which runs `git check-ref-format`) + `cli/compat.rs` (2) | The real-export/`br`/`bv` test is `#[ignore = "needs br, bv and real Beads exports"]`, also run by `xtask check --real` |
| `test_commands` | 113 | `crates/beadroll/tests/cli/commands/*.rs` | 97 black-box; 8 on the crash hooks; 2 on the `clock` failpoint; 4 unit tests of the store, sync and replay (2 `detect_actor`, `Link`'s environment, a replay of a CLI-written store), none core-only; 1 black-box with a fake `ps` (`nearest_harness`); 1 rewritten |
| `test_protocol` | 57 | `cli/protocol/*.rs` + `crates/beadroll-sync/tests/sync/protocol/` | 35 black-box; 6 on the crash hooks, which Python has; 4 on new failpoints (`append.mid`, `clock`, `atomic-write:claims/`, `backoff`); 11 in-crate API tests (fence wins, `Link` outcomes, per-`Link` timeout), ported in Slice 2c; 1 core |
| `test_hardening` | 208 | `cli/hardening/{g01…g65,v1…v9}.rs` + unit tests in git/store/core | 172 black-box (14 on the crash hooks); 15 failpoint/API; 12 unit tests, most of them in the git and store crates (the `without_password` pairs, `normalize_remote`, URL schemes, runner timeouts, `Workspace.remote`; the catalogue lists them); 9 rewritten (§4.6) |
| `test_stress` | 2 | `cli/stress.rs`, `#[ignore = "stress: randomised models"]` | Same seeds and oracles; schedules run in parallel |
| `test_sweep` | 1 | `cli/sweep.rs` | Failpoints replace the `runpy` wrapper; the 12-schedule slice runs by default, `BEAD_SWEEP=full` runs all 660; schedules in parallel |
| `test_check` | 9 | `xtask` tests (`check::check_script`), the sweep's own (`sweep::sweep_failure_reporting`) and `infra/test_roundtrip.py` | The verification tooling must fail when a selected check fails; the sweep's reporting tests sit beside the sweep, whose runner is test code xtask cannot reach |
| `test_hygiene` | 1 | `xtask` test | Same patterns over `git ls-files` |

**Test binaries.** Each crate has one integration-test binary, rooted at `tests/<name>/main.rs`:
`tests/cli/main.rs` declares `mod commands;`, which is `tests/cli/commands.rs`, whose modules are
`tests/cli/commands/*.rs`, with no `mod.rs` files. A root at `tests/cli.rs` would look for
`tests/commands.rs` instead (E0583 on 1.99.0), and Cargo would build that file as a test binary
of its own. One binary per crate keeps link time down, which pancake-studio-beads does the same
way. Slow tests use `#[ignore = "reason"]`, the house convention.

### 4.4 `beadroll-test-support`

A direct port of `support.py`:

- `Sandbox`: a temp root, a bare `origin.git` and a seeded `main`.
- `Machine`: its own home, clone and identity.
  - `run`, `bead` (with `--json`), `show`, `ready`, `export`, `store`.
  - The environment is stripped of `CLAUDE*`, `CODEX*`, `BEAD_*` and `GIT_*` before
    `BEAD_HOME`/`BEAD_AGENT`/`BEAD_THREAD` are set.
- Explicit binary paths: the consuming `crates/beadroll/tests/cli/main.rs` evaluates
  `env!("CARGO_BIN_EXE_bead")` and passes that default into test support. The shared dependency
  must not expand the macro itself; Cargo does not define it there. Tests in other crates, which
  Cargo gives no `bead`, supply the checkout's Python launcher until the cutover removes it
  (`crates/beadroll-sync/tests/sync/`). Selection is per-machine override, then
  `BEAD_UNDER_TEST`, then the supplied default. Phase 0 can supply Python's absolute `./bead` path
  before Rust works.
- Fixtures:
  - writers for pre-receive hooks (stall, gate, refuse, slow post-receive);
  - fake `ssh`, `ps` and `git` shims;
  - an HTTP git server driver for the transport tests;
  - a PTY helper;
  - a `ulimit` runner.
- Failpoint helpers: environment builders, and the `ready`/`go` file handshake.
- `json!`-style event builders for core tests. They serve core's integration tests
  (`crates/beadroll-core/tests/core/`) only: in core's own unit tests the dev-dependency cycle
  builds a second `beadroll_core`, and its types do not match (E0308 on 1.99.0).

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
| `fail-reply` | Report a push that succeeded as failed: the lost acknowledgement. It drops git's porcelain status line too, because a status flag counts as acceptance whatever the exit code |
| `error(CODE)` | Raise that error code |
| `pause-until(PATH)` | Touch `PATH.ready`, then wait for `PATH.go` |
| `sleep(MS)` | Sleep for MS milliseconds |
| `skip` | Skip the step |
| `offset(±DUR)` / `at(TIME)` | Shift or set the clock |

Specs are `point=action` (first hit), `point@nth(k)=action` (one hit, `k >= 1`), or
`point@always=action` (every hit in that process). Hit counters are per point in the invocation's
registry. Invalid selectors and duplicate specs for one point are `CONFIG` errors.
`backoff@always=skip` removes every retry wait; `backoff=skip` removes only the first. The registry
tests cover first/nth/always dispatch and the exhaustion tests still assert exactly 12 rounds.

As Slice 2c built it (`crates/beadroll-store/src/failpoint.rs`):
- **Which point takes which action.** Every point but `clock` can `exit`, `sigkill`, `sleep` and
  `pause-until`. `torn-sigkill` belongs to `append.mid`, `errno` to `atomic-write:`, `fail-reply`
  to `push.reply`, `skip` to `backoff`, and `offset`/`at` to `clock`. `error(CODE)` fails
  `publish.result`, and fails the command at `ownership.after-confirm`, which leaves its attempt
  confirmed and its pending twin in place (Q03's interruption, in process). Any other pairing,
  an unknown point or a code bead does not have is `CONFIG`.
- **Arguments.** Durations take `ns`, `us`, `ms`, `s`, `m`, `h` or `d` with an optional sign;
  `at` takes nanoseconds since the epoch; `errno` names one of `ENOSPC`, `EFBIG`, `EIO`, `EACCES`,
  `EPERM`, `EROFS`, `EDQUOT`, `EMFILE`, `ENFILE`, `ENOENT` and `EINTR`; `BEAD_FAILPOINTS` must be
  UTF-8. A `pause-until` waits at most 300 seconds for its `.go` file.
- **In-crate tests** build `Settings` with `BEAD_FAILPOINTS` among its variables and read
  `Failpoints::hits` to check that the fault they injected was reached.

| Point | Where | Replaces |
|---|---|---|
| `ownership.before-publish` / `ownership.after-publish-ok` | `coordinated`, around the attempt's publish | `BEAD_TEST_CRASH_BEFORE_PUSH` (98) / `AFTER_PUSH` (99), which stay as aliases |
| `push.before` / `push.after-ok` | Every push, including `publish_soon`, sync and fences | The sweep's `run_git` wrapper; G12 ("crash before *any* push") |
| `push.reply` | After git push returns | `run_git` patched to report a failure after success (lost-ack tests) |
| `publish.result` | `Link::publish` | `mock.patch.object(Link, "publish", side_effect=…)` |
| `ownership.after-confirm` | After `confirm_ownership`, before pending cleanup | The sweep's kill-confirm; q03; G54; `V2_ConfirmedTwin` |
| `atomic-write:<path-part>` | `write_atomic`, matched on the target path, after the temporary file is written and fsynced and before the rename; `atomic-written:<path-part>` fires after the rename | kill-write-pending/-confirm; `V5_KillAtRename`; `Protocol3`'s kill during confirmation; ENOSPC on `meta.json` (G29); the reader-sees-all-or-none handshake, which pauses at both |
| `append.mid` | Inside a segment append, after a partial line is written and flushed (Python's driver writes 100 bytes) | The kill-mid-append driver |
| `backoff` | The unlocked wait between rounds | `patched("pause", …)`; G11 t2; V2's "unlocked sleep"; retry-exhaustion speed |
| `claim.before-push` | Between fetch and push of a claim | G09's competing publish |
| `pending.after-list` | Between listing and reading `pending/` | G28 t2 |
| `journal.after-enumerate`, `view.before-load-pending`, `identity.before-link`, `publish.before` | Reader/writer races | test_storage drivers |
| `clock` | The UUIDv7 clock and torn/fork stamps | Drivers patching `time.time_ns` (f14 ≈ 2110, −48 h, −1 day); G47 t3: two torn fragments within one second stay apart, their names in nanoseconds (equal stamps would share one, in Python as in Rust) |

Two seams need no failpoint. The process-tree walk of `detect_actor` is driven by a fake `ps` on
`PATH` (§2.2), against either binary. `Protocol3`'s fence test needs a per-call network timeout:
an in-crate `Link` test, or black-box with `BEAD_NET_TIMEOUT=0.25` and `backoff=exit(N)`.

The tests that run Rust's in-crate APIs instead of the binary take the same registry through
`Settings`, not the environment.

### 4.6 Python-specific tests, and what each becomes

- **G13:** `.beadroll.json` is UTF-8 whatever the locale. It stays black-box across ISO-8859-1,
  US-ASCII and C, without the `python3` probe.
- **G30 t4:** after one machine tries to import a record 12,000 levels deep (refused: events nest at
  most 100 levels and imported records 98), a peer whose reader stops lower keeps working and sees
  what the first sees. It becomes a mixed-fleet case: Python 3.14 against Rust, whose reading
  ceilings differ (§5 row 7). G30 t1 keeps its own property: a line a million levels deep is one
  integrity problem, never a stack overflow.
- **G52 t5:** `chmod 0` on the store gives `IO`. The test runs unconditionally.
- **G59:** help contains no ESC byte on a PTY, with `FORCE_COLOR=1`, or under `--json`, and it
  says `usage: bead create`.
- **V8_Python312:** becomes the CI matrix: Linux and macOS, and the oldest supported git (2.40)
  beside the newest. musl waits for public binaries, which §10 defers.
- **`HelpAndPrime.test_every_command_has_help`:** a Rust unit test iterates the command table
  (`cli/spec.rs`), and the integration test runs `--help` and `help NAME` for each of the 27
  names.
- **`test_check` and `test_hygiene`:** xtask tests (§4.3). The three `RoundtripFailureReporting`
  tests check `infra/roundtrip.py`, which stays Python (§10, decision 6): they move to `infra/`
  beside it, and `xtask check` runs them, as `./check.sh` does until the cutover.
  `SweepFailureReporting` checks the sweep's own runner, so its port sits beside the sweep.
- **Checks kept in other classes:**
  - G48 (SIGXFSZ) keeps its property, using the code in §2.3f.
  - The "no Traceback" and "Exception ignored" checks become the panic and signal checks of §4.4.

### 4.7 Speed

**Targets** (measured and recorded as evidence by `cargo xtask timing`; as jig's CI uploads its
test timings, timings are evidence and never gate a test):

| Suite | Rust target | Python now |
|---|---|---|
| Fast suite, this Mac (16 cores) | ≤ 2 min | ≈ 20 min |
| Fast suite, a 4-core CI runner | ≤ 6 min | |
| Stress | ≤ 10 min | 7–17 min |
| Full sweep | ≤ 20 min | ≈ 1.7 h |

**How.**
- **nextest**, process-per-test, as in jig: `nextest-version` in `.config/nextest.toml` is the
  minimum, and CI installs the exact version. Process-per-test also isolates environments.
- **The thread count is measured, not assumed.** jig caps it at 4 because git- and process-heavy
  tests fail spuriously at full core count; the 16-core target above is confirmed or revised in
  Phase 3.
- **Test groups** that limit concurrency for the git-maintenance and descriptor-limit tests.
- **`slow-timeout` with `terminate-after`**, so that hangs fail, not stall (jig's sets only the
  warning period).
- **`retries = 0`**, nextest's default, written down. A flaky protocol test is a bug.
- **Cheap `bead` calls:**
  - start-up is about 1 ms instead of 60;
  - replay is in-process and fast;
  - `opt-level = 2` for core in test builds.
- **No real waits for synchronisation.** Interleavings that Python got from real backoff sleeps
  use `pause-until` handshakes, which are deterministic and instant. Retry-heavy tests use
  `backoff@always=skip`. Timeout tests keep real but short timeouts: `BEAD_NET_TIMEOUT` of 0.25–1 s, with
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
- **Fuzzing.** proptest, already used in hushgit and runledger, drives the differential and
  property tests:
  - replay is invariant under delivery order;
  - JSON round-trips in every dialect.

  `cargo-fuzz` (installed here) covers the JSON parser, the segment reader and the Beads importer
  nightly with a time budget. Its corpus is seeded from real exports, which are not checked in.

### 4.9 CI

The workflow copies the shape of jig-sh's `.github/workflows/rust-tests.yml`, the one sibling
workflow that has all of it, with that file's current SHA pins:
- `contents: read`, concurrency groups, `merge_group`, and actions pinned by SHA;
- `actions-rust-lang/setup-rust-toolchain` with `cache: false`, then a separate
  `Swatinem/rust-cache` step;
- nextest installed with `taiki-e/install-action` at the exact version.

Each job joins CI with the phase that gives it something to run: fmt, clippy, test, deny and the
oracle in Phase 0, diff in Phase 1, mixed-fleet in Phase 2a, and the nightly suites as they land.
The oracle runs twice: against Python's `./bead`, where it must pass completely (the harness
checking itself), and against the built binary, where it reports the pass count per module and
gates only from the close of Phase 2d.

| Job | When | What |
|---|---|---|
| fmt | every push and PR | `cargo fmt --all -- --check` |
| clippy | every push and PR | `--workspace --all-targets --all-features --locked -- -D warnings -D clippy::mod_module_files` |
| test | every push and PR | nextest on ubuntu-latest and macos-latest; `cargo test --doc` |
| deny | every push and PR | cargo-deny |
| oracle | every push and PR, until the cutover | Python 3.14 + `xtask oracle` against `./bead` and against the built binary |
| diff | every push and PR, until the cutover | bounded `xtask diff replay` and `xtask diff json` |
| mixed-fleet | every push and PR, until the cutover | the mixed Python/Rust scenarios |
| nightly | schedule or dispatch | stress; full sweep; volume differential runs; cargo-fuzz smoke; git-version matrix (2.40 and latest) |

`./check.sh` survives as a thin wrapper around `cargo xtask check [--stress] [--real]`, because
`AGENTS.md`, `CONTRIBUTING.md` and `REVIEWING.md` all point at it.

## 5. Python semantics the port keeps, or decides not to

These are the places where "the same as Python" means CPython's semantics, not Rust's defaults.
Each is implemented in `beadroll-core::text` or `json` and covered by the differential tests. Rows
3 and 6 were decided on 2026-10-09: reject. Row 7's ceiling and rows 22 to 26 came from review 9
and were decided in Phase 1, on 2026-10-09 (§10); rows 27 to 30, which Phase 1's differential
testing found, were decided with them. Every row decided "reject" is applied to Python too, with a
test and a `FINDINGS.md` row (U01–U11), so `cargo xtask diff` needs no list of expected
differences; the generators cover every row. Every row that departs from Python goes into
`REVIEWING.md` §2 when the Rust implementation lands. The reference interpreter is CPython 3.14, the one CI runs; a
difference that comes only from another Python version's texts or Unicode tables is recorded, not
chased.

| # | Python behaviour | Where it shows | Rust | |
|---|---|---|---|---|
| 1 | `isinstance(True, int)`: replay accepts booleans as `priority`, `estimated_minutes` and `generation`, and `"v": true` equals 1 | Replay of histories that predate `write_problem` | `PyInt::Bool`; writes still refuse booleans | keep |
| 2 | Arbitrary-precision integers; default decimal parsing/formatting limit is 4,300 digits in Python 3.11+ | Extras, imports, generations | Integers beyond `i64` kept as digit strings, with the same decimal limit on reading and writing; oracle pinned to that default | keep default contract |
| 3 | Floats print as `repr()`; `1e400` parses to infinity, and export then raises `ValueError` (`allow_nan=False`) | Extras from imports | `repr`-compatible formatting; non-finite is "not a JSON number" | reject (decided) |
| 4 | Lone surrogates, escaped or raw `ED A0 80`, read as one U+FFFD each, with an integrity row | Journal and attempt lines | In the parser | keep |
| 5 | A duplicate key keeps its last value at the first key's position | All JSON | Same | keep |
| 6 | `UUID_RE.match` with `$` accepts a trailing newline in ids and deps | Hostile events only | Reject: ids and deps must match the UUID shape exactly | reject (decided) |
| 7 | Readable nesting depends on the interpreter and its C stack: 994 levels on 3.11, about 10,000 on 3.12 and 3.13, 116,199 on 3.14 on this Mac; replay has no depth check, so a readable deep event applies. Writes and imports are capped at 100 and 98 | Parsing | Iterative parser with a ceiling of 512 levels; deeper is an unreadable line; caps kept. Python counts a line deeper than 512 as unreadable too, checked before `json.loads` (U03) | decided: 512 in both (§10) |
| 8 | `str.strip()`/`isspace()` use Python's Unicode set (U+001C–001F count as space); `bytes.strip()` uses ASCII | `split_list`, comment emptiness, blank JSONL lines | `text::py_strip` and friends | keep |
| 9 | `str.lower()` is full Unicode case mapping | `search`, `--title-contains`; `normalize_remote`, hence derived workspace ids; `init`'s default prefix | `to_lowercase`, confirmed by `diff` | keep |
| 10 | `os.path.realpath` is lenient and pure Python, with no case folding | Remote resolution and comparison | Port the algorithm; never libc `realpath` | keep |
| 11 | `repr()` of values in `show`'s conflict lines: quote choice, escapes, printability | Human output | Port of the `repr` rules; printability table generated from CPython | keep |
| 12 | `int()` accepts `" +5 "`, `"5_000"` and non-ASCII digits | `--estimate`, `--limit` | `text::py_int` | keep |
| 13 | argparse: prefix abbreviations, last value wins, its own error texts and help layout | CLI | argparse's parser, help formatter and `textwrap` ported (§2.3h): behaviour and texts kept | keep behaviour |
| 14 | `str(True)` is `True` in human lines (`[PTrue]`) | `list`, `ready`, `show` | `PyInt: Display` | keep |
| 15 | `OSError` reads `"{strerror}: {filename}"`, and a rename names its target | IO errors | Paths attached at every call; libc `strerror` text | keep |
| 16 | `json.dumps` dialects | Everywhere | `json::Format` | keep |
| 17 | CPython ignores SIGXFSZ; children get the default | File-size limits | §2.3f | keep |
| 18 | Closed fds at exec give `None` streams; `-` with no stdin is an error | stdio | §2.3f | keep |
| 19 | 12 rounds; `U(0.1, 0.4) × attempt` | sync and ownership | Same | keep |
| 20 | Names: `refs/beads/fetch/<pid>-<hex8>`, `.NAME.PID.tmp`, `*.torn-<ns>`, `*.fork-<ns>`, `sync.git.damaged-<ns>` | Store, `sync.git` | Same | keep |
| 21 | `command_signature` serialises actor, normalised argparse args and resolved request; accepted receipts survive sync | `pending/*.command` | Preserve Python signature bytes and lifecycle, including same-store upgrade and rollback (§2.3j); do not delete receipts to migrate | keep |
| 22 | JSON read from bytes: `json.loads` detects a UTF-8 BOM, UTF-16 and UTF-32 from the leading bytes, then decodes with `surrogatepass`; read from text, a BOM is an error. A journal line in UTF-16 is a valid event today | Journal and attempt lines, the torn-tail check, the manifest | Python decodes the bytes as UTF-8 (`surrogatepass`) before parsing, as Rust does (U04) | reject (decided) |
| 23 | Two readers use plain `json.loads`, not `loads_strict`: `NaN` and `Infinity` parse there, and the manifest's `format` is compared with `==`, so `true` and `1.0` pass as format 1 | `complete_prefix` (torn tails, also of fetched segments), `Link.fetch`'s manifest | Both readers are strict in Python, and `format` must be the integer 1 (U05) | reject (decided) |
| 24 | `re` on `str` is Unicode-aware: `\d` matches every decimal digit (Nd), and `int()` reads it; `(?i)[a-z]` also matches U+0130, U+0131, U+017F and U+212A | `free_key` (a replay rename: `x.٣` gives way as `x.3`), `mint_key`, the merge's segment-name filter; `without_password` and `normalize_remote`, so `httpſ://h/x` derives the same workspace id as `https://h/x` | ASCII digits and ASCII letters only, in both: `[0-9]` and `(?ai)` in Python (U06) | reject (decided, against the recommendation to keep) |
| 25 | Replay's last-resort `except` stores the exception's `repr` as the integrity text, and that text differs between Python versions. Reachable: `dep_add` with a truthy `meta` that is not an object (`1`, `"x"`, `[1]`), `dep_remove` with a list or object `key` beside `to`, `free_key` on a numeric suffix over 4,300 digits, or of 4,300 nines (counting on needs one more digit than Python prints) | `integrity` in `info` and the differential report | `payload_problem` refuses each in Python, a key's number at 4,300 digits or more included, so both report the same validation text and Rust has no last resort (U07) | reject (decided) |
| 26 | An uncaught exception prints a traceback with exit 1: an unparsable `BEAD_NET_TIMEOUT`, `BEAD_ROTATE_BYTES` or `BEAD_REPACK_*` (at start-up, even for `--version`), a `*.jsonl` name that is not a number in this machine's own journal, `published` counts in `meta.json` that are not numbers, `mint_key` over the digit limit | Hostile settings and store files | A structured error with exit 1, never a panic: `CONFIG` for settings, `IO` for store files, `VALIDATION` for keys; core's `mint_key` answers `MintError::NumberTooLong` | depart (decided: Rust only); recorded as each lands |
| 27 | A create's comments are imported ones, kept as `{"uid": None, **comment}`: one naming a `uid` or a `where` made `detail` raise (a `where` that is not an object) or `export_records` raise (a `uid` that is not a string), or exported as a comment made here | `show`, `export` | Refused: a create's comment carries neither (U08) | reject (decided) |
| 28 | A create's fields may clear `created_at` (an import of a record whose `created_at` is null): `select` then compares `None` with strings, and `list` and `ready` raise | `list`, `ready` | A create cannot clear `created_at`, as it cannot clear a title, so such an import fails whole (U09) | reject (decided) |
| 29 | The stand-in for an id with several contents names the issue of the copy placed first; with no valid copy that can be a list or an object, and replay raises hashing it, so every command does | Every command on such a tracker | The stand-in names its issue only by a string (U10) | reject (decided) |
| 30 | An import reads its file as text, so an escaped lone surrogate survives into the record: the import fails ("text is not valid UTF-8") only where it reaches an event, and passes where the import drops it | `import` | An import refuses a lone surrogate anywhere in a record, as it refused a raw one (U11) | reject (decided) |

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
  - `beadroll-test-support`, with caller-supplied binary paths (§4.4);
  - xtask with `oracle`, `inventory` and `timing`;
  - the oracle skip list;
  - the test inventory seeded from the catalogues;
  - executable selection in the Python harness (§4.2): the test modules and the `infra/` drivers
    they run (`compat_real.py`, `roundtrip.py`) honour `BEAD_UNDER_TEST`, with no change in
    behaviour; `target/` in `.gitignore`;
  - a committed `Cargo.lock` (CI runs `--locked`), and `.cargo/config.toml` with
    `[alias] xtask = "run --package xtask --"`, without which `cargo xtask` does not exist; at
    least one test lands with the skeleton, because nextest fails a run that finds none.
- **Exit:** CI is green on the skeleton; the first ported tests pass against Python's `./bead`;
  `xtask oracle` runs the Python suite against any binary, passes completely against `./bead`,
  and fails a test that bypasses the selected binary.

**Phase 1 — Core (L)**
- **Deliverables:**
  - `json` and `text` (§2.3a, §5), including the default 4,300-digit decimal boundary on reads
    and writes and its parser/writer/import/replay fixtures;
  - ids and times; event validation;
  - the journal; causal order; replay; state;
  - queries: readiness, blockers, descendants, Tarjan + Johnson cycles;
  - views; Beads import planning and export (comment ids from 2⁴⁷); key minting;
  - published trees;
  - the two rejections of §5 (rows 3 and 6) applied to the Python implementation too: ids and
    deps matched in full against `UUID_RE`, and `loads_strict` refusing non-finite numbers, each
    with a test and a `FINDINGS.md` row. A mixed fleet then agrees even on hostile input, and the
    differential harness needs no list of expected differences;
  - decisions on §5 row 7's ceiling and rows 22 to 26, recorded in §10; every row decided
    "reject" is applied to the Python implementation the same way, with a test and a
    `FINDINGS.md` row.
- **Exit:**
  - the 60 B-class tests that exercise only `beadroll-core` are ported and green (with the nine
    `PortAlignment` tests Phase 1 added and the replay half of f09, 70; done). The other 16
    need git, the store or sync and land with Phase 2a: `test_replay`'s
    `test_one_remote_has_one_identity…`, `test_the_coordination_ref_cannot_be_a_branch` and
    `test_reused_event_ids_are_isolated_across_workspaces`; `test_compat`'s ref validation;
    `test_commands`' two `detect_actor` tests, `test_replay_does_not_depend_on_delivery_order`
    and the first `SyncRepoMaintenance` test; `test_hardening`'s G25 t3, G32, G57 t2,
    `V1_RunGitTimeoutHygiene` and the four V7 tests;
  - `xtask diff replay`/`json` are clean on every Python fixture and on at least 100k generated
    cases, including the classes of §5 row 7 and rows 22 to 26, which are decided and recorded;
  - replay of 10k events (parsed journal to `State`, release build) is measured and recorded by
    `xtask timing` against its Appendix A estimate. Phase 4 owns that budget; a timing is
    evidence, not a gate (§4.7).

**Phase 2 — Engine and CLI, in vertical slices (L).** Each slice raises the oracle's pass count,
which is the progress measure.

| Slice | Scope |
|---|---|
| 2a | The git runner with actual session detachment (§2.3f); store layout, files, lock, identity, workspace discovery; `Tx` and `writing`, including `forget_receipts`; `Link` whole: fetch, merge (which confirms pending attempts it finds on the remote), tree, publish with its four outcomes, maintenance and rebuild; `synchronise` (which sweeps, fences and drops pending attempts and adopts a published tracker) and `publish_soon`; `init`, `create`, `show`, `list`, `ready`, `sync`; JSON and human rendering; the output module; exit codes; the 16 B-class tests that need git, the store or sync (Phase 1 lists them) |
| 2b | Every other non-ownership verb, `import`/`export`, `info`/`log`/`machines`/`claims`, `help`/`prime`/`onboard`, unpublished counts; `xtask golden` and the goldens of §4.2 |
| 2c | The coordinated commands (`claim`, `takeover`, `release`, `update --claim` or `--status in_progress`): `coordinated`, `settle_commands`, `settle_pending` and Python-compatible receipts (§2.3j); an owner's `close`/`delete` ending its claim (`data.claim`, the `superseded` answer); the failpoint registry including repeat selectors |
| 2d | The hardening surface: URLs and credentials, transport config, damaged `sync.git`, refusal classification, stdio, signals, locales, PTY, modes, ENOSPC/EFBIG |

- **Exit:** `xtask oracle` is 100% green on Linux and macOS; the controlling-terminal and timeout
  properties pass, as do mixed-fleet smoke and same-store receipt upgrade/rollback cases (§4.2).

**Phase 3 — Test port complete (L; runs from Phase 0)**
- **Deliverables:**
  - every module ported (§4.3);
  - failpoint-based C tests;
  - D-class replacements;
  - stress and sweep, slice and full;
  - the mixed-fleet suite, including same-store receipt upgrade/rollback (§4.2);
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
    both binaries. Never a write: the copy still routes to the real remote. Outputs must be equal
    apart from `info`'s `version` (0.1.0 against 0.2.0) and `claims`' `idle_minutes`, which
    depends on the minute each binary runs in.
  - explicit executable selection in `infra/drive.py` (the other drivers gain it in Phase 0) and
    version/path preflight in the retained infrastructure drivers (§8), then `infra/drive.py` and
    `soak.py` with the Rust binary on the four machines and the throwaway repository; selecting
    Rust must work before the Python launcher is removed;
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

beadroll keeps its published tracker on a coordination ref, normally `refs/beads/journal`.
Moving studio to that source requires a persisted source migration as well as a reader adapter.
The concrete surfaces are studio's `crates/pancake-studio-core/src/source.rs`,
`migrations/20260909080058_project_identity.sql` (the existing constraint; change it through a new
migration), `crates/pancake-studio/src/sources/{reader,worker,policy}.rs`, and
`crates/pancake-studio-db/src/sources.rs` (source revisions and baseline resets).

What studio's schema and policy already fix (checked at studio 31cfafa):
- **One source per project.** `studio_project_sources.project_id` is the primary key, and refresh
  cycles, publications and approval receipts each record one repository, ref and commit.
- **Approval is per host.** `authorize_remote` admits any repository path on an operator-enabled
  endpoint, and some credential scopes cover a whole host.
- **The revision rule is also a trigger.** `studio_source_revision()` enforces generation and
  access revision in the database, besides `sources.rs`.
- **The reader is branch-shaped.** `CanonicalRef`, `clone --branch` and discovery are branch-only;
  a refresh reads one ref and one blob, and its unchanged check compares one tip.
- **Limits are stricter than bead's.** 10,000 issues, JSON depth 64, titles of 500 characters,
  16 MiB of input; bead nests to 100 and has no title cap. A tracker beyond them is rejected
  whole, by studio's all-or-nothing rule.
- **Publications admit one input profile**, `beads-rust-jsonl-v1`, and store the input bytes.
- `pancake-studio-core/AGENTS.md` states that persisted source refs are `refs/heads/...`.

1. **Bridge.**
   - Retain the approved code repository and branch as the configuration source. Discover
     `.beadroll.json` there and keep that binding separate from the tracker source, so later
     configuration changes can still be discovered. With one source row per project, the
     tracker binding needs its own row or columns with its own access revision, and provenance
     records both the configuration commit and the journal commit. Decide whether the tracker ref
     lives in `canonical_ref`; only then does its `refs/heads/*` constraint change.
   - Persist the tracker binding: approved coordination endpoint, ref, expected workspace and
     credential binding. Read both `remote` and `ref` from the tracked configuration; absent
     values default to the approved code repository and `refs/beads/journal`. Validate the
     expected workspace using the tracked `workspace` or bead's origin-derived identity rules
     and require the published manifest to match it.
   - Because approval is per host, the tracker endpoint, ref and expected workspace are set only
     by an operator's source command, never from `.beadroll.json`. Tracked configuration is a
     proposed binding: a refresh whose `.beadroll.json` names a different endpoint, ref or
     workspace fails and changes nothing. Unsupported remote names and local-path forms fail
     explicitly; do not resolve them using the worker's git configuration or filesystem.
   - Extend the `studio_source_revision()` trigger along with `sources.rs`, and update the
     `refs/heads/...` invariant in `pancake-studio-core/AGENTS.md`. A refresh reads two refs and a
     whole tree within the existing byte budget, and its unchanged check compares both tips.
   - Extend `CanonicalRef` and migrate the database's independent `refs/heads/*` constraint to
     admit valid bead coordination refs. Replace the branch-only `clone --branch` assumption
     with an exact approved-ref fetch path, retaining transport policy and object-format support.
   - Transition existing publications through studio's source-selection transaction: advance
     source generation, clear the code-branch ancestry baseline and obsolete trust/admission
     state, and retain historical publication content. Apply the same lifecycle to later
     endpoint, ref or expected-workspace changes, extending the current repository/ref content
     comparison to include expected workspace; credential changes use the existing access
     revision mechanism. A source change must not silently redirect an in-flight refresh.
   - Every journal commit's tree holds the complete journals, so depth one suffices for replay.
     After the first publication establishes the journal baseline, ancestry verification still
     uses the bounded deepening loop. An incomplete shallow history is not proof of divergence.
   - Pass the tree's entries to `beadroll_core::published::Tree`, replay it, and feed
     `state.export_beads()` to the existing `pancake-studio-beads` parser. The normalised graph
     and member-output contracts stay the same; its br-0.5.7 fixtures describe that export format.
   - Acceptance covers migrating an existing code-branch publication and two successive journal
     refreshes, including ancestry that needs deepening; code and coordination in distinct
     repositories; rejection of unapproved redirection, unsupported remote forms and workspace
     mismatch; later binding changes with obsolete refresh work rejected; and a tracker beyond
     studio's limits, rejected whole.
2. **Native.** Studio maps `State` directly to its normalised graph and drops the JSONL round trip.
   That needs a second publication `input_profile` and its migration. It keeps its own rules:
   - limits;
   - all-or-nothing validation;
   - privacy (design, notes, comments, labels and assignee never reach member output);
   - canonical agreement and plan digests.

   beadroll provides typed, exact text. The digests stay studio's.
3. **Build.** Studio pins `beadroll-core` to one revision, as a git dependency the way it takes
   `batter` or as a vendored copy (decision 7), and moves its toolchain pin (and its image
   builder) from 1.97.1 to 1.99. `sha2` 0.10,
   `thiserror` 2 and `indexmap` 2 are already in its graph. This repository is private, so every
   environment that builds studio needs read access to it: local machines, its CI runners and the
   container image builder. Studio fetches its git dependencies anonymously over HTTPS today, and
   its image build carries no secret by rule (`infra/images/Dockerfile`: "no build argument, secret
   mount or layer carries a secret"). How studio's builds reach a private `beadroll-core` is open
   (§10).

For the execution-runs branch: agents in devbox worktrees run `bead` instead of `br`. bead is built
for worktrees, and every write reaches the remote at once. That branch is not merged: its runner
diffs the worktree's `.beads/issues.jsonl` against the base commit's export every 30 seconds, and
with bead neither file exists. The runner must read the published ref (or the local store) and
diff by event instead, with each change attributed to an agent thread and a branch; that is new
runner work, not part of this port.

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
| With a checkout | From the checkout's root: `cargo install --locked --force --path crates/beadroll --root ~/.local`. rustup reads `rust-toolchain.toml` from the current directory, not from `--path`, so run elsewhere this builds with the default toolchain and fails `rust-version`. |
| Without one | `cargo +1.99.0 install --locked --force --config net.git-fetch-with-cli=true --git ssh://git@github.com/bpcakes/beadroll.git beadroll --root ~/.local`. cargo rejects the scp-like `git@github.com:…` form; a `--git` install never reads the repository's toolchain file, hence `+1.99.0`; fetching through git uses the machine's own SSH setup. It needs read access to the repository. |

Both put `bead` in `~/.local/bin`, where `infra/` expects it (cargo also writes its install records,
`.crates.toml` and `.crates2.json`, in `~/.local`). The Rust implementation is version
0.2.0; Python's was 0.1.0. Each machine needs rustup; it already has git.

**Upgrading a machine.**
1. List the workspaces this machine uses, and pause its agents and other bead writers for the
   replacement. For each workspace with a coordination remote, run the current Python binary's
   `bead sync`; resolve a failed sync before proceeding. Inspect local-only workspaces as well.
   Save a store copy and the Python install revision for rollback. Do not delete `.command`
   files: `sync` can leave accepted receipts even with `pending_ownership: []`.
2. Verify the cutover Python runtime uses the default 4,300-digit integer conversion limit. If a
   machine used a non-default limit, stop and resolve its store compatibility before replacement;
   changing the oracle setting alone does not make that store safe. Run the read-only comparison
   on store copies, including all affected workspaces, before enabling the Rust binary.
3. Install the Rust binary as above. Run `bead --version` from the executable the agents will
   actually use and verify 0.2.0. For each workspace, `bead info` must show `integrity: []` and
   `pending_ownership: []`; retained accepted receipts are expected and remain recognisable.
4. Resume writers only after those checks pass. On failure, keep writers paused and reinstall
   the saved Python revision. Keep the current store and its receipts: replacing it with an old
   snapshot could discard later accepted work. Recheck the workspace with Python before resuming.

Machines can upgrade in any order. Separate-machine convergence and same-store upgrade/rollback
tests (§4.2) are both required evidence. Receipt compatibility holds throughout this window.

**Infrastructure executable selection, before removal.**
- `infra/drive.py` currently hardcodes the local `./bead`; `infra/roundtrip.py` and
  `infra/compat_real.py` also hardcode it. Add `BEAD_UNDER_TEST` as an explicit absolute local
  executable override in all three, with an installed `bead` fallback after launcher removal.
  Resolve the compatibility helper from the repository, not from the executable's directory.
  The oracle and `xtask check --real` need this for `roundtrip.py` and `compat_real.py` long
  before the cutover, so those two change in Phase 0 (§4.2); `drive.py` changes here.
  `soak.py` already takes its executable as its first positional argument; its caller must pass
  the selected Rust path. `drive.py` also needs per-host absolute executable overrides, retaining
  installed `~/.local/bin/bead` as its remote default. Rehearse against staged Rust binaries and
  throwaway stores before replacing live installations; the machine upgrade procedure above
  governs that later replacement.
- Before a driver writes test data, record the selected executable and `--version` on every
  participating machine. The cutover gate requires version 0.2.0 everywhere and fails if any host
  still selects Python. Retain the throwaway-remote and scratch-store restrictions.
- Prove explicit native selection while `./bead` still exists; after deleting it, exercise the
  drivers again. Run the four-machine gate and `xtask check --real` with the selected Rust binary;
  a passing drive with a local Python process does not count. Phase 7 owns `drive.py`'s change,
  the preflight and both proofs.

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
- `infra/` stays as stdlib-Python drivers with the explicit executable selection above, and can
  later move into xtask. Do not remove the launcher until the drivers can select Rust without it.

## 9. Risks

| Risk | Mitigation |
|---|---|
| Replay drifts from Python on some history | Differential testing (§4.2.3), the §5 ledger, and the mixed-fleet suite; no silent tightening |
| The generators miss a CPython behaviour no row names (bytes decoding, Unicode `re`, interpreter-dependent depth, exception texts) | §5 rows 7 and 22 to 26 are decided in Phase 1 and generated for; a new difference adds a row |
| CLI or output drift (help, argparse quirks, key order) | The oracle, goldens from Python, and the exact-text lists in the catalogues |
| Process, signal and stream edge cases | §2.3f, the two audited unsafe spots, and the hardening tests that already pin each case |
| git fidelity (environment, transport config, porcelain, translations) | Keep the binary and the exact environment; the git-version matrix |
| Test-port fidelity | Ported tests must pass against Python; `xtask inventory` requires completeness |
| The oracle counts a Python run as a Rust pass | Phase 0 makes every test select the binary under test, and Python bead refuses to start while the oracle targets another binary (§4.2) |
| Hidden slowness (F_FULLFSYNC, debug builds, real sleeps) | §2.3e, the profiles, handshakes, `xtask timing` |
| Scope creep | Parity first. Improvements go to the tracker as issues for after the cutover. |
| Upgrade or rollback replays an accepted request | Python-compatible receipt signatures/lifecycle; same-store tests; pause writers and check every affected workspace (§8) |
| Large Rust integer tokens are unreadable on Python | Preserve the default 4,300-digit boundary; parser/writer/import/replay fixtures; preflight non-default Python settings |
| Studio switches source without changing its persisted binding | Source revision and schema migration; approved endpoint/ref/workspace/credentials; existing-publication and later-change tests (§7) |
| Infrastructure gate accidentally runs Python locally | Explicit executable selection and per-machine version/path preflight before and after launcher removal (§8) |
| Studio's builds cannot fetch the private repository | Decision 7 (§10), before Phase 6: vendoring at a pinned revision, or credentials for every environment that builds studio, which changes its no-secret image rule (§7) |

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
7. **How studio's builds reach a private `beadroll-core`** (§7.3). Studio's image build carries no
   secret by rule, and its CI fetches git dependencies anonymously. Recommended: vendor
   `beadroll-core`'s source into studio at a pinned revision, refreshed by a script, so that no
   build needs credentials and decision 4 stands. The alternative is a BuildKit secret mount plus
   CI and local credentials, which changes studio's rule. Needed before Phase 6, not before.

Decided in Phase 1 (`roll-k2wy.2`), on 2026-10-09 (§5; the Python changes are U01–U11 in
`FINDINGS.md`):

8. **§5 row 7: one nesting ceiling of 512 levels**, in both implementations; Python counts the
   depth before `json.loads`. The maintainer chose it over the proposed 10,000 because, called
   from about 40 frames deep as bead's readers are, Python 3.11.15 stops at 951 levels, 3.12.10 at
   9,994, 3.13.14 at 9,995 and 3.14.7 at 116,188 on this Mac: 512 is below all of them, and bead
   writes at most 100.
9. **Rows 22, 23 and 25: reject**, as recommended. The maintainer delegated these, noting that only
   this repository and batter use bead. A key's number may have at most 4,299 digits, since a
   renamed copy of 4,300 nines would need 4,301.
10. **Row 24: reject**, against the recommendation to keep: ASCII digits and letters only, in both.
    Neither existing tracker moves: this repository names its workspace in `.beadroll.json`, and
    batter's origin is plain ASCII.
11. **Row 26: Rust only**, as planned: structured `CONFIG`, `IO` or `VALIDATION` errors in Rust,
    while Python keeps its tracebacks until the cutover, each departure recorded in `REVIEWING.md`
    §2 as it lands.
12. **Rows 27 to 30: reject**, decided under the delegation of rows 22, 23 and 25, which they
    resemble: shapes bead never writes, on which Python failed or answered by version. Neither
    existing tracker holds one (checked read-only on 2026-10-09).

**Revision of 2026-10-09, after review 8.** The maintainer accepted the audit corrections. The
port now preserves Python receipt bytes and their retry lifecycle, uses the default 4,300-digit
decimal boundary, and detaches git with `setsid`. Studio's scope includes persisted source and
ancestry migration plus approved coordination endpoints. Cutover explicitly selects native
infrastructure binaries and verifies same-store upgrades and rollback. Test support receives its
default executable from the caller, and failpoints distinguish first, nth and every hit.
These correct the assumptions demonstrated in
[`reviews/2026-10-09-8-codex-rust-port.md`](reviews/2026-10-09-8-codex-rust-port.md); the implementation
and new acceptance tests remain work for the epic's existing children. The original five decisions
above stand.

**Revision of 2026-10-09, after review 9.** A readiness audit against the code, the tests, Rust
1.99.0, the sibling repositories and studio
([`reviews/2026-10-09-9-claude-rust-port.md`](reviews/2026-10-09-9-claude-rust-port.md)). The
coordinated commands are named exactly, and an owner's close and delete write no receipt. Slice
2a ports `Link` and `synchronise` whole. §5 gains row 7's ceiling and rows 22 to 26, decided in
Phase 1. The lint configuration, test layout, install commands, CI gating and oracle harness are
corrected so that a skeleton passes and the oracle cannot count a Python run. The test classes are
recounted and the children reassigned to match. Studio's schema and build facts are added, with
decision 7 open. The five decisions stand.

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
  - Replay of 10k events: ≤ 20 ms in a release build. Python takes 0.7 s. Measured in Phase 1
    (`cargo run --release --package xtask -- timing --replay 10000`, a valid history on six
    machines, parsed and settled outside the clock, best of seven): 20 ms; Python's replay of the
    same journal took 73 ms.
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
| `BEAD_UNDER_TEST` | Read by test harnesses and retained local infrastructure drivers: explicit binary under test |
| `BEADROLL_ORACLE_REFUSALS` | Set by `cargo xtask oracle` while it targets a binary other than `./bead`: Python bead refuses to start (exit 3) and records the attempt in the file it names (§4.2) |
| `PYTHONINTMAXSTRDIGITS` | CPython setting: oracle pins 4300; upgrade preflight checks the old runtime's effective limit (§8); not a Rust product setting |
| `BEAD_SWEEP`, `BEAD_SWEEP_GC`, `BEAD_SWEEP_CONCURRENT` | Read by the sweep only: `full` runs all 660 schedules; `BEAD_SWEEP_GC=1` runs `git gc --prune=now` on the remote after every push, and `BEAD_SWEEP_CONCURRENT=1` runs the two follow-ups at once |
| `BEAD_TEST_REMOTE`, `BEAD_DRIVE_HOSTS`, `BEAD_TEST_LOCAL_REPO` | Read by `infra/` only |
| `HOME`, `TMPDIR`, `PATH` | Read implicitly: the default store root, the temporary files that take git's output, and where `git` and `ps` are found |
| `GIT_CONFIG_COUNT` and its `GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n`, `GIT_SSH`, `GIT_SSH_COMMAND`, the `REPO_LOCATION_ENV` names | Read for `sync.git`'s environment (§2.3d): the count is extended, never replaced; either SSH variable suppresses bead's default; the location variables are removed |

Product settings are read only by the CLI crate and translated into `Settings`, never read inside
a library. Harness and infrastructure variables are consumed by their respective tools.
