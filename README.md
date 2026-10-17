# beadroll

[![rust](https://github.com/bpcakes/beadroll/actions/workflows/rust.yml/badge.svg)](https://github.com/bpcakes/beadroll/actions/workflows/rust.yml)

**An issue tracker for coding agents: one tracker for every worktree, clone
and machine of a repository, claims that are exclusive, and nothing committed
on your branches. Reads and writes Beads JSONL.**

Several agents, and the people directing them, work on one repository at once,
each in its own worktree, often on more than one machine. A tracker kept in a
file on a code branch gives each of them a different copy: a worktree on a
feature branch does not see what `main` closed, two agents take the same issue
because a claim is only a status field, and every sync is tracker commits
merged into code history. beadroll gives them one tracker:

- **The same issues everywhere.** Every worktree, clone and machine of the
  repository resolves to the one tracker, which lives outside the working tree.
- **A claim that is exclusive.** `bead claim` is accepted by the remote or
  refused. Once accepted, nobody else's claim on that issue goes through until
  release, close or takeover, and `bead ready` never offers it.
- **Nothing on your branches.** The tracker travels through one dedicated git
  ref, `refs/beads/journal`, on the repository's own remote. No commits, no
  merges, no server, no daemon, no database.

With one machine, one worktree and no agents, Beads serves you well. beadroll
earns its place at the second worktree or the second machine.

## Coming from Beads

beadroll is not a fork of [Beads](https://github.com/steveyegge/beads) and
does not use its database. It keeps the JSONL record format, the issue model
(ids, parent-child keys, dependencies, comments, labels, statuses) and the
verbs agents use, so a tracker kept with `br` or `bd` imports in one command
and exports back for `bv` or any Beads reader, history intact.
[MIGRATING.md](MIGRATING.md) is the procedure, both ways.

| | Beads (`br`, `bd`) | beadroll (`bead`) |
|---|---|---|
| Where the tracker lives | `.beads/issues.jsonl` on every branch, and a local database | journals outside the checkout, published to one git ref |
| What a worktree sees | its branch's copy | the one tracker |
| `in_progress` | a status field anyone sets | a claim the remote accepted, held by one agent thread |
| Syncing | tracker commits, merged like code | append-only journals: nothing to merge, ever |

Not implemented: `stats`, `count`, `epic status`, `graph`, saved queries,
templates, external-project dependencies and `bv`-style ranking. A leftover
`.beads/` directory is history: nothing in bead reads or writes it after the
import.

## Status

Alpha, in daily use by its author. What has been exercised:

- This repository tracks its own work with it, across several machines.
- Real Beads trackers, the largest 227 issues and 1.9 MB of JSONL, have been
  imported and published through their repositories. Import, export, `ready`
  and `blocked` agree with `br` 0.5.7 on 48 real trackers, and `bv` reads
  bead's export. Upstream `bd` has not been tested.
- The cross-machine protocol is driven by script over four machines against
  GitHub, and enumerated in the tests: three machines, nine operations at
  seven fault boundaries, every ordered pair of follow-ups.
- Twelve independent reviews have been run against the code. Every finding
  and how it was closed is in [docs/FINDINGS.md](docs/FINDINGS.md); the
  reports are kept verbatim under `docs/reviews/`.

The on-disk and published format is frozen at version 1 and changes only
additively, so machines upgrade in any order (*How it works*).

## Install

macOS or Linux, git 2.40+, and [rustup](https://rustup.rs), which fetches the
pinned toolchain (Rust 1.99.0, `rust-toolchain.toml`) on first use. bead is
built from source; there is no package to download. From a checkout's root:

```sh
cargo install --locked --force --path crates/beadroll --root ~/.local
bead --version
```

That puts `bead` in `~/.local/bin`, and run again it upgrades it. Run it from
the root: rustup reads `rust-toolchain.toml` from the current directory, and
elsewhere the build fails `rust-version`. Without a checkout, given read access
to the repository:

```sh
cargo +1.99.0 install --locked --force --config net.git-fetch-with-cli=true \
  --git ssh://git@github.com/bpcakes/beadroll.git beadroll --root ~/.local
```

`+1.99.0` because a `--git` install never reads the toolchain file; `ssh://`
because cargo refuses the `git@github.com:…` form; fetching through git uses
your own SSH setup. In a checkout, `cargo build --package beadroll` builds the
working copy as `target/debug/bead`. Start-up is about 2 ms, and a read
(`show`, `list`, `ready`) about 15 ms.

## A session

Set a repository up once, from any checkout. `--import` brings a Beads tracker
in, and `--dry-run` first says what it would bring and writes nothing:

```
$ cd app                                  # any worktree or clone
$ bead init --prefix app                  # or: bead init --import .beads/issues.jsonl
Tracker 'app' at ~/.local/state/beadroll/c9acf05de07c7939
Published to git@github.com:acme/app.git (refs/beads/journal)
```

Nothing was committed. The tracker is found from the origin URL, so every
worktree, clone and machine of the repository shares it; on another machine,
`bead sync` fetches it, and an `init` there joins it rather than starting a
second one. Then the loop an agent runs:

```
$ bead create "Rate limiter drops the first request after a restart" -t bug -p 1
✓ Created app-zs43: Rate limiter drops the first request after a restart
$ bead ready
○ app-zs43 [P1] [bug] - Rate limiter drops the first request after a restart (main)
$ bead claim app-zs43
Claimed app-zs43: Rate limiter drops the first request after a restart (confirmed)
$ bead comments add app-zs43 "the counter is reset before the first tick"
Comment added to app-zs43
$ bead close app-zs43 -r "counter now survives a restart"
✓ Closed app-zs43: Rate limiter drops the first request after a restart
```

While it is held, from any other worktree, machine or session:

```
$ bead claim app-zs43
Error: app-zs43 is claimed by claude:4f1c2a9e@3f761b2d (generation 1, since 2026-10-13T19:15:45.853Z)
$ bead claims
app-zs43  confirmed   claude:4f1c2a9e@3f761b2d  since 2026-10-13T19:15  gen 1  last activity 2026-10-13T19:15  on main
```

Afterwards, anywhere:

```
$ bead show app-zs43
✓ app-zs43 · Rate limiter drops the first request after a restart   [P1 · CLOSED]
  closed on main at d80871914bf5 · merged into the default branch
  type bug · created 2026-10-13 by human@3f761b2d · assignee claude:4f1c2a9e@3f761b2d
  close_reason: counter now survives a restart
  [2026-10-13T19:15] claude:4f1c2a9e@3f761b2d: the counter is reset before the first tick
$ bead log app-zs43
2026-10-13T19:15:45  laptop     human  -         create       app-zs43  (main)
2026-10-13T19:15:45  laptop     claude 4f1c2a9e  claim        app-zs43  (main)
2026-10-13T19:15:45  laptop     claude 4f1c2a9e  comment      app-zs43  (main)
2026-10-13T19:15:46  laptop     claude 4f1c2a9e  close        app-zs43  (main)
```

Every command that writes publishes before it returns. `--offline` keeps a
write local, and a remote that does not answer never fails one: it is
recorded, every command says what the remote still lacks, and `bead sync`
carries it. `ready`, `show`, `list` and the other reads never touch the
network.

`bead recap`, from any directory, shows what is new in every tracker on the
machine since you last looked: one line per event as `bead log` prints it, with
its issue's title, who wrote it, on which machine and branch (`--format issues`
gives one line per issue, `--format full` each event's thread and commit). It
never touches the network, so sync a repository first to see its other
machines' latest work. The first recap shows the last 24 hours; after that,
each one on a terminal marks what it showed as seen, while a pipe, a file or
`--json`, an agent's run, only looks unless given `--advance`.

The commands: `create`, `show`, `update`, `list`, `ready`, `blocked`, `close`,
`reopen`, `delete`, `dep add|remove|list|tree|cycles`, `comments [add]`,
`search`, `lint`; `init`, `import`, `export`, `sync`, `claim`, `release`,
`takeover`, `claims`, `log`, `recap`, `info`, `machines`; and for agents `help`,
`prime`, `onboard`. `bead` alone prints the one-screen overview and `bead help
COMMAND` one command's page. `bead ready EPIC` keeps to an epic's subtree,
`bead close --recursive EPIC` closes one, children first, in one write, and a
command that would change nothing says so instead of writing. The rest of the
command line, colour included, is in [docs/GUARANTEES.md](docs/GUARANTEES.md).

## For agents

bead is the tracker the agents run themselves; nothing else is set up.

1. `bead onboard >> AGENTS.md` (or `CLAUDE.md`) appends the block that tells an
   agent how this repository tracks its work; fill in the project specifics at
   its end.
2. An agent starts every session with `bead prime`: the workflow on one screen,
   who it is, what it holds, and every error code with what to do about it.
3. Who acts is read from the harness, `CLAUDE_CODE_SESSION_ID` for Claude Code
   and `CODEX_THREAD_ID` for Codex, so a claim belongs to the agent thread that
   made it, and a new session continues its own earlier work with `bead
   takeover ID -r "same task, new session"`. `--actor NAME` or `BEAD_ACTOR`
   names a person, or a stable identity, instead.

`--json` works anywhere on the command line, and an error is structured, with
a code an agent can act on:

```
$ bead claim app-zs43 --json
{"error": {"code": "ALREADY_CLAIMED", "message": "app-zs43 is claimed by claude:4f1c2a9e@3f761b2d (generation 1, since 2026-10-13T19:15:45.853Z)"}}
```

Several bead processes on one machine, agents in different worktrees, run at
once without colliding. Output into a pipe or a file is plain text, and what
an issue says is escaped before anything is coloured, so a title never carries
a control code into an agent's transcript.

## How it works

- **One journal per machine**, in append-only segment files. A machine only
  appends to its own journal; a full segment is never rewritten.
- **Every event names what its writer had seen** (UUIDv7 ids, explicit causal
  dependencies). Current state is the replay of all journals in causal order,
  so there is nothing to merge: two edits are ordered or concurrent, and a
  concurrent pair is kept whole and reported (below).
- **One store per repository per machine**, outside every working tree
  (`~/.local/state/beadroll/<workspace>/`, or `$BEAD_HOME`). Every worktree and
  clone resolves to it; nothing is written in a checkout.
- **Sync through one dedicated git ref** (`refs/beads/journal`) on one
  coordination remote, origin by default. The pushed history holds a manifest
  and the journals, and no code.
- **Claims are settled online.** A claim is a conditional update of that ref:
  fetched, checked against who holds what, pushed as a fast-forward. Accepted,
  it is exclusive until release, close or takeover; refused, the decision is
  made again against the new head; unanswered, it counts for nothing until the
  next contact with the remote settles it. A claim never expires and there is
  no heartbeat: `bead claims` shows each owner's branch and last activity.
- **Every write is published at once**, about 1.7 s against GitHub, like a
  claim. `--offline` keeps it local; a remote that does not answer leaves it
  recorded and reported as unpublished until `bead sync` or the next online
  command carries it.
- **Identity and place on every event**: machine, agent type and thread id,
  and the branch and commit the command ran on, so a claim says where the work
  is happening and a close whether it is merged.
- **Format 1 is frozen; changes are additive.** An older bead sets aside what
  it cannot apply, reports it under `integrity`, carries it through sync
  untouched and applies it once upgraded, so a mixed fleet is safe and machines
  upgrade in any order.
- **Beads JSONL is an import and export format**, not the store.

What the protocol does at every boundary (a lost acknowledgement, a crash in
the middle of a command, a remote that lost objects, a damaged local
repository, a torn write) is in [docs/GUARANTEES.md](docs/GUARANTEES.md).

## When two machines edit the same thing

| Two machines, before syncing, both… | Result on every machine |
|---|---|
| edit different fields of one issue | both edits kept |
| edit the same field | both values kept and listed under `conflicts`; the newer one is shown; any later write that has seen both settles it |
| set different statuses | as above, and until settled the issue is not ready, cannot be claimed, and does not unblock others |
| add comments | all kept |
| add and remove a label or dependency | a removal removes only the additions its writer had seen |
| create a child of the same parent | the later one is renumbered (`x.1` → `x.2`) and `sync` reports it |
| add dependencies that form a loop | kept, reported by `dep cycles`, never `ready` |

An edit made after seeing another is never a conflict, whatever the clocks say.
A `close`, `reopen` or `delete` is judged on what this machine has seen: repeated
on an issue already in that status, with no claim to end and its status
undisputed, it writes nothing, so a change another machine made meanwhile stands;
a disputed status still takes the repeat that settles it.

## What gets published

Everything bead publishes goes to `refs/beads/journal` on the coordination
remote (origin by default), and only there: never to a branch, never into a
commit of yours. Anyone who can read that repository can read the tracker. A
journal holds, for every event: the issue data you wrote (titles, descriptions,
comments), who wrote it (an opaque machine id, the agent type and its session
or thread id, `claude:<session>` or `codex:<thread>`, or the `--actor` name you
chose), the branch and commit the command ran on, and a timestamp. It never
holds hostnames, user names, paths, or the password of a remote URL (those are
dropped before anything is stored or shown; git's credential helpers are the
place for secrets). `"where": false` in `.beadroll.json` stops events from
recording the branch and commit; `bead machines --name` keeps hostnames as
local aliases only. On a public repository, the ref is public: treat issue text
as you would a public issue tracker.

## Configuration

The golden path needs none. The workspace is derived from the origin URL
(`git@host:org/repo` and `https://host/org/repo` are one workspace) and the
coordination remote is origin, looked up each time, so nothing is committed
and every checkout finds the same tracker. A tracked `.beadroll.json` is the
exception, for a coordination remote other than origin or for `"where":
false`: `bead init --tracked [--remote NAME_OR_URL]` writes it, and it is
committed on the default branch. What it holds, how a remote is named, and
what happens when a repository moves to a new URL are in
[docs/GUARANTEES.md](docs/GUARANTEES.md).

bead's own git runs in a private bare repository with the checkout's transport
settings carried over (credential helpers, `core.sshCommand`,
`http.*.extraheader`, `includeIf` includes), so whatever lets you push lets
bead publish.

The variables bead reads, every one optional:

| Variable | Meaning |
|---|---|
| `BEAD_HOME` | The store's root, absolute after `~` expansion; default `~/.local/state/beadroll` |
| `BEAD_WORKSPACE` | The workspace id, overriding the derived one and its check (*Moving a repository* in `docs/GUARANTEES.md`) |
| `BEAD_REMOTE` | The coordination remote for this call: a remote's name, a URL or a path |
| `BEAD_AGENT`, `BEAD_THREAD`, `BEAD_ACTOR` | Who is acting, over what the harness says; `--actor` wins over all of them |
| `CLAUDE_CODE_SESSION_ID`, `CLAUDECODE`, `CODEX_THREAD_ID`, `CODEX_SESSION_ID` | The harness and its thread (*For agents*) |
| `BEAD_NET_TIMEOUT` | Seconds, a decimal, that one fetch or push may take; default 45 |
| `BEAD_ROTATE_BYTES` | The size at which this machine's journal starts a new segment; default 262,144 |
| `BEAD_REPACK_LOOSE`, `BEAD_REPACK_PACKS` | `sync.git` is repacked beyond this many loose objects or packs; defaults 1,000 and 40 |
| `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `TERM` | Colour on a terminal (*The command line* in `docs/GUARANTEES.md`) |
| `COLUMNS` | The width bead's own texts wrap at, else the terminal's, at most 100 |

A number bead cannot read is a `CONFIG` error, even for `--version`. The tests'
and the `infra/` drivers' own variables are in `CONTRIBUTING.md`.

## Known limits

- The owner of a claim is the agent thread. A later session is a different
  owner and has to `takeover` (or use a stable `--actor`) to continue the work.
- Publishing is serialised on one ref, and the store lock is held while a
  claim or a sync pushes, so other local writes wait a round trip or two.
  Under heavy contention (four machines syncing continuously) claims took 9 s
  at the median and 27 s at p90, and a write up to 23 s; every machine
  converged with nothing left unpublished.
- A read costs what the state does, tens of milliseconds for a thousand
  issues; a published write a few hundred milliseconds plus the remote's round
  trip. A merge of events another machine wrote concurrently replays the
  journal, 1 to 2 s at 200,000 events, which is more than the largest public
  Beads trackers hold.
- A renumbered child key (`x.1` → `x.2`, after two machines created children of
  one parent offline) changes under the machine that created it; a key written
  into a commit message before the sync names the other issue.
- A repository with neither a remote nor a `.beadroll.json` gets a
  machine-local tracker that cannot sync.
- A Beads export that holds memory records (`{"_type": "memory", ...}`) is
  refused whole by `import`. Comments made in bead export with integer ids from
  a range Beads never mints.

Every limit, with its measurement, is under *Known limits* in
[docs/GUARANTEES.md](docs/GUARANTEES.md).

## Reading on

- [MIGRATING.md](MIGRATING.md): moving a Beads tracker to bead, and back.
- [docs/GUARANTEES.md](docs/GUARANTEES.md): every guarantee bead keeps: the
  claims protocol at each boundary, what the journals tolerate, the format,
  configuration in full, moving a repository, and every limit measured.
- [docs/REVIEWING.md](docs/REVIEWING.md): the map of the design and the
  decisions that are not findings; [docs/FINDINGS.md](docs/FINDINGS.md): every
  review finding and how it was closed.
- [CHANGELOG.md](CHANGELOG.md): what changed in each version.
- [CONTRIBUTING.md](CONTRIBUTING.md): the setup, the one rule about stores,
  the layout, the tests and how a change is made. `AGENTS.md` is the same for
  an agent working in this repository. Unless you say otherwise, a
  contribution is licensed as the project is.

## Licence

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
the [MIT license](LICENSE-MIT), at your option.
