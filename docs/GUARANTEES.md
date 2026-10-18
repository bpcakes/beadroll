# What bead guarantees

`README.md` is the front door; this is the ledger of behaviour behind it: every guarantee bead
keeps, in the words a review checks it against, kept as each was recorded when the behaviour was
built. `docs/REVIEWING.md` §1 is the map of the design and §2 the decisions that are not findings;
`docs/FINDINGS.md` is what the reviews found and how each finding was closed. A change in behaviour
is recorded here (`AGENTS.md`, *Making a change*). Two things stay in the README because a new
user needs them first: how two machines' concurrent edits resolve (*When two machines edit the
same thing*) and the variables bead reads (*Configuration*).

## The command line

The commands: `create`, `show`, `update`, `list`, `ready`, `blocked`, `close`,
`reopen`, `delete`, `dep add|remove|list|tree|cycles`, `comments [add]`,
`search`, `lint`, plus `init`, `import`, `export`, `sync`, `claim`, `release`,
`takeover`, `claims`, `log`, `recap`, `info`, `machines`, and for agents `help`, `prime`,
`onboard`. `--json` works anywhere on the command line; `bead` alone prints the
overview, and `bead help COMMAND` one command's page: what each option takes and,
for `create`, `dep` and `comments`, their forms and what a dependency type does.
A command that would change nothing says so instead of writing: a `close`,
`reopen` or `delete` of an issue already in that state answers `Already closed`
(and so on; `unchanged` in JSON) and keeps the reason it has, and an `update`
that names nothing to change answers `Unchanged`. `bead log` names who wrote
each event as claims do: the `--actor` or `BEAD_ACTOR` name when one was given,
else the agent and its thread; a `set` row names the fields it changed.
A mistyped command or option is a usage error that says which one was meant
(`bead lis`: `tip: some similar subcommands exist: 'lint', 'list'`); a long
option may be shortened while it stays unambiguous (`--stat`), and a value that
starts with `-` goes after `--` unless it is an option's (`--title "- x"`).

On a terminal bead's output is coloured: an issue's status mark, id and priority,
its type and branch dimmed, warnings and errors, and the help pages. Into a pipe
or a file, and with `--json`, it never is. `--color always|never` decides, else
`NO_COLOR` turns colour off and `CLICOLOR_FORCE` on (`CLICOLOR=0` and
`TERM=dumb` off too); `FORCE_COLOR` is not followed, since agent harnesses set it.
What an issue says is escaped first, so colour never carries a code of its own.

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

## Recap across trackers

`bead recap` reads every store directly under `BEAD_HOME` that holds an
`events` directory, from any working directory. It starts no git, needs no
checkout and contacts no remote; `BEAD_WORKSPACE` does not narrow it.

Of each tracker it shows the events not shown yet, in causal order, `init`
excepted. An event a late sync brings, or one a machine wrote with its clock
behind, shows when it arrives, whatever its time. The cursor keeps, for each
tracker, the frontier of what was shown, never a time, so it does not grow with
the history. Events waiting for causes not here yet are counted and show once
they arrive, and a tracker none of whose events can show yet (its init aside)
keeps the window it is owed; an ownership attempt shows once the remote has
accepted it. A tracker's window applies once, to what the first recap that shows
or counts any of its events finds in causal order; whatever is placed after
that is new activity and shows, whatever its time, as a late sync does. The
first recap on a machine shows the last 24 hours and counts the events before
them, and a tracker first seen later shows what came since the last recap that
marked anything seen, counted the same way. Those hours are the events' own
times, as their writers' clocks gave them: bead keeps no record of when an event
arrived. `--all` shows every event.

Only output to a terminal marks what it showed as seen, and only once it is
written. A pipe, a file and `--json` only show it unless `--advance` is given;
`--no-advance` marks nothing anywhere, and the two together are a usage error.
A reader that went away marks nothing. A save that fails once the answer is
written is an `IO` error with status 1, and the next recap shows the same
activity again, as it does after one killed before it saved. Two recaps at once
may show the same events: each saves under a lock held only while it reads and
replaces the cursor, never while it reads a journal or writes its answer, and
neither loses the other's mark. When both marked one tracker, the later save
keeps, of the other's frontier, only what its own read did not hold, since two
reads of a store that a sync is writing need not see one another's events; the
next recap that marks what it shows as seen, even with nothing new to show,
keeps its own frontier instead, so overlapping recaps do not grow the cursor.

A tracker that cannot be read, its journal or a directory whose name is no
workspace id, is reported with its error, and the others still show. The
cursor keeps what it held of it: its frontier, or, for a tracker never shown,
the window it is owed, so that the first recap that can read it shows what came
since that window rather than since the latest recap. Integrity problems are
listed under their tracker, each as `bead info` gives it, and its cursor moves
past them all the same. A `meta.json` that cannot be read does not stop recap;
the tracker is then named as its own journal names it.

The cursor is `$BEAD_HOME/.recap.json`, private and replaced atomically, beside
its lock file `.recap.lock`; both names start with a dot, which no workspace id
can. It holds the time of the last recap that marked anything seen, by showing
or counting an event, and, for each tracker, the frontier of what was shown or
the start of the window it is owed. It belongs to the home, whoever runs bead
there, and is never synced. One that cannot be read, or is not one, is an `IO`
error and is never reset: repair it, or remove it to start again from the last
24 hours.

`--format` lays the human output out: `events`, the default, one line per event
as `bead log` prints it, with its issue's title; `issues`, one line per issue,
in the order of its last event, with its status now, what happened (`created,
claimed, closed, 2 comments`) and who wrote last, where and when; `full`, every
event with its summary, machine, agent, whole thread, branch and whole commit.
Journal text is escaped in all three. `--json` always gives every event: one
object with `workspaces`, in workspace-id order, and `advance`, whether this
recap marks what it shows as seen. A tracker has `workspace`, `name`,
`last_sync`, `since` (where a first recap's window starts, else null), `earlier`
(the events before it, not shown), `events`, `pending` and `integrity`, or
`error` (`code`, `message`) when it cannot be read. An event has every member
`bead log --json` gives it, with the same meaning, and `machine_id` (the
machine's id, where `machine` is its local name when it has one), `title`,
`summary` (display text, not to be parsed) and `commit`. With no tracker it
says `No local trackers`, status 0.

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
| `bead close X` / `bead delete X` (as owner) | Closes (or tombstones) and ends exactly your claim, then publishes at once. `show` tells whether the closing commit is merged into the default branch; a commit recorded as anything but 4 to 64 lowercase hex digits, an object name or an abbreviation of one (uppercase hex, a revision expression), which another machine's event can hold, is `unknown`. |
| `bead close --recursive X` | Closes X and every issue beneath it through parent-child edges, at any depth, children before parents, in one write: your claims among them end, and someone else's claim on any of them refuses the whole command, naming each (`--force` overrides). Issues beneath that are deleted, or already closed, are left as they are. |
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
uniform. What an older bead sets aside costs it nothing more: it keeps
reading its projection and applying new events to it. The format number
changes only for a change that would make an old reader *misread* events;
none is planned. Should one ever be needed it follows
*writers lag readers*: the new bead reads both formats but writes the old one
until every machine on the tracker has announced, in the manifest it publishes,
that it reads the new one; then it flips. Nothing ever rewrites existing events.

## What else holds

- An imported dependency on an issue that is not in the tracker yet is kept by
  key and becomes a real edge on every machine the moment that key exists, so
  importing in several batches gives the same result as one. An import is
  checked whole before anything is written: one bad record fails it.
  Large imports and dry runs resolve dependencies without rescanning the batch
  for each target.
- Several bead processes on one machine (agents in different worktrees) never
  collide inside the private `sync.git`: each fetch lands in a ref of its own.
- A publish hashes only the journal files that changed since the last one: the
  segment being appended to and new claim files, not the whole history. What
  bead remembers of each file is a cache, `sync.git/journal-memo`: a file that
  changed in any way is hashed again, as is one written in the same moment as
  the memo itself, which a rewrite in place could have changed since with
  nothing to show for it. Deleting the memo, or `sync.git` itself, is always
  safe.
- A command reads the store's projection, `<store>/projection`, instead of
  replaying the whole journal: the state the journal replays into, kept with
  the identity of every journal file it was made from and a sum of its bytes.
  Records appended since are applied after it, as replay would apply them; a
  file written in the same moment as the projection is read against its sum;
  events written on this machine and others with all it holds in their past
  are applied after it as replay places them, though they were written
  concurrently with each other;
  anything else (a file changed another way, events written concurrently with
  what the projection holds, a takeover) has the journal replayed, and the
  projection written again. A write
  leaves it as it found it while the records after it are few, as `bead info`
  and `bead machines` do, which read it as a write does; the next command that
  reads issues (`list`, `show`, `ready` and the like) writes it again with them.
  It is a cache, never a
  source: deleting it is always safe, and a damaged one, or one another build
  of bead wrote, is never read.
- Each issue's texts (descriptions, designs, acceptance criteria, notes,
  comments, and the metadata its dependencies were added with, which only
  `export` prints) are kept beside the projection, in `<store>/projection-texts`.
  To print them, `show`, `comments` and the writes that answer with the issues
  they wrote (`claim`, `takeover`, `update`) read those issues' alone, and
  `export`, `search` and `lint` read them all; `list`, `ready`, `claims` and
  `prime` print none. Any command that takes records into the projection (a
  write its own, a reading command those written since it was saved, a sync
  those it merges) first reads in the texts of every issue they are about, since
  a long field set, a comment, a delete and a dependency added or removed may
  change them, and of every issue whose import waits for a key a create among
  them brings, since that create gives it an edge; and appends those that
  changed to the file when it writes the projection back. It is a cache too:
  texts that cannot be read as written have the journal replayed.
- Unlike the journal, neither file is flushed to disk: they are written whole
  and renamed into place, not fsynced. A crash or a power loss can leave either
  lost, cut, zeroed, as it was, or out of step with the other, and the next
  command that cannot read what they hold as written replays the journal once
  and writes them again; no answer ever differs for it.
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
  failed one keeps its status. Output is plain text into a pipe, a file and
  with `--json`; on a terminal it is coloured (*The command line*, above).
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

The variables bead reads are listed in the README, *Configuration*.

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
  (`cargo xtask timing --replay`; Python's bitsets took 11.8 s and 1.7 GB).
- The branch recorded on an event is whatever `HEAD` was in the directory the
  command ran in. It is a trace, not a lock: a claim does not pin a branch.
- A command reads the store's projection while it is current, and replays the
  journal when it is not (after another build of bead, a change it could not
  apply, or the projection removed). At 200,000 events (the largest public
  Beads trackers import as at most 29,000), a read took 73–87 ms with it and
  1.06 s without when the projection came in (roll-lqjj.2); a sync that merges
  ten events another machine wrote, 0.31 s and 1.31 s; a publishing sync, 0.38 s
  and 1.34 s (`cargo xtask timing`). With it, a read costs what the state does:
  18 ms for a thousand issues, whether 50,000 events made them or 200,000. A
  read that finds the projection current decodes its state part alone, which
  keeps, of the events, only who wrote values two writers set without seeing
  each other; every event's record is in the projection's index, which only
  `Projection::decode` reads: a write, the first read after one (which extends
  the projection and saves it), `info` and `machines` (roll-lqjj.35). Measured
  later, with both builds back to back on a machine shared with other work
  (review 16 had `list` at 0.205 s there), at 200,000 events `list` takes
  0.121 s rather than 0.146 s, `show` 0.116 s rather than 0.150 s and `ready`
  0.157 s rather than 0.201 s (`cargo xtask timing --read N`, `--decode N`); the
  first read after a write gained nothing then. Since each short field is held
  once, in its register (roll-lqjj.41), measured again with both builds back to
  back, the state part at 200,000 events is 9.45 MB rather than 10.68 MB and
  decodes in 38.8 ms rather than 43.6 ms, `Projection::decode` (a write's and
  the first read's after one) takes 58.5 ms rather than 62.4 ms, and `list`
  takes 0.108 s rather than 0.115 s, `show` 0.103 s rather than 0.113 s and
  `ready` 0.151 s rather than 0.157 s. Since dependency edges are held typed,
  their metadata with the texts (roll-lqjj.39), the state part at 200,000
  events is 7.69 MB rather than 9.45 MB and decodes in 33.1 ms rather than
  41.0 ms, holding 43.2 MB rather than 51.4 MB; `list` takes 0.099 s rather
  than 0.111 s, `show` 0.091 s rather than 0.106 s, `ready` 0.146 s rather than
  0.165 s and a local write 0.162 s rather than 0.184 s. A write derives the labels its own events
  changed, each issue's once however many of them changed it, and takes its new
  event ids in wherever they sort among the ids held, so one `bead close`
  naming 8,000 issues of 8,000 takes 0.7 s rather than 19.8 s; a batch that
  creates an issue, adds a dependency or takes one back still has the whole
  parent-child index built again, since a create gives the imports that waited
  for its key their edges, and that is one pass a command (roll-lqjj.17, V01,
  V03 and V05; `cargo xtask timing --extend N` measures the projection's part
  of it). A journal holding what `bead info` reports under `integrity` (a
  newer bead's event, an id with two contents, a copied machine identity's
  broken chain) keeps its projection current; only a takeover of an id no
  event has, and an id with two contents whose causes are not all there or
  loop back onto it, have every command replay it. A `close`, `reopen` or
  `delete` reads the state once before its events and once after, whatever
  the journal holds, and once more before an issue it names again: 60 closes
  of 4,000 issues with a newer bead's event in the journal take 0.066 s
  rather than 11.6 s, and 0.26 s rather than 11.7 s when the projection
  cannot be kept (roll-lqjj.28). An import's dependencies
  on keys no issue has yet cost what they are: a create visits only those
  waiting for its own key, so 16,000 imports
  each waiting for a key that never comes replay in 0.09 s rather than 30 s,
  and one issue waiting for 16,000 keys costs about what the same 16,000 edges
  cost it given at once (roll-lqjj.18; `cargo xtask timing --dangling N`).
  `bead ready`, `bead blocked` and `bead prime` judge what is held once for
  every issue rather than issue by issue, so a chain of 4,000 issues under one
  held root is judged in 2 ms where listing each one's blockers took 1.7 s, and
  `bead ready` there takes 0.037 s rather than 2.05 s; `bead blocked` lists
  every held issue's blockers once for all of them, so a chain of 16,000 issues
  under one held root takes 0.28 s rather than 39 s, and the commands that show
  several issues judge each one's `blocked` once for all of them, so `bead show`
  of all of them takes 0.62 s rather than 27 s; one issue's detail walks its own
  ancestors (roll-lqjj.19, roll-lqjj.24; `cargo xtask timing --ready N`).
  `bead show`, `bead update`, `bead claim` and `bead takeover` find the issues
  that depend on every issue they show in one pass for all of them, so showing
  all 8,000 issues of a sparse tracker takes 0.28 s rather than 3.26 s; one
  issue's detail still reads every issue's dependencies once (roll-lqjj.20;
  `cargo xtask timing --details N`). A write published at once reads the
  state twice, for the write and for the merge of the sync that publishes it,
  a claim once a round, and `bead sync` once a round when nothing comes in and
  twice when something does, so that at 20,000 imported issues an online
  comment takes 0.72 s rather than 1.05 s, an update 0.75 s rather than
  1.39 s and a sync that brings nothing 0.27 s rather than 0.60 s (roll-lqjj.29;
  `cargo xtask timing --sync N`, `online_seconds`).
  `bead info` and `bead machines` answer from the projection as `list` does,
  0.23 s at 200,000 events where they loaded the journal in 3.30 and 2.40 s;
  `bead log` still reads the whole journal for its events and their order,
  3.21 s there, but replays none of it (roll-lqjj.31; `cargo xtask timing
  --read N`). A write after a write takes the records before it in without
  writing the projection back until they hold more than a 64th of its bytes, so
  at 200,000 events it takes 0.26 s rather than 0.38 s, and a sync after a local
  write 0.34 s rather than 0.51 s; the read after a write still writes it back
  (roll-lqjj.36; `cargo xtask timing --read N`, `write_after_write_seconds`).
  A write that is not published (`--offline`, or on a tracker with no
  remote) keeps its transaction until the process exits, as a read keeps
  its state, and one that publishes frees it first, so that none holds
  two: at 200,000 events an offline write takes 0.20 s rather than 0.27 s
  and at 20,000 about 32 ms rather than 37 ms, while a write published at
  once and a sync are as they were (roll-lqjj.42; `cargo xtask timing
  --read N`, `write_seconds`); a write on a tracker with no remote, which
  freed it too, now keeps it and takes what an offline write takes, about
  90 ms less at 200,000 events (roll-lqjj.50, `no_remote_write_seconds`).
  A write that is not published (`--offline`) keeps its transaction until
  the process exits, as a read keeps its state, and one that publishes
  frees it first, so that none holds two: at 200,000 events an offline
  write takes 0.20 s rather than 0.27 s and at 20,000 about 32 ms rather
  than 37 ms, while a write published at once and a sync are as they were
  (roll-lqjj.42; `cargo xtask timing --read N`, `write_seconds`).
  A write-back of the projection cuts what it keeps of what every event had
  seen down to the events its state names only once it holds more than an
  eighth more than it did after the last such cut, not every time, so at
  200,000 events the read after a write takes 0.22 s rather than 0.26 s;
  between, it holds up to an eighth more events: at worst there its index is
  9.65 MB rather than 8.93 MB, and its file larger by as much, and a decoded
  projection holds 77.1 MB rather than 75.7 MB (roll-lqjj.43; `cargo xtask
  timing --read N`, `read_after_write_seconds`; `--decode N`,
  `worst_index_bytes` and `worst_projection_heap_bytes`).
  A real tracker's state is mostly
  its texts, which only the commands that print them read: on three of the
  largest public Beads trackers (21–47 MB, up to 13,394 issues) a `list` takes
  36–75 ms and a published write 0.21–0.35 s, while `export`, `search` and
  `lint` take what the texts do. Importing the largest holds 172 MB on the
  heap offline (341 MB resident); published, it peaks at 235 MB (563 MB
  resident) as its publish makes the projection, which a new machine's first
  sync of the tracker takes too (roll-lqjj.9). A merge that brings events another
  machine wrote concurrently with what the store's projection holds replays the
  journal, as about one merge in ten does where two machines write: 0.45 s at
  50,000 events, 1–2 s at 200,000, kept so for correctness (roll-lqjj.8). One
  whose events are concurrent only with writes left after the projection
  extends it (roll-lqjj.36): a sync that merges an event concurrent with three
  such writes, and publishes them, takes 0.23 s rather than 0.93 s at 50,000
  events, and 0.63 s rather than 3.69 s at 200,000. A replay holds at most 107 MB
  on the heap at 200,000 events (events the size of this repository's own take
  more than three times that). A store heavy in claims still checks every
  attempt file's metadata. Reusing directory listings, unchanged published
  claims trees and the post-fetch ref writer reduced a warm `list` from
  0.57 s to 0.47 s and a publishing sync from 2.86 s to 2.45 s at 92,000
  attempt files; at 2,680 files, 33 ms to 28 ms and 208 ms to 170 ms
  (`cargo xtask timing --read N --ownership` and `--sync N --ownership`,
  200,000 and 6,000 events, release on macOS arm64, best of five against a
  local bare remote). Reads still check metadata for every file, and the
  local-write timings at 200,000 events did not improve. Each attempt file's
  metadata is now checked once a lock phase, whoever asks, and `meta.json`,
  which lists every published file, is read once a command (roll-lqjj.30): at
  91,952 attempt files a `list` takes 0.75 s rather than 0.97 s, a local write
  0.93 s rather than 1.26 s, a published write 3.9 s rather than 5.7 s and a
  sync with nothing new 2.9 s rather than 4.0 s (Linux, release, best of three
  to five against a local bare remote, other work running). A fetch or push that
  gets no answer fails
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
