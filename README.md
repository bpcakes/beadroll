# beadroll

[![CI](https://github.com/bpcakes/beadroll/actions/workflows/rust.yml/badge.svg)](https://github.com/bpcakes/beadroll/actions/workflows/rust.yml)

An issue tracker for coding agents working across Git worktrees, clones, and
machines. beadroll provides the `bead` command: a shared tracker with exclusive
task claims, dependency tracking, and Beads JSONL import and export.

- Every worktree and clone of a repository uses the same tracker.
- Claims are confirmed by the remote, so two agents cannot hold the same task.
- Tracker data lives outside your checkout and syncs through a dedicated Git
  ref. Your code branches stay free of tracker commits.
- One Rust binary uses your existing Git remote and credentials. No server,
  daemon, or database to set up.

[Install](#install) · [Quick start](#quick-start) · [For agents](#for-agents) ·
[Migration guide](MIGRATING.md) · [Contributing](CONTRIBUTING.md)

## Install

Requirements: Linux or macOS, git 2.40+, and [rustup](https://rustup.rs).
beadroll builds from source. From a new checkout:

```sh
git clone https://github.com/bpcakes/beadroll.git
cd beadroll
cargo install --locked --force --path crates/beadroll --root ~/.local
export PATH="$HOME/.local/bin:$PATH"
bead --version
```

Run the install command from the repository root so rustup picks up
[rust-toolchain.toml](rust-toolchain.toml) and installs Rust 1.99.0. The binary
goes in `~/.local/bin`; keep that directory on your `PATH`. To upgrade, pull
the latest source and run the install command again.

<details>
<summary>Install directly from Git without a checkout</summary>

```sh
rustup toolchain install 1.99.0 --profile minimal
cargo +1.99.0 install --locked --force --config net.git-fetch-with-cli=true \
  --git https://github.com/bpcakes/beadroll.git beadroll --root ~/.local
```

The explicit toolchain is required because `cargo install --git` does not
read the repository's toolchain file. Add `~/.local/bin` to your `PATH` as above.

</details>

## Quick start

In a Git repository whose `origin` you can push to, initialize a tracker once:

```sh
cd your-project
bead init --prefix app
bead prime
bead create "Fix the rate limiter after a restart" -t bug -p 1
```

`init` publishes the tracker to `refs/beads/journal` on `origin`. It writes
no files in your checkout and makes no commits on your code branches. The
origin URL identifies the tracker across worktrees, clones, and machines.

Use the issue ID printed by `create` in place of `app-zs43` below:

```sh
bead ready
bead claim app-zs43
bead comments add app-zs43 "The counter resets before the first tick."
# Make and verify the fix, then record the outcome.
bead close app-zs43 -r "The counter now survives a restart."
bead show app-zs43
```

A confirmed claim belongs to its owner until release, close, or takeover.
Other agents cannot claim it, and `bead ready` excludes it while held.

On another machine, run these commands in any clone of the same repository:

```sh
bead sync
bead ready
```

Writes publish automatically. With `--offline`, or when the remote does not
answer, changes remain local and bead reports what is unpublished. Run
`bead sync` to publish them and fetch other machines' work. Reads such as
`ready`, `show`, and `list` use local state and never contact the network;
sync first when you need the latest changes.

## Common commands

| Task | Command |
|---|---|
| Find available work | `bead ready`, or `bead ready EPIC` for an epic's subtree |
| Inspect an issue and its history | `bead show ID`, `bead log ID` |
| List or search issues | `bead list`, `bead search "query"` |
| See who is working on what | `bead claims` |
| Hand back unfinished work | `bead release ID` |
| Create a child task | `bead create "Title" --parent ID` |
| Inspect blockers | `bead blocked`, `bead dep tree ID` |
| Review activity across local trackers | `bead recap` |
| Inspect the store and remote | `bead info` |

Run `bead` for the command overview and `bead help COMMAND` for usage and
options. Add `--json` for machine-readable output.

`bead recap` works from any directory and reads all local trackers without
syncing. The first recap shows the last 24 hours. Terminal output marks
activity as seen; pipes, files, and `--json` only read it unless you pass
`--advance`.

## For agents

Add the workflow to your repository's agent instructions:

```sh
bead onboard >> AGENTS.md
```

Use `CLAUDE.md` instead if that is where your agent reads instructions. Fill
in the project specifics at the end of the generated block. Each agent then
starts a session with `bead prime`, which reports its identity, held issues,
workflow, and error codes.

bead detects Claude Code and Codex session identities from their environments.
A claim belongs to the agent thread that made it. To continue your own work
in a new session:

```sh
bead takeover ID -r "same task, new session"
```

Use `--actor NAME` or `BEAD_ACTOR` for an explicit identity. Verify who owns a
task before taking it over; claims never expire automatically.

For automation, use `--json` and inspect structured error codes such as
`ALREADY_CLAIMED`. If a write succeeds but bead cannot count unpublished
events, the result stays on stdout and a warning goes to stderr. With
`--json`, the warning is a separate JSON document. Do not repeat a successful
write because of that warning; follow its repair instructions. See the
[command guarantees](docs/GUARANTEES.md) for error and recovery details.

## Coming from Beads

beadroll reads and writes [Beads](https://github.com/steveyegge/beads) JSONL,
including issue IDs, parent-child keys, dependencies, comments, labels,
statuses, and timestamps. Preview an import, then apply it:

```sh
bead init --import .beads/issues.jsonl --dry-run
bead init --import .beads/issues.jsonl
```

After migration, use `bead` for tracker changes. The old `.beads/` directory
is no longer read or updated automatically. Export when a Beads reader such
as `bv` needs a copy:

```sh
bead export -o issues.jsonl
```

Imported `in_progress` issues have no owner until someone claims them.
beadroll's claims have no Beads equivalent and are not exported. The
[migration guide](MIGRATING.md) covers preparation, validation, and moving back.

Compatibility checks cover import, export, `ready`, and `blocked` against
`br` 0.5.7 on 48 real trackers, plus reading exports with `bv`. Upstream `bd`
has not been tested. Imports containing memory records (`_type: memory`)
are rejected. Commands and features not implemented include `stats`, `count`,
`epic status`, `graph`, saved queries, templates, external-project
dependencies, and `bv`-style ranking.

## How it works

Each machine appends events to its own journal under
`~/.local/state/beadroll/<workspace>/`. Worktrees and clones on that machine
share the store. Sync exchanges journals through `refs/beads/journal` on the
coordination remote, which defaults to `origin`. Events record what their
writer had seen, allowing every machine to reconstruct the same issue state.

Claims use a conditional update of the remote ref. A claim is exclusive only
after the remote accepts it; an offline or unanswered claim is provisional.
Publishing is serialized on that ref, so heavy contention and network latency
can delay writes.

The stored and published format is frozen at version 1, with additive changes
only. Machines can upgrade in any order. Older versions preserve and report
events they cannot apply until upgraded. The [guarantees](docs/GUARANTEES.md)
describe recovery after interrupted writes, lost acknowledgements, and damaged
stores, along with measured performance and known limits.

### When two machines edit the same thing

| Concurrent changes | Result after sync |
|---|---|
| Edit different fields | Both edits are kept. |
| Edit the same field | Both values are kept under `conflicts`; the newer one is shown. A later write that has seen both settles the conflict. |
| Set different statuses | The conflict also prevents readiness, claims, and unblocking other issues until settled. |
| Add comments | All comments are kept. |
| Add and remove a label or dependency | A removal affects only additions its writer had seen. |
| Create children of the same parent | A colliding child key is renumbered, for example `app-x.1` to `app-x.2`, and sync reports it. |
| Add dependencies that form a blocking cycle | Dependencies are kept and reported by `bead dep cycles`; affected issues are not ready. |

An edit made after seeing another is not a conflict, regardless of clock
differences. Child keys can change after concurrent creation; a key copied
into a commit message before sync may then refer to another issue.

## Configuration

Most repositories need no configuration. bead derives the workspace from the
origin URL; SSH and HTTPS URLs for the same host and repository resolve to
the same workspace. A repository with neither a remote nor a `.beadroll.json`
gets a machine-local tracker that cannot sync.

For a different coordination remote, create a tracked configuration:

```sh
bead init --tracked --remote NAME_OR_URL
```

Commit the generated `.beadroll.json` on the default branch so other
checkouts can find it. bead uses your checkout's Git transport settings,
including credential helpers and SSH configuration. See
[configuration](docs/GUARANTEES.md#configuration) and
[moving a repository](docs/GUARANTEES.md#moving-a-repository) for details.

All environment variables are optional:

| Variable | Purpose / default |
|---|---|
| `BEAD_HOME` | Store root; must be absolute after `~` expansion. Default: `~/.local/state/beadroll`. |
| `BEAD_WORKSPACE` | Override the derived workspace ID and its check; see the repository move guide above. |
| `BEAD_REMOTE` | Coordination remote for this call: a remote name, URL, or path. |
| `BEAD_AGENT`, `BEAD_THREAD`, `BEAD_ACTOR` | Override detected identity. `--actor` takes precedence. |
| `CLAUDE_CODE_SESSION_ID`, `CLAUDECODE`, `CODEX_THREAD_ID`, `CODEX_SESSION_ID` | Agent harness and session detection. |
| `BEAD_NET_TIMEOUT` | Fetch or push timeout in seconds, including decimals. Default: 45. |
| `BEAD_ROTATE_BYTES` | Journal segment rotation size. Default: 262,144 bytes. |
| `BEAD_REPACK_LOOSE`, `BEAD_REPACK_PACKS` | Repack thresholds for loose objects and packs. Defaults: 1,000 and 40. |
| `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `TERM` | Terminal color; see the [command-line reference](docs/GUARANTEES.md#the-command-line). |
| `COLUMNS` | Text wrapping width; otherwise the terminal width, capped at 100. |

Invalid numeric settings produce a `CONFIG` error, even for `--version`.
Variables used by tests and infrastructure scripts are listed in
[CONTRIBUTING.md](CONTRIBUTING.md#variables-the-tests-and-drivers-read).

## Published data

Anyone who can read the coordination repository can read its tracker. On a
public repository, issue text and history are public too.

Published events contain issue data, an opaque machine ID, the agent type and
thread or explicit actor name, a timestamp, and the branch and commit where
the command ran. Set `"where": false` in `.beadroll.json` to omit the branch
and commit from future events. Hostnames used as machine aliases stay local;
remote URL credentials are removed before storage or display.

## Documentation and support

- [Migration guide](MIGRATING.md): move a Beads tracker to beadroll or export it back.
- [Guarantees](docs/GUARANTEES.md): behavior, recovery, configuration, and known limits.
- [Design and review guide](docs/REVIEWING.md): architecture, format, and design decisions.
- [Review findings](docs/FINDINGS.md): findings and their resolutions.
- [Changelog](CHANGELOG.md): changes by version.

To report a problem, [open an issue](https://github.com/bpcakes/beadroll/issues)
with the command you ran, its `--json` output, `bead info --json`, and your
Git version. Check the output for private information before posting it.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, crate boundaries,
and test requirements. From the checkout root:

```sh
cargo build --package beadroll
./check.sh
```

The build produces `target/debug/bead`. The baseline requires cargo-nextest
and Python 3 in addition to the install prerequisites. Tests use temporary
stores; for manual experiments, set `BEAD_HOME` to a scratch directory and
work in a disposable repository to keep real trackers untouched.

## License

Licensed under either the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option. Contributions use the same terms
unless stated otherwise.
