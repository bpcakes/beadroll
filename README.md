# beadroll

[![rust](https://github.com/bpcakes/beadroll/actions/workflows/rust.yml/badge.svg)](https://github.com/bpcakes/beadroll/actions/workflows/rust.yml)

**An issue tracker for coding agents that works across git worktrees, clones
and machines, and keeps its state off your code branches.**

Several agents (and people) work on one repository at once, each in its own
worktree, often on more than one machine. They need one tracker: the same
issues everywhere, a claim that means *mine and nobody else's* the moment it is
accepted, and nothing to merge. beadroll gives each machine an append-only
journal of events, replays all journals into the current state, and moves them
through one dedicated git ref (`refs/beads/journal`) on the repository's own
remote. Ownership is settled online by a conditional update of that ref. No
server, no daemon, no database, no commits on your branches.

It reads and writes [Beads](https://github.com/steveyegge/beads) JSONL
(`.beads/issues.jsonl`), so a tracker kept with `br` or `bd` imports in one
command and exports back for `bv` or any Beads reader. The command is `bead`;
its verbs mirror the `br` verbs agents actually use.

One binary, `bead`, built from a Cargo workspace (`crates/`: the pure core,
git, the store, the sync protocol and the command line), for macOS and Linux
with git 2.40+. Up to 0.1.0 it was a Python package (the tag `python-0.1.0`);
0.2.0 is the same tool in Rust, reading and writing the same stores and
published trees, with the same commands, output and error codes.

## Status

Alpha, in daily use by its author: this repository tracks its own work with
it, real Beads trackers (the largest 227 issues, 1.9 MB of JSONL) have been
imported and published through their repositories, and the cross-machine
protocol is exercised by a scripted drive over four machines against GitHub.
Import, export, `ready` and `blocked` have been checked against `br` 0.5.7 on
48 real trackers and bead's export is read by `bv`; upstream `bd` has not been
tested. The on-disk and
published format is frozen at version 1 (changes are additive; see *Format
and upgrades*). Seven independent reviews have been run against the code;
every finding and its resolution is in `docs/FINDINGS.md`.

## Install

bead is built from source; there is no package to download. It needs
[rustup](https://rustup.rs), which fetches the pinned toolchain (Rust 1.99.0,
`rust-toolchain.toml`) on first use. From a checkout's root:

```sh
cargo install --locked --force --path crates/beadroll --root ~/.local
bead --version
```

Run it from the root: rustup reads `rust-toolchain.toml` from the current
directory, not from `--path`, and elsewhere it builds with the default
toolchain, which fails `rust-version`. Without a checkout, given read access to
the repository:

```sh
cargo +1.99.0 install --locked --force --config net.git-fetch-with-cli=true \
  --git ssh://git@github.com/bpcakes/beadroll.git beadroll --root ~/.local
```

cargo refuses the scp-like `git@github.com:…` form; a `--git` install never
reads the repository's toolchain file, hence `+1.99.0`; fetching through git
uses the machine's own SSH setup. Either command puts `bead` in
`~/.local/bin`, and run again it upgrades it. In a checkout, `cargo build
--package beadroll` builds the working copy as `target/debug/bead`. Start-up
is about 2 ms, and a read (`show`, `list`, `ready`) about 15 ms.

## Quick start

```sh
cd some-repo                      # any worktree or clone
bead init --import .beads/issues.jsonl   # keeps ids, comments, dependencies; publishes at once
bead                              # the one-screen overview; bead help COMMAND for one command
bead prime                        # the workflow an agent loads at the start of a session
bead onboard                      # the instruction block for a project's AGENTS.md / CLAUDE.md
bead ready --type task --json
bead ready <epic> --json            # only its subtree: its children, theirs, and so on
bead claim <id>                     # or: bead update <id> --status in_progress
bead comments add <id> "started"
bead close <id> --reason "done"
bead claims                         # who holds what, on which branch, and when each was last active
bead log <id>                       # who did what: machine, agent, thread, branch
bead export -o /tmp/x/.beads/issues.jsonl   # for bv or any Beads reader
bead info                           # where the store is, who you are, what the remote still lacks
```

Set a repository up once, from any checkout: `bead init`, with `--import` when
it comes from Beads. Nothing is committed: the tracker is found from the origin
URL, so every worktree, clone and machine of the repository shares it. On
another machine or in another clone, install `bead` and run `bead sync` there;
an `init` there joins the tracker the remote already publishes and never starts
a second one. Every command that writes publishes before it returns
(`--offline` keeps a write local); `sync` is the catch-up after `--offline` or a
remote that did not answer, and `ready`, `show`, `list` and the other reads
never touch the network.

Commands: `create`, `show`, `update`, `list`, `ready`, `blocked`, `close`,
`reopen`, `delete`, `dep add|remove|list|tree|cycles`, `comments [add]`,
`search`, `lint`, plus `init`, `import`, `export`, `sync`, `claim`, `release`,
`takeover`, `claims`, `log`, `info`, `machines`, and for agents `help`, `prime`,
`onboard`. `--json` works anywhere on the command line; `bead` alone prints the
overview.

`bead ready` is the frontier: open issues that nothing blocks and nobody
holds, by br's rule. `bead ready ID`, or `--epic ID` as in br, keeps to ID's
subtree: its children, theirs, and so on, through every parent link and
whatever the issues in between are (closed, deferred, claimed, an epic still
waiting). ID itself is not part of it, nor is an issue whose key only looks
like a child's. Readiness is still judged on the whole tracker, so a blocker
outside the subtree (of the root, say, or of a second parent) still counts.
`--parent ID` keeps to ID's direct children, here and in `list`. A scope that
names no issue is `ISSUE_NOT_FOUND`, never an empty answer, and only one scope
is taken at a time.

## What gets published

Everything bead publishes goes to `refs/beads/journal` on the coordination
remote (origin by default), and only there: never to a branch, never into a
commit of yours. Anyone who can read that repository can read the tracker. A
journal holds, for every event: the issue data you wrote (titles, descriptions,
comments), who wrote it — an opaque machine id, the agent type and its session
or thread id (`claude:<session>`, `codex:<thread>`), or the `--actor` name you
chose — the branch and commit the command ran on, and a timestamp. It never
holds hostnames, user names, paths, or the password of a remote URL (those are
dropped before anything is stored or shown; git's credential helpers are the
place for secrets). `"where": false` in `.beadroll.json` stops events from
recording the branch and commit; `bead machines --name` keeps hostnames as
local aliases only. On a public repository, the ref is public: treat issue text
as you would a public issue tracker.

## How it works

- **One journal per machine**, in append-only segment files. A machine only
  appends to its own journal; a full segment is never rewritten.
- **Every event names what its writer had seen** (UUIDv7 ids, explicit causal
  dependencies). Current state is the replay of all journals in causal order.
- **One store per repository per machine**, outside every working tree
  (`~/.local/state/beadroll/<workspace>/`, or `$BEAD_HOME`). Every worktree and
  clone resolves to it; nothing is written in a checkout.
- **Sync through one dedicated git ref** (`refs/beads/journal`) on one
  coordination remote. The pushed history holds a manifest and the journals,
  and no code.
- **Claims are persistent and settled online.** A claim is a conditional
  update of the coordination ref; it lasts until release, close or takeover.
  Ownership events live beside the journal as one immutable file each and
  count only once the remote holds them.
- **Every write is published at once.** A create, edit, comment, dependency,
  import or init reaches the remote before the command returns (about 1.7 s
  against GitHub, like a claim); `--offline` keeps it local. A remote that
  does not answer never fails a write: it is recorded, and every command says
  what the remote still lacks (`unpublished`) until a sync or an online
  command carries it. `bead sync` is the catch-up, not the only way out.
- **Identity on every event:** machine, agent type, thread id.
- **Place on every event:** the branch and commit the command ran on, so a
  claim says where the work is happening and a close whether it is merged.
- **Beads JSONL is an import and export format**, not the store.

## Claims

The journal holds sequential events; ownership is kept apart from it. An
*attempt* (a `claim`, `release` or `takeover`, plus any edits made in the same
command) is one small file under `events/<machine>/claims/`, written whole, and
such a file exists only once the remote has accepted the push that carried it.
A reader therefore sees all of an attempt or none of it.

| Action | What happens |
|---|---|
| `bead claim X` | Under the store lock: fetches the ref, checks nobody holds X, writes the claim as a *pending attempt*, and pushes it as a fast-forward of the head just fetched. Accepted: the file is moved into place and the claim is confirmed. Refused (another machine moved the ref first): the attempt is dropped and the decision is made again against the new head. No answer: the attempt stays pending, counting for nothing, until the next contact with the remote finds it there (confirmed) or pushes a *fence* commit on the same head, after which the lost push can no longer land, and drops it. Nothing in the journal is ever written or taken back for a claim. |
| `bead update X --claim --title ...` | The claim and the edits made with it form one attempt: all of it lands with the push, or none of it. The edits live in the attempt file, not in the journal. |
| the push lands but the answer is lost | The command keeps trying; when the next fetch finds its attempt on the remote, it reports the result of that attempt instead of doing the work again. Every pending attempt also carries a receipt of the command that made it, so after a crash a retry of that same command by the same actor finds its attempt accepted and reports done, rather than rebuilding the command against the state it already changed. A receipt lives until a later command of that actor *goes through* (the remote accepts it, or a local write is appended); a failed command in between leaves it alone. A recognised retry still answers for the claim as it is now (`ALREADY_CLAIMED` if someone took it over since). |
| the remote has lost objects | A ref on the remote that names objects it no longer has is `REMOTE_DAMAGED`, naming the ref and the repair (delete the ref there, then `bead sync` everywhere: every machine still has its own journal). |
| the remote refuses the push | A refusal by the remote itself (a declining hook, a read-only key, a policy) is `REMOTE_REFUSED` at once, quoting git: nothing was lost and retrying will not help. A remote that could not take the pack (maintenance running there, our own sync repository pruned under us) is retried after a fresh fetch, like a race. The outcome of a push is read from git's own per-ref status (`--porcelain`), never from text a server hook prints. A push that got no answer is `REMOTE_UNREACHABLE` with git's words, and the attempt stays pending. |
| an attempt file differs from the accepted copy | Attempt files are immutable: on sync the remote copy wins, the local one is kept beside it as `*.fork-*` and reported. A pending attempt that differs from the remote file of the same id is `ATTEMPT_FORK` (the machine identity is in use elsewhere). A file with an unreadable line is ignored whole. |
| `bead claim X --offline` | Records a provisional claim. It is listed, never exclusive, and never displaces a confirmed owner. |
| `bead release X` | Ends your claim; the issue is open again. |
| `bead takeover X --reason "..."` | Replaces the current owner and starts a new generation. Online only. |
| `bead close X` / `bead delete X` (as owner) | Closes (or tombstones) and ends exactly your claim, then publishes at once. `show` tells whether the closing commit is merged into the default branch. |
| old owner acts after a takeover | Its close or release names a claim that is no longer current, so every machine ignores it and reports it under `stale`. |

`in_progress` is reached only through a claim, so `update --status in_progress`
claims. Any other status change on an issue someone else holds is refused
(`--force` overrides), `--status closed` is the same transition as `bead close`,
`--status tombstone` is refused in favour of `bead delete` (which ends the claim),
and `ready` never offers an issue that has a confirmed owner. The owner is the
full agent thread id. Ownership never expires. `bead claims` shows each owner's
branch, last journal activity, and any of our own attempts still awaiting an
answer; there is no heartbeat. While an attempt of ours is unanswered, further
ownership commands refuse (`UNRESOLVED`) until a `sync` can settle it; plain
edits are never held up.

## How concurrent edits resolve

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

## What the journals tolerate

- An interrupted append (crash mid-write) is repaired before the next write or
  publish: the fragment is kept beside the segment as `*.torn-*`.
- A malformed event, an event of a newer format, an unknown operation, or a
  field of the wrong type is reported by `bead info` under `integrity` and
  applied by nobody; everything unrelated keeps working. One event id with two
  different payloads is set aside the same way, and nothing waits on it. A
  record without a usable id is reported wherever it was found.
- An event that does not build on its predecessor in its own machine's journal
  is reported and ignored, and its position is never used to infer what it saw.
  On sync, that condition in our own journal is `JOURNAL_FORK`: the machine
  identity is in use somewhere else. A copy of an event found in another
  machine's journal never changes where the event sits in its own.
- A takeover that names a claim its writer had not seen, or that is not a claim
  on the same issue, applies nothing and is reported.
- A ref whose manifest is not this version's format is refused untouched
  (`FORMAT_UNSUPPORTED`): an older client never rewrites a newer tracker.
- A damaged private sync repository (`sync.git` with empty or corrupt objects,
  which git reports as a local read error) is set aside as
  `sync.git.damaged-<time>` and made again from the remote and `events/`,
  once per command; `info` lists the copies kept, which hold nothing that is
  not also elsewhere. Damage again after that is `SYNC_REPO_DAMAGED`.
- git's own maintenance is off in `sync.git` (no detached `gc` racing the next
  command) and what a fetch brings stays in the pack it arrived in. bead repacks
  the repository itself, under the store lock, once loose objects or packs pile
  up (`BEAD_REPACK_LOOSE`, `BEAD_REPACK_PACKS`); `sync --json` then says
  `repacked`.
- `title`, `priority`, `issue_type` and `status` cannot be cleared; a `set`
  that tries is reported and applies nothing.

## Format and upgrades

Events carry `"v": 1` and the published manifest `"format": 1`. **Format 1 is
frozen; changes are additive.** A newer bead may add an operation or a field,
still as format 1; an older bead sets aside the events it cannot apply
(reported under `integrity`, applied by nobody), carries them through sync
untouched, and applies them once upgraded. Machines can therefore be upgraded
in any order; a mixed fleet is safe, and shows integrity notes until it is
uniform. The format number changes only for a change that would make an old
reader *misread* events; none is planned. Should one ever be needed it follows
*writers lag readers*: the new bead reads both formats but writes the old one
until every machine on the tracker has announced, in the manifest it publishes,
that it reads the new one; then it flips. Nothing ever rewrites existing events.

## What else holds

- An imported dependency on an issue that is not in the tracker yet is kept by
  key and becomes a real edge on every machine the moment that key exists, so
  importing in several batches gives the same result as one. An import is
  checked whole before anything is written: one bad record fails it.
- Several bead processes on one machine (agents in different worktrees) never
  collide inside the private `sync.git`: each fetch lands in a ref of its own.
- A publish hashes only the journal files that changed since the last one: the
  segment being appended to and new claim files, not the whole history. What
  bead remembers of each file is a cache, `sync.git/journal-memo`: a file that
  changed in any way is hashed again, and deleting the memo, or `sync.git`
  itself, is always safe.
- A command reads the store's projection, `<store>/projection`, instead of
  replaying the whole journal: the state the journal replays into, kept with
  the identity of every journal file it was made from. Records appended since
  are applied after it, as replay would apply them; anything else (a file
  changed another way, events written concurrently, a takeover) has the
  journal replayed, and the projection written again. It is a cache, never a
  source: deleting it is always safe, and a damaged one, or one another build
  of bead wrote, is never read.
- Each issue's texts (descriptions, designs, acceptance criteria, notes and
  comments) are kept beside the projection, in `<store>/projection-texts`:
  `list`, `ready`, a write and a sync never read them, `show` reads only its
  issue's, and `export`, `search` and `lint` read them all. It is a cache too:
  texts that cannot be read as written have the journal replayed.
- There is no history boundary: work written offline merges on the next sync
  however long ago it was written, after days or months away, and nothing is
  refused for its age. Should bead one day publish checkpoints, so that a new
  machine fetches less history, they will keep what such work needs to merge
  (`docs/REVIEWING.md` §2).
- When the publish of an owner's close could not go out, the `--json` answer
  says so (`"unpublished": "recorded locally, not yet published (…)"`) and a
  retry of the close publishes as the first run would have.
- Human output never hands the terminal a control character: titles, comments
  and names from journals or imports are shown with `\xNN` escapes. JSON output
  and stored data are untouched. Output is UTF-8 whatever the locale.
- A password in a remote URL is dropped (with a note) and never stored, shown
  or used, whatever its shape (`https://TOKEN@host`, `https://TOKEN:x@host`,
  `transport::https://…` included), and only user-info in the authority counts:
  an `@` in a path or query is left alone; git's credential helpers are the
  place for secrets. The store is private by its own modes whatever your umask
  (0700 directories, 0600 files, `sync.git` created `--shared=0600`); files
  written into a checkout (`.beadroll.json`, `export -o`) follow your umask like
  any other file there. `BEAD_HOME` must be absolute (`~` is expanded): the
  store never lives in a checkout.
- An event nests at most 100 levels deep, so every supported Python can read
  what any machine wrote; an import deeper than that fails whole, as do a
  repeated id, a self-dependency, malformed comments, a record whose
  `created_at` is null and text holding a lone surrogate. Records end at `\n`
  only, so bead's own export (with U+2028 and friends in text) imports again.
- Every bead reads a journal line the same way, whichever Python (or binary) it
  runs on: as UTF-8, never in an encoding guessed from its first bytes; at most
  512 levels deep; with NaN, Infinity and numbers beyond a float refused. A line
  that fails is reported under `integrity` and skipped, never guessed at, and so
  is an event that would have made replay or a query fail.
- A status bead does not set itself (`pinned`, `hooked`, a custom one; Beads
  tools keep these as they are) is imported and exported verbatim; such an
  issue is neither ready nor done. `bead update --status` takes only bead's own
  vocabulary. An imported record without `created_by` stays unattributed; the
  importer is not its author.
- An imported dependency on a key that is not an issue yet is listed by `show`
  and `dep list` as `unresolved_dependencies` ("not an issue yet"), exported
  under the record's own id, and stamped like any other dependency when it
  binds. A new or renamed issue never takes a key such a dependency is waiting
  for.
- A closed stdout or stderr, a terminal that hung up (EIO) or a descriptor
  closed at exec never produce a traceback: a committed command exits 0, a
  failed one keeps its status. Help is plain text, never coloured.
- An owner's close that a takeover superseded says so: `"superseded"` on the
  record and a `!` line, exit 0 (the decision in `docs/REVIEWING.md` §2 stands).
- Text comes from files and stdin as UTF-8 whatever the locale; anything else
  is `VALIDATION`. A read-only store, a full volume or a missing file is an
  `IO` error, never a traceback, and never a half-written journal. A fetch
  that gives no answer is killed with the helpers it started; a push that
  gives no answer is abandoned on our side and left to land or fail on the
  remote's, which is exactly what the pending-attempt protocol settles next.
- An imported dependency on a key that is not an issue yet can be taken back
  (`dep remove A KEY`, or re-importing the record without it) and stays taken
  back when the key arrives.
- A foreign journal file that differs from the published copy is replaced by
  the published copy, ours is kept beside it as `*.fork-*`, and `sync` reports
  it. Our own file in that state is `JOURNAL_FORK`.

## Configuration

The golden path needs none. The workspace is derived from the origin URL (where
`git push origin` goes: the first `pushurl`, else the first `url`, read as git
reads it, so `host:path`, ssh aliases and `gh:` shortcuts are remotes, not
paths; `git@host:org/repo` and `https://host/org/repo` are one workspace) and
the coordination remote is origin, looked up each time, so nothing is
committed and every checkout finds the same tracker. Before `bead init` starts
a tracker it asks the remote: one already published there is joined instead,
and adopted when it was published under another id (below).

A tracked `.beadroll.json` is the exception: for a coordination remote other
than origin, or for `"where": false`. `bead init --tracked [--remote
NAME_OR_URL]` writes one with an explicit workspace id (the tracker's the
checkout already has or the remote publishes, else a new one; a URL-derived
tracker stays URL-derived, since its id would stop matching when the URL
changed) and optionally a different coordination remote. Origin's own URL is stored as `origin`, so the file follows
origin when it moves; a remote given by name, here or in `--remote` and
`$BEAD_REMOTE`, means the URL it is configured with for `git push` (its first
push URL, else its first URL, `insteadOf` left to git), as origin itself does,
and a store remembers it by that name, looked up again by every command.
Commit the file on the default branch at once: a checkout whose tree predates
it reads the committed copy (`HEAD`, then `origin/HEAD`), but a file that is
not committed, or not on the default branch, reaches no other checkout, and
`bead info` and `bead prime` warn about it until it does. Running it again
keeps the keys it does not manage. `--remote` and `$BEAD_REMOTE` override it per
call; a tracked remote that differs from the one a store already uses is
refused until named deliberately, and a tracked id that is another repository's
derived id is refused outright: a cloned repository cannot attach you to someone
else's store.

bead's own git runs in a private bare repository with the checkout's transport
settings carried over (`http.*.extraheader`, credential helpers,
`core.sshCommand`, `includeIf` includes), the project's object format, no
client hooks, and the remote string always passed after `--`.
`"where": false` in `.beadroll.json` stops events from recording the branch and
commit; outside a checkout nothing is recorded anyway.

Agent type and thread come from the environment: `CLAUDE_CODE_SESSION_ID`
for Claude Code and `CODEX_THREAD_ID` for Codex (both confirmed to reach shell
commands). When one agent is started from inside the other's session both are
present, and `bead` picks the nearest agent in the process tree. `BEAD_AGENT`,
`BEAD_THREAD` and `--actor NAME` override them. The machine is an opaque id; its
hostname is a local alias (`bead machines --name ID=ALIAS`), never in events.

## Moving a repository

A new origin URL (a rename, another owner, another host) gives the URL-derived
workspace a new id. Each machine, once its checkout's origin is the new URL,
finds no tracker until its first `bead sync` (or `bead init`), which adopts the
tracker published under the old id and remembers the mapping (`moved.json` in
`BEAD_HOME`). That needs the move to carry the tracker's ref,
`refs/beads/journal`: a rename or a transfer on a forge keeps every ref, and
`git push --mirror` copies them. An import that copies only branches and tags
leaves it behind: `bead sync` then says none is published and names this
machine's tracker of the same name if it has one; `BEAD_WORKSPACE=ID bead sync
--remote origin` on a machine that has it, its origin moved, publishes it at the new
URL, and every other machine's next `sync` adopts it. Until a machine's origin moves it keeps using the old
URL, which a forge's rename redirects.

A tracked `.beadroll.json` keeps its id through any move. With its remote left
to origin, or named `origin`, the first `sync` after the move publishes the
tracker at the new URL if it is not there yet. A tracked remote stored as a URL
keeps pointing at the old place: `bead init --tracked --remote origin` rewrites
it, and the file is committed again.

## Tests and reviews

```sh
./check.sh            # every test, the doctests and the infra/ drivers' tests (about 4 minutes)
./check.sh --stress   # also the randomised multi-machine models
./check.sh --real     # also import/export of this machine's real tracker exports, with br and bv
```

`./check.sh` runs `cargo xtask check` with the options given: one line per step, a scratch
`BEAD_HOME` for every step, and a non-zero exit when any step fails, the later steps still
running. It needs rustup and cargo-nextest. Every selected check must pass. `--real` exits
non-zero if a command fails or exported records differ, and keeps the failing round-trip report
instead of only its last line. Its imports run offline in the scratch store.

The tests are the Cargo workspace's, one integration-test binary per crate, organised by subject
as the Python suite they were ported from was, test by test: `replay` (causality, registers,
quarantine, validation, cycles; in beadroll-core, in-process and fast), `protocol` (claims,
attempts, fences, lost acknowledgements, sync races), `storage` (segments, torn tails, identity,
readers against writers), `commands` (every verb end to end, identity detection, branch
tracking), `compat` (Beads import/export, real exports, `br`/`bv`), `hardening` (the base under
the protocol: git environment and remotes, URLs and credentials, store files and modes, output
streams, imports), `stress` (randomised models, run by `--stress`), and `sweep` (the ownership
protocol enumerated: three machines, nine operations at seven fault boundaries, every ordered
pair of follow-ups, each schedule showing that its fault fired; a thirteen-schedule slice by
default, including retries of interrupted owner closes and deletes and a sync stopped at its
push; `BEAD_SWEEP=full` for all 660, with no failure exemptions). `golden` compares
every help page and the reading commands' output with Python's, captured before the cutover, and
`parity` holds what the mixed Python and Rust fleets checked, among it receipts byte for byte as
Python wrote them. `crates/beadroll-test-support` holds the sandbox: every test builds its own
repository, bare remotes and machine homes in a temporary directory, each machine with a scratch
`BEAD_HOME`, so the default store is never touched.

Every test that runs bead runs the one Cargo builds, unless `BEAD_UNDER_TEST` names another by its
absolute path. Until 0.2.0 bead was a Python package, whose suite the Rust tests port; the
port's tests passed against both, and the last Python is tagged `python-0.1.0`
(`docs/RUST-PORT.md` is the plan the port followed).

Seven independent reviews have been run against the code. Their reports are kept
verbatim under `docs/reviews/`; `docs/FINDINGS.md` is the ledger of every finding, its status,
how it was resolved and which tests cover it. `docs/REVIEWING.md` is the brief for the
next one: the map of the design, the decisions that are not findings, the rules,
the checklist and the deliverables. `infra/` holds the maintainer's harness against
real machines and a throwaway GitHub repository, stdlib Python that drives any `bead`:
`drive.py` (cross-machine scenarios over ssh), `soak.py` (random agent-like traffic),
`roundtrip.py` (import/export over real Beads exports), `selected.py` (which bead each runs, and
the record of it before any test data is written), their tests beside them, and the synthetic
`seed.jsonl`.

## Design decisions that changed while building

| First design | Now | Why |
|---|---|---|
| Concurrent values need an explicit resolution operation | Flagged with every alternative; any later write that saw them settles it | Same information, no extra protocol for agents to learn |
| Fallback display is the smallest event id | The newest event id | Matches what people expect to see |
| Conditional push with `--force-with-lease` | Plain fast-forward push | Already conditional, and cannot truncate history |
| A corrupt duplicate holds back its dependents | Only the event itself is set aside | With a project-wide frontier, everything later depends on it |
| Heartbeat events | Last journal activity per owner | No extra traffic or permanent noise |
| Structured acceptance items | One text field | Round-trips through Beads JSONL |
| Checkpoints and pruning | Segment rotation only | The rest is not needed at this size |
| Owner's close is a conditional transition | Local write, published at once | The stale-claim rule makes a late one harmless |
| Ownership in the journal | Ownership files beside the journal, pending until the remote holds them | A refused or unanswered claim then needs no compensating record, and the journal stays append-only |

## Known limits

- The owner of a claim is the agent thread. A later session is a different
  owner and has to `takeover` (or use a stable `--actor`) to continue the work.
- The store lock is held while a `claim` fetches and pushes, and while a `sync`
  pushes, so other local writes wait one or two network round trips (a second
  or two against GitHub). A routine `sync` fetches before taking the lock.
- A `--status` change made with `update` follows the same ownership rules and
  publishes an owner's close immediately, like `bead close`; `--parent` refuses a
  loop, like `dep add`.
- Exported Beads comment ids: an imported comment keeps its id; a comment made
  here exports an integer derived from its event id (48 bits, from 2^47), so
  every machine and every later export agrees. Two such comments sharing a
  number is a one-in-10^14 event; if it happens, the tie is broken by the ids
  alone and one of them moves. Beads readers accept these ids; `br` would not
  have minted them.
- `dep cycles` lists every elementary cycle, up to 200.
- What each event had seen is kept as one clock per event over chains: every
  machine's journal is a chain, and the claims, releases and takeovers of
  attempt files join chains of their own, about one for each machine that
  writes them. A clock holds the furthest position seen on each chain, so
  whether one event saw another is one comparison, and memory grows with events
  × chains rather than with events × ownership events. Replaying 200,000
  events, about 55% of them in attempt files, takes 0.29 s and holds at most
  133 MB on the heap (a 215 MB resident peak), 116 MB of it the loaded journal
  (`docs/RUST-PORT.md`, Appendix A; Python's bitsets took 11.8 s and 1.7 GB).
- The branch recorded on an event is whatever `HEAD` was in the directory the
  command ran in. It is a trace, not a lock: a claim does not pin a branch.
- A command reads the store's projection while it is current, and replays the
  journal when it is not (after another build of bead, a change it could not
  apply, or the projection removed). At 200,000 events (the largest public
  Beads trackers import as at most 29,000), a read takes 73–87 ms with it and
  1.06 s without; a sync that merges ten events another machine wrote, 0.31 s and
  1.31 s; a publishing sync, 0.38 s and 1.34 s (`docs/RUST-PORT.md`, Appendix
  A). With it, a read costs what the state does: 18 ms for a thousand issues,
  whether 50,000 events made them or 200,000. A real tracker's state is mostly
  its texts, which only the commands that print them read: on three of the
  largest public Beads trackers (21–47 MB, up to 13,394 issues) a `list` takes
  36–75 ms and a published write 0.21–0.35 s, while `export`, `search` and
  `lint` take what the texts do, and importing the largest held 0.87 GB
  (roll-lqjj.9). A merge that brings events another
  machine wrote concurrently replays the journal, as about one merge in ten
  does where two machines write (roll-lqjj.8). A replay holds at most 107 MB
  on the heap at 200,000 events (events the size of this repository's own take
  more than three times that). A store heavy in claims pays for listing its
  attempt files on every command: a read takes 0.56 s at 92,000 of them, which
  only a checkpoint would change. A fetch or push that gets no answer fails
  after `BEAD_NET_TIMEOUT` seconds (default 45) instead of hanging.
- Publishing is serialised on one ref. Under heavy contention (four machines
  syncing continuously) claims took 9 s median and 27 s at p90, and 3 of 841
  commands gave up after eight attempts. Writes wait too: four machines each
  publishing every write continuously against a remote taking 0.3 s a fetch
  or a push, a write took 1.5 s at the median and 23 s at the longest, and
  every machine converged with nothing left unpublished (roll-lqjj.7: a sync
  makes 24 rounds before it gives up, its waits doubling to at most 2 s).
- A renumbered child key changes under the machine that created it. Events
  refer to issues by an immutable id, so nothing is lost, but a key written
  into a commit message before the sync now names the other issue.
- A repository with neither a remote nor a `.beadroll.json` gets a machine-local
  tracker that cannot sync.
- A Beads export that holds memory records (`{"_type": "memory", ...}`, which
  recent Beads versions keep in `issues.jsonl` beside the issues) is refused
  whole by `import`, at the first of them (roll-dih8.16). Export writes every
  dependency with br's six members, so one written without `thread_id` comes
  back with `"thread_id": ""`.
- Not implemented: `stats`, `count`, `epic status`, `graph`, saved queries,
  templates, external-project dependencies, `bv`-style ranking.

## Relation to Beads

beadroll is not a fork of Beads and does not use its database. It shares the
JSONL record format (`import`, `export`), the issue model (ids, parent-child
keys, dependencies, comments, labels, statuses) and the verbs agents use, so a
project can move from `br`/`bd` to `bead` and back with its history intact
(`bead export` writes what `br sync --import-only` and `bv` read);
[MIGRATING.md](MIGRATING.md) is the procedure, both ways. What it
adds is the multi-machine, multi-worktree model: journals instead of a database,
one ref instead of commits on branches, claims settled online. What it leaves
out is listed under *Known limits*. A leftover `.beads/` directory is history:
nothing in bead reads or writes it after the import.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md): the setup, the one rule about stores,
the layout and how a change is made and tested. `AGENTS.md` is the same for an
agent working in this repository; `docs/REVIEWING.md` is the brief for an
independent review. Unless you say otherwise, a contribution is licensed as the
project is.

## Licence

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
the [MIT license](LICENSE-MIT), at your option.
