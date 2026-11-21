# beadroll-mcp crate guide

## Purpose

`crates/beadroll-mcp` is an optional read-only MCP App companion server (`beadroll-mcp`) for
browsing a beadroll tracker inside stock T3 Code threads and other MCP Apps hosts. It speaks
newline-delimited JSON-RPC 2.0 over stdio, binds one checkout once at startup, runs the installed
`bead` executable for local reads only (`bead info`, `bead ready`, `bead show`), and serves a
single self-contained HTML view at `ui://beadroll/issues.html`.

## Key entrypoints

- `src/main.rs`: executable entrypoint; delegates to [`beadroll_mcp::run`].
- `src/cli.rs`: the only module that reads CLI arguments (`--repo`, `--bead`, `--timeout-ms`), the
  startup working directory, and the process environment; canonicalizes the checkout once at
  startup, builds an explicit `Config` with git and beadroll routing overrides scrubbed, and
  handles closed stdout pipes cleanly.
- `src/lib.rs`: `Config`, `run_stdio`, `UI_RESOURCE_URI`, `UI_RESOURCE_MIME_TYPE`, `UI_HTML`,
  `MAX_READY_LIMIT`, `MAX_FRAME_BYTES`, `DEFAULT_TIMEOUT`, and `MAX_OUTPUT_BYTES`.
- `src/protocol.rs`: `Server`, bounded byte-frame reader (recovering from invalid UTF-8 and
  oversized frames), strict JSON-RPC 2.0 envelope validation, `initialize` version negotiation,
  `tools/list`, `tools/call`, `resources/list`, `resources/templates/list`, `resources/read`,
  `ping`, and `-32601` for unknown methods (such as Claude Code's `server/discover`).
- `src/tools.rs`: tool metadata for `bead_ready` and `bead_show` (both marked
  `annotations.readOnlyHint = true` and linked to `ui://beadroll/issues.html`), argument
  validation (returning MCP tool errors `INVALID_ARGUMENT` per MCP 2025-11-25), and dispatch.
- `src/process.rs`: nonblocking `poll(2)` subprocess supervisor in its own process group
  (`CommandExt::process_group(0)`) with per-stream byte caps and `rustix::process::kill_process_group`
  cleanup while the leader PID is still unreaped, closing read ends cleanly even if an
  escaped-session descendant retains a write end.
- `src/bead.rs`: `BeadCli`, `ShowTarget`, and `AppError`; runs `bead` in the bound checkout,
  pins `BEAD_WORKSPACE` on `ready`/`show` to the workspace ID checked by `bead info`, enforces
  workspace ID matching and exact `uid` matching on explicit-UID `bead_show` lookups, parses
  structured bead error JSON, and enforces stable `uid` presence.
- `src/model.rs`: typed workspace, ready-row, and issue-detail models (including `unconfirmed`,
  `lost_claims`, `stale`, `closed_on`/`merged`, multi-parent `parents`/`parent_uids`, and
  `tracked_config_warning`) decoded from `bead --json` and serialized into `structuredContent`.
- `src/format.rs`: readable plain-text `content` formatting for non-UI MCP clients and model
  context.
- `ui/index.html`: bundled single-file MCP App view (zero external origins or bundler), talking
  JSON-RPC 2.0 over `window.parent.postMessage`.
- `tests/mcp/`: integration tests covering MCP protocol framing, browsing, checkout/workspace
  binding, stable `uid` reopening across key renames, UI contract invariants, and real `bead` runs
  in `beadroll-test-support::Sandbox`.

## Edit here for X

- Tool schemas, read-only annotations, or argument validation: `src/tools.rs`.
- JSON-RPC framing, envelope checks, lifecycle, or resource serving: `src/protocol.rs`.
- Subprocess timeouts or pipe capture: `src/process.rs`.
- Subprocess invocation, workspace pinning, error codes, or `uid` enforcement: `src/bead.rs` and
  `src/model.rs`.
- Text-only fallback formatting: `src/format.rs`.
- Interactive issue browser layout, accessibility, theming, or iframe bridge: `ui/index.html`.
- Integration coverage: `tests/mcp/<subject>.rs`.

## Invariants

- Read-only and local-only: `beadroll-mcp` invokes only `bead info`, `bead ready`, and `bead show`.
  It never runs `bead sync`, never contacts a git remote, and never mutates tracker events (though
  `bead` may refresh its own derived local projection/text caches).
- Session-bound checkout: `Config.repo` is canonicalized once at startup (`--repo` or startup cwd),
  inherited git routing/config overrides, `XDG_CONFIG_HOME`, and all `BEAD_*` variables except
  `BEAD_HOME` are scrubbed, an offline repo-local-only Git environment (`GIT_CONFIG_GLOBAL=/dev/null`,
  `GIT_CONFIG_SYSTEM=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_NO_LAZY_FETCH=1`,
  `GIT_ALLOW_PROTOCOL=""`, `GIT_TERMINAL_PROMPT=0`) is injected so external configs cannot redirect
  workspace discovery and partial clones cannot lazily fetch on `bead show`, `bead ready` and `bead
  show` pin `BEAD_WORKSPACE` to the exact ID resolved by `bead info`, and tool arguments never
  accept a repository path.
- Stable identity on reopen: once an issue is returned, callers keep its `workspace` ID and `uid`
  and pass both to `bead_show`. A mismatched `workspace` ID is refused (`WORKSPACE_MISMATCH`), an
  explicit `uid` whose response returns a different `uid` (key collision) is refused
  (`UID_MISMATCH`), and a `bead` binary that omits `uid` produces `UPGRADE_REQUIRED`.
- No ambient state in the library: only `src/cli.rs` reads process arguments, cwd, or environment.
- No panics, no `unsafe`, no `print!`/`eprintln!`, no `mod.rs`, and no external network origins or
  `innerHTML` sinks in `ui/index.html` (strict default MCP App CSP).
- `_meta.ui.resourceUri` is always advertised on both tools (never gated on client `initialize`
  capabilities) so hosts like Claude Code that omit `io.modelcontextprotocol/ui` during
  `initialize` still render the app, while text-only clients receive full plain-text `content`.

## Common commands

- `cargo build -p beadroll-mcp`
- `cargo nextest run -p beadroll-mcp`
- `cargo clippy -p beadroll-mcp --all-targets -- -D warnings -D clippy::mod_module_files`
- `cargo install --locked --force --path crates/beadroll-mcp --root ~/.local`
