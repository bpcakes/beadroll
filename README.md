# beadroll

A Beads-compatible issue tracker whose state does not live on code branches.
One Python file, standard library only (Python 3.12+, git 2.40+).

It exists to test a design before building it anywhere permanent.

## Design under test

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
- **Identity on every event:** machine, agent type, thread id.
- **Place on every event:** the branch and commit the command ran on, so a
  claim says where the work is happening and a close whether it is merged.
- **Beads JSONL is an import and export format**, not the store.

## Try it

```sh
cd some-repo                      # any worktree or clone
bead init --import .beads/issues.jsonl   # keeps ids, comments, dependencies
bead                              # the one-screen overview; bead help COMMAND for one command
bead prime                        # the workflow an agent loads at the start of a session
bead onboard                      # a snippet for CLAUDE.md / AGENTS.md
bead ready --type task --json
bead claim <id>                     # or: bead update <id> --status in_progress
bead comments add <id> "started"
bead close <id> --reason "done"
bead claims                         # who holds what, on which branch, and when each was last active
bead log <id>                       # who did what: machine, agent, thread, branch
bead export -o /tmp/x/.beads/issues.jsonl   # for bv or any Beads reader
bead info                           # where the store is and who you are
```

On a second machine: copy `bead`, then `bead sync` inside a clone of the same
remote. `sync`, `claim`, `release` and `takeover` contact the remote, as does
closing an issue you hold; every other command is local.

Commands mirror the `br` verbs and flags that are actually used: `create`,
`show`, `update`, `list`, `ready`, `blocked`, `close`, `reopen`, `delete`,
`dep add|remove|list|tree|cycles`, `comments [add]`, `search`, `lint`, plus
`init`, `import`, `export`, `sync`, `claim`, `release`, `takeover`, `claims`,
`log`, `info`, `machines`, and for agents `help`, `prime`, `onboard`. `--json`
works anywhere on the command line; `bead` alone prints the overview.

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
- `title`, `priority`, `issue_type` and `status` cannot be cleared; a `set`
  that tries is reported and applies nothing.
- An imported dependency on an issue that is not in the tracker yet is kept by
  key and becomes a real edge on every machine the moment that key exists, so
  importing in several batches gives the same result as one. An import is
  checked whole before anything is written: one bad record fails it.
- Several bead processes on one machine (agents in different worktrees) never
  collide inside the private `sync.git`: each fetch lands in a ref of its own.
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
  repeated id, a self-dependency and malformed comments. Records end at `\n`
  only, so bead's own export (with U+2028 and friends in text) imports again.
- An imported dependency on a key that is not an issue yet is listed by `show`
  and `dep list` as `unresolved_dependencies` ("not an issue yet"), exported
  under the record's own id, and stamped like any other dependency when it
  binds. A new or renamed issue never takes a key such a dependency is waiting
  for.
- A closed stdout or stderr, a terminal that hung up (EIO) or a descriptor
  closed at exec never produce a traceback: a committed command exits 0, a
  failed one keeps its status. Help is plain text on every Python.
- An owner's close that a takeover superseded says so: `"superseded"` on the
  record and a `!` line, exit 0 (the decision in `REVIEWING.md` §2 stands).
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

Without configuration the workspace is derived from the origin URL (where
`git push origin` goes: the first `pushurl`, else the first `url`, read as git
reads it, so `host:path`, ssh aliases and `gh:` shortcuts are remotes, not
paths) and the coordination remote is origin, looked up each time. `bead init
--tracked [--remote NAME_OR_URL]` writes a `.beadroll.json` to commit: an
explicit workspace id, and optionally a different coordination remote. Running
it again keeps the keys it does not manage. A checkout whose tree predates the
file reads the committed copy (`HEAD`, then `origin/HEAD`). `--remote` and
`$BEAD_REMOTE` override it per call; a tracked remote that differs from the one
a store already uses is refused until named deliberately, and a tracked id that
is another repository's derived id is refused outright: a cloned repository
cannot attach you to someone else's store. If a repository was renamed, the
first `sync` adopts its published tracker and remembers the mapping.

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

## Tests and reviews

```sh
./check.sh            # every test module with a scratch store (about 25 minutes)
./check.sh --stress   # also the randomised multi-machine models and the protocol sweep
./check.sh --real     # also import/export of the real tracker exports on this machine
```

`tests/` is one package organised by subject, not by who wrote the test:
`test_replay` (causality, registers, quarantine, validation, cycles; in-process and
fast), `test_protocol` (claims, attempts, fences, lost acknowledgements, sync races),
`test_storage` (segments, torn tails, identity, readers against writers),
`test_commands` (every verb end to end, identity detection, branch tracking),
`test_compat` (Beads import/export, real exports, `br`/`bv`), `test_hardening`
(the base under the protocol: git environment and remotes, URLs and credentials,
store files and modes, output streams, imports), `test_stress` (randomised
models) and `test_sweep` (the ownership protocol enumerated: three machines,
nine operations at seven fault boundaries, every ordered pair of follow-ups;
a ten-schedule slice by default, `BEAD_SWEEP=full` for all 660). `tests/support.py` holds the sandbox: every test builds its
own repository, bare remotes and machine homes in a temporary directory, and the
package sets a scratch `BEAD_HOME` so the default store is never touched.

Seven independent reviews have been run against this POC. Their reports are kept
verbatim under `reviews/`; `FINDINGS.md` is the ledger of every finding, its status,
how it was resolved and which tests cover it. `REVIEWING.md` is the brief for the
next one: rules, what not to report, the checklist, and the deliverables (a report
under `reviews/`, a `tests/test_review<n>.py`, proposed ledger rows). `infra/` holds
the scripts used against real machines and GitHub: `drive.py` (twelve cross-machine
scenarios over ssh), `soak.py` (random agent-like traffic), `roundtrip.py`
(import/export over real Beads exports) and the synthetic `seed.jsonl`.

## Where this differs from the design document

| Document | Here | Why |
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
- Ancestry over ownership events is kept as one bitset per event: memory grows
  with (events × ownership events)/8, about 60 MB at 10,000 ownership events.
  A permanent implementation would use a reachability index.
- The branch recorded on an event is whatever `HEAD` was in the directory the
  command ran in. It is a trace, not a lock: a claim does not pin a branch.
- With no persisted projection, every command replays the journal: measured
  by the fifth review at 0.7 s / 45 MB for 10k events, 3.4 s / 125 MB at 50k and
  14 s / 412 MB at 200k, far beyond the trackers measured so far (the largest
  real one is 280 issues). A persisted projection is the obvious next step if
  that ever matters.
  A fetch or push that gets no answer fails after `BEAD_NET_TIMEOUT` seconds
  (default 45) instead of hanging.
- Publishing is serialised on one ref. Under heavy contention (four machines
  syncing continuously) claims took 9 s median and 27 s at p90, and 3 of 841
  commands gave up after eight attempts.
- A renumbered child key changes under the machine that created it. Events
  refer to issues by an immutable id, so nothing is lost, but a key written
  into a commit message before the sync now names the other issue.
- A repository with neither a remote nor a `.beadroll.json` gets a machine-local
  tracker that cannot sync.
- Not implemented: `stats`, `count`, `epic status`, `graph`, saved queries,
  templates, external-project dependencies, `bv`-style ranking.
