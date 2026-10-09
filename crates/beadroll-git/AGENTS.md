# beadroll-git crate guide

## Purpose

`crates/beadroll-git` is the only code in bead that starts git (`docs/RUST-PORT.md` §2.1, §2.3d,
§2.3f): the runner (its own session, a timeout that kills the process group for a fetch and only
the client for a push, output captured to unlinked files, signal names), the environments for the
checkout and for `sync.git`, typed commands, the push-outcome and local-damage classifiers, and the
remote URL rules. Slice 2a (`roll-k2wy.3`) fills it; until then it is empty.

## Key entrypoints

- `src/lib.rs`: the crate root, and so far all of it.

## Edit here for X

- A git invocation, or the environment one runs in: here, and nowhere else.
- How a push's outcome or a local read error is classified: here.
- `without_password`, `normalize_remote`, `is_local_path`: here.

## Invariants

- No other crate starts git.
- Phase 2 issues exactly the commands Python issues, in the environment Python builds (§2.3d);
  Phase 4 consolidates them.
- The runner's `pre_exec` (a real `setsid`, SIGXFSZ back to default) is one of the two audited
  `unsafe` spots, each block with a `// SAFETY:` comment (§2.2); never `process_group(0)` first.
- `sync.git`'s git runs with `LC_ALL=C`; checkout-side git keeps the caller's locale.
- No panics outside tests; nothing read from the process environment except what the caller passes.

## Common commands

- `cargo test -p beadroll-git`
- `cargo clippy -p beadroll-git --all-targets -- -D warnings -D clippy::mod_module_files`
