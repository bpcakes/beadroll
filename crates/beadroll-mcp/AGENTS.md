# beadroll-mcp crate guide

## Purpose

`crates/beadroll-mcp` is an optional read-only MCP App companion server (`beadroll-mcp`) for
browsing a beadroll tracker inside stock T3 Code threads and other MCP Apps hosts. It speaks
newline-delimited JSON-RPC 2.0 over stdio, binds one checkout once at startup, runs the installed
`bead` executable for local reads only (`bead info`, `bead ready`, `bead show`), and serves a
single self-contained HTML view at `ui://beadroll/issues.html`.

## Key entrypoints

- `src/main.rs`: executable entrypoint; delegates to [`beadroll_mcp::run`].
- `src/cli.rs`: the only module that reads CLI arguments (`--repo`, `--bead`), the startup
  working directory, and the process environment; builds an explicit `Config` with git routing
  variables scrubbed and handles closed stdout pipes cleanly.
- `src/lib.rs`: `Config`, `run_stdio`, `UI_RESOURCE_URI`, `UI_RESOURCE_MIME_TYPE`, `UI_HTML`, and
  `MAX_READY_LIMIT`.
- `src/protocol.rs`: `Server`, JSON-RPC 2.0 stdio framing, `initialize` version negotiation,
  `tools/list`, `tools/call`, `resources/list`, `resources/templates/list`, `resources/read`,
  `ping`, and `-32601` for unknown methods (such as Claude Code's `server/discover`).
- `src/tools.rs`: tool metadata for `bead_ready` and `bead_show` (both marked
  `annotations.readOnlyHint = true` and linked to `ui://beadroll/issues.html`), strict argument
  validation, and dispatch.
- `src/bead.rs`: `BeadCli` and `AppError`; runs `bead` in the bound checkout, distinguishes missing
  checkout directories from a missing `bead` binary, enforces workspace ID matching on `bead_show`,
  parses structured bead error JSON, and enforces stable `uid` presence.
- `src/model.rs`: typed workspace, ready-row, and issue-detail models decoded from `bead --json`
  and serialized into `structuredContent`.
- `src/format.rs`: readable plain-text `content` formatting for non-UI MCP clients and model
  context.
- `ui/index.html`: bundled single-file MCP App view (zero external origins or bundler), talking
  JSON-RPC 2.0 over `window.parent.postMessage`.
- `tests/mcp/`: integration tests covering MCP protocol framing, browsing, checkout/workspace
  binding, stable `uid` reopening across key renames, older-`bead` upgrade messages, and real
  `bead` runs in `beadroll-test-support::Sandbox`.

## Edit here for X

- Tool schemas, read-only annotations, or argument validation: `src/tools.rs`.
- JSON-RPC lifecycle or resource serving: `src/protocol.rs`.
- Subprocess invocation, error codes, or `uid`/workspace enforcement: `src/bead.rs` and
  `src/model.rs`.
- Text-only fallback formatting: `src/format.rs`.
- Interactive issue browser layout, theming, or iframe bridge: `ui/index.html`.
- Integration coverage: `tests/mcp/<subject>.rs`.

## Invariants

- Read-only and local-only: `beadroll-mcp` invokes only `bead info`, `bead ready`, and `bead show`.
  It never runs `bead sync`, never contacts a git remote, and never mutates the tracker.
- Session-bound checkout: `Config.repo` is fixed once at server startup (`--repo` or startup cwd),
  git routing variables (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, `GIT_INDEX_FILE`,
  `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_SUPER_PREFIX`) are scrubbed,
  and tool arguments never accept a repository path.
- Stable identity on reopen: once an issue is returned, callers keep its `workspace` ID and `uid`
  and pass both to `bead_show`. A mismatched `workspace` ID is refused (`WORKSPACE_MISMATCH`), and
  a `bead` binary that omits `uid` produces `UPGRADE_REQUIRED`.
- No ambient state in the library: only `src/cli.rs` reads process arguments, cwd, or environment.
- No panics, no `unsafe`, no `print!`/`eprintln!`, no `mod.rs`, and no external network origins in
  `ui/index.html` (strict default MCP App CSP).
- `_meta.ui.resourceUri` is always advertised on both tools (never gated on client `initialize`
  capabilities) so hosts like Claude Code that omit `io.modelcontextprotocol/ui` during
  `initialize` still render the app, while text-only clients receive full plain-text `content`.

## Common commands

- `cargo build -p beadroll-mcp`
- `cargo nextest run -p beadroll-mcp`
- `cargo clippy -p beadroll-mcp --all-targets -- -D warnings -D clippy::mod_module_files`
- `cargo install --locked --force --path crates/beadroll-mcp --root ~/.local`
