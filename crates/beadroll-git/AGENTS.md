# beadroll-git crate guide

## Purpose

`crates/beadroll-git` is the only code in bead that starts git (`docs/RUST-PORT.md` §2.1, §2.3d,
§2.3f): the runner (its own session, a timeout that kills the process group for a fetch and only
the client for a push, output captured to unlinked files, signal names), the long-lived gits of
Phase 4 (Appendix B) with git's own tree and commit reading mirrored in-process, the checkout's
facts asked once and its configuration and HEAD asked afresh, the environment `sync.git`'s git
runs in, the
push-outcome and local-damage classifiers, the remote URL rules, and Python's lenient `realpath`,
which the store shares. Slice 2a (`roll-k2wy.3`) built it; Phase 4 (`roll-k2wy.13`) consolidated
the calls.

## Key entrypoints

- `src/runner.rs`: `Env` (a git's whole environment, its directory and where its output files go),
  `Git` (`run`, `check`, `text`, `string`, with `input`, `timeout` and `reap`), `Output`
  (`checked_text`); the audited `pre_exec` (`setsid`, SIGXFSZ back to default).
- `src/session.rs`: `Session`, one long-lived git spoken to through pipes, started as the runner
  starts every git and killed with its helpers and reaped when dropped; `NoAnswer`.
- `src/objects.rs`: `Objects` (`cat-file --batch-command`: `name`, `get`, `start`, `stop`),
  `Object`, `list_tree` (what `ls-tree -r -z COMMIT` lists) and `root_tree` (what `rev-parse
  COMMIT^{tree}` finds), with git's own checks: tree entries as `decode_tree_entry` reads them,
  commits as `parse_commit_buffer` does, the commit's and the root tree's names checked against
  their contents.
- `src/writers.rs`: `Hasher` (`hash-object -w --no-filters --stdin-paths`), `Trees` (`mktree
  --batch`), `Refs` (`update-ref --stdin` transactions).
- `src/checkout.rs`: `Checkout` and `Fact`: one combined `rev-parse` (top, common directory,
  object format), each line cut back to what Python's single call prints, the single calls when
  the combined one cannot answer; `origin_urls`, Python's own `config --get-regexp` of
  `ORIGIN_KEYS` asked each time the workspace is found (the first time beside the combined
  `rev-parse`). `head_and_branch` and `HeadAnswers`: HEAD and its branch, asked each time a
  transaction starts, in one `rev-parse` where its two lines are certain.
- `src/alongside.rs`: `Git::run_alongside`, two local gits at once, both started from the calling
  thread.
- `src/format.rs`: `stored_format`, `StoredFormat`: `extensions.objectformat` read from a plain
  config file, where its `core.repositoryformatversion` makes git read it. `src/hash.rs`: object
  names (SHA-1 written out, SHA-256 from `sha2`).
- `src/env.rs`: `sync_env` (Python's `Link.__init__` environment), `transport_settings` (Python's
  `config --null --get-regexp` of `TRANSPORT_KEYS`, asked each time a link is made),
  `transport_config` (from its answer), `REPO_LOCATION_ENV`, `SYNC_REPO_CONFIG`, `CountRules` (how
  `GIT_CONFIG_COUNT` is read).
- `src/classify.rs`: `local_damage`, `remote_lacks_objects`, `push_outcome` and `PushOutcome`.
- `src/url.rs`: `without_password`, `normalize_remote`, `is_local_path`, `py_strip`, `py_lower`.
- `src/path.rs`: `exists`, `realpath` (CPython 3.14's, not libc's).
- `src/error.rs`: `Error`, `IoError` (Python's `OSError` text), `strerror`; `src/signal.rs`:
  signal names; `src/text.rs`: Python's `strip()` and `splitlines()` where git's output is read.
- `tests/git/`: the Python suite's tests of the URL rules, the runner's timeouts and the
  local-damage classifier, ported under the naming rule of `docs/RUST-PORT.md` §4.2. The unit
  tests of the Phase 4 code
  compare each in-process answer with git's own, in both object formats (`src/testing.rs`).

## Edit here for X

- A git invocation, or the environment one runs in: here, and nowhere else.
- A question asked of a long-lived git instead of a one-shot one: here, with its fallback in the
  caller (`NoAnswer` means "run Python's git").
- How a push's outcome or a local read error is classified: `src/classify.rs`.
- `without_password`, `normalize_remote`, `is_local_path`: `src/url.rs`, working on bytes as Python
  works on text decoded with `surrogateescape`.

## Invariants

- No other crate starts git.
- Phase 4 asks fewer gits than Python, never anything else: every in-process or long-lived answer
  is exactly the one Python's git gives, and whatever this code cannot answer with certainty
  (`NoAnswer`: missing, damaged, unusual, a name it would have to quote, no git) goes to that git,
  whose answer or failure is the one bead gives. Code that reads git's formats mirrors git's own
  checks and is at least as strict.
- A question is asked when Python asks it. `Checkout` keeps only the top, the common directory and
  the object format, which do not change while bead runs. What can change is asked afresh, by
  Python's own call, where Python asks it: origin's URLs whenever the workspace is found, the
  transport settings whenever a link is made, HEAD and its branch whenever a transaction starts
  (under the store lock, as Python's `Tx` asks them).
- A long-lived git starts as the runner starts every git (its own session, SIGXFSZ default, the
  caller's environment and directory, no descriptor but its pipes and an unlinked stderr file), has
  no time limit as no local runner call has, reads answers only as far as they are announced, and is
  killed with its helpers and reaped when dropped.
- A `hash-object` or `mktree` never outlives the fetch after which it started: one that looked for
  packs before a fetch would not see the fetch's pack (and `hash-object` would write loose copies).
- Gits are started from one thread: two run at once (`run_alongside`) are started one after the
  other, so neither can inherit a descriptor whose close-on-exec flag the other's start had not
  set yet (on macOS, std makes a pipe and marks it in two steps).
- The runner's `pre_exec` (a real `setsid`, SIGXFSZ back to default) is one of the two audited
  `unsafe` spots, with a `// SAFETY:` comment (§2.2); never `process_group(0)` first.
- `sync.git`'s git runs with `LC_ALL=C`; checkout-side git keeps the caller's locale.
- It depends on no other workspace crate (core and git build in parallel): what it needs of
  Python's text rules it is handed (`CountRules`) or keeps to ASCII.
- No panics outside tests; nothing read from the process environment, the working directory or the
  clock: every git runs in an `Env` its caller built.

## Common commands

- `cargo test -p beadroll-git` (`cargo nextest run -p beadroll-git`: V1's descriptor count needs a
  process of its own)
- `cargo clippy -p beadroll-git --all-targets -- -D warnings -D clippy::mod_module_files`
