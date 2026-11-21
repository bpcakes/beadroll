# Browsing beadroll in T3 Code (MCP App)

`beadroll-mcp` is an optional read-only [MCP App](https://modelcontextprotocol.io/extensions/apps/overview)
companion server that lets you browse a repository's ready issues and inspect issue details inside
a stock [T3 Code](https://github.com/pingdotgg/t3code) thread — without modifying T3 Code or
running a separate web server.

## What it does

- **Ready list (`bead_ready`)**: shows up to 25 ready issues in beadroll's order with key, title,
  priority, type, labels, workspace identity, and last sync time (plus a clear empty state).
- **Issue detail (`bead_show`)**: click any row (or ask the agent to show an issue) to read its
  description, acceptance criteria, status, claim owner, dependencies, dependents, and comments
  while keeping the ready list available via **← Ready issues**.
- **Refresh**: rereads the local tracker and refreshes the last sync timestamp. `beadroll-mcp`
  performs **local reads only** (`bead info`, `bead ready`, `bead show`). It never runs
  `bead sync`, never contacts a remote, and never mutates issues; claims, edits, and `bead sync`
  remain explicit CLI actions.
- **Stable issue IDs**: once `bead` returns an issue, every row click and refresh reopens it by
  `(workspace, uid)`, so a row whose display key was renumbered during sync still opens the right
  issue. An older `bead` without stable `uid` output returns an actionable `UPGRADE_REQUIRED`
  error.
- **Text fallback**: both tools also return readable plain text in `content`, so non-MCP-App
  clients and providers work without rendering HTML.

## Requirements

- `bead` built with stable issue UIDs (PR #319 or newer) installed on `PATH` (or passed with
  `--bead PATH`).
- Stock T3 Code with either:
  - **Codex** (MCP Apps supported out of the box), or
  - **Claude Code 2.1.301+** (MCP Apps supported out of the box).

## Install

From the root of a beadroll checkout:

```sh
cargo install --locked --force --path crates/beadroll --root ~/.local
cargo install --locked --force --path crates/beadroll-mcp --root ~/.local
```

Ensure `~/.local/bin` is on your `PATH`, then verify:

```sh
bead --version
beadroll-mcp --version
```

## Configure in your repository

`beadroll-mcp` binds one checkout once at startup and refuses to guess a repository from tool
arguments. Both Codex (including T3 Code's shared `codex app-server`) and Claude Code launch
project-scoped stdio MCP servers with the thread's checkout as their working directory. Pass
`--repo /abs/path/to/checkout` if your host does not set the server's working directory to the
checkout.

### Option A: Codex (`.codex/config.toml`)

In the repository you want to browse, create `.codex/config.toml` (and make sure the project is
trusted in `~/.codex/config.toml`):

```toml
[mcp_servers.beadroll]
command = "beadroll-mcp"
```

To bind an explicit checkout or `bead` binary path:

```toml
[mcp_servers.beadroll]
command = "beadroll-mcp"
args = ["--repo", "/path/to/your-repo", "--bead", "/path/to/bead"]
```

### Option B: Claude Code (`.mcp.json`)

In the repository you want to browse, create `.mcp.json` (and approve the project server when
Claude Code prompts):

```json
{
  "mcpServers": {
    "beadroll": {
      "command": "beadroll-mcp"
    }
  }
}
```

Or bind an explicit checkout or `bead` binary:

```json
{
  "mcpServers": {
    "beadroll": {
      "command": "beadroll-mcp",
      "args": ["--repo", "/path/to/your-repo", "--bead", "/path/to/bead"]
    }
  }
}
```

## Verify in a fresh T3 Code thread

1. Open your repository in T3 Code and start a fresh **Codex** or **Claude Code** thread.
2. Ask: **"What's ready in beadroll?"** — the agent calls `bead_ready` and T3 renders the
   interactive issue list below the tool call.
3. Click a row to open its full details (description, acceptance criteria, status, claim owner,
   dependencies, comments). Because `bead_show` and `bead_ready` declare `readOnlyHint: true`,
   T3 executes clicks and refreshes without asking for confirmation.
4. Click **← Ready issues**, then open a second issue.
5. Make a local change in a terminal (for example `bead create "Check MCP refresh" --offline` or
   `bead comments add ID "Updated locally" --offline`) and click **Refresh** in the app. The new
   local state appears immediately without running `bead sync`.

## Troubleshooting

- **`UPGRADE_REQUIRED`**: the installed `bead` binary does not emit `uid` in `bead ready --json` /
  `bead show --json`. Upgrade `bead` to a build that includes stable issue UIDs (PR #319 or newer)
  and restart the thread/server.
- **`BEAD_NOT_FOUND`**: `bead` is not on the `PATH` inherited by Codex or Claude Code. Add
  `~/.local/bin` to your login `PATH` or pass `--bead` with the absolute path to `bead`.
- **`NOT_INITIALIZED`**: the bound repository has no local tracker yet. Run `bead sync` (to join an
  existing tracker) or `bead init --prefix NAME` (to start one), then click **Refresh**.
- **`WORKSPACE_MISMATCH`**: an interactive click sent a `workspace` ID from another tracker to a
  server bound to a different checkout. Open a thread in that repository instead.
- **Session stopped**: T3 Code keeps rendered MCP Apps visible after a thread stops, but interactive
  clicks require the thread's agent session to be running. Send a message in the thread to wake it.
