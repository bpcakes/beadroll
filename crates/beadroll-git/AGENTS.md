# beadroll-git crate guide

## Purpose

`crates/beadroll-git` is the only code in bead that starts git (`docs/RUST-PORT.md` §2.1, §2.3d,
§2.3f): the runner (its own session, a timeout that kills the process group for a fetch and only
the client for a push, output captured to unlinked files, signal names), the environment `sync.git`'s
git runs in, the push-outcome and local-damage classifiers, the remote URL rules, and Python's
lenient `realpath`, which the store shares. Slice 2a (`roll-k2wy.3`) built it.

## Key entrypoints

- `src/runner.rs`: `Env` (a git's whole environment, its directory and where its output files go),
  `Git` (`run`, `check`, `text`, `string`, with `input`, `timeout` and `reap`), `Output`; the audited
  `pre_exec` (`setsid`, SIGXFSZ back to default).
- `src/env.rs`: `sync_env` (Python's `Link.__init__` environment), `transport_config`,
  `REPO_LOCATION_ENV`, `SYNC_REPO_CONFIG`, `CountRules` (how `GIT_CONFIG_COUNT` is read).
- `src/classify.rs`: `local_damage`, `remote_lacks_objects`, `push_outcome` and `PushOutcome`.
- `src/url.rs`: `without_password`, `normalize_remote`, `is_local_path`, `py_strip`, `py_lower`.
- `src/path.rs`: `exists`, `realpath` (CPython 3.14's, not libc's).
- `src/error.rs`: `Error`, `IoError` (Python's `OSError` text), `strerror`; `src/signal.rs`:
  signal names; `src/text.rs`: Python's `strip()` and `splitlines()` where git's output is read.
- `tests/git/`: the Python suite's tests of the URL rules, the runner's timeouts and the
  local-damage classifier, under the inventory's naming rule.

## Edit here for X

- A git invocation, or the environment one runs in: here, and nowhere else.
- How a push's outcome or a local read error is classified: `src/classify.rs`.
- `without_password`, `normalize_remote`, `is_local_path`: `src/url.rs`, working on bytes as Python
  works on text decoded with `surrogateescape`.

## Invariants

- No other crate starts git.
- Phase 2 issues exactly the commands Python issues, in the environment Python builds (§2.3d);
  Phase 4 consolidates them.
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
