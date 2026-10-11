# Reviewing `bead`

A brief for an independent reviewer: an agent or a person who did not write this, whose job is
to find out whether the design holds, to break it if they can, and to report with
reproductions. Reviewers do not extend the tool.

Nine reviews have run; the latest two audit the Rust port plan. Their reports are kept verbatim in
`reviews/`; every finding and what became of it is in `FINDINGS.md`. Start from what they did
not cover.

## 1. What this is

`bead` is a proof-of-concept issue tracker for coding agents that work from many git worktrees and
several machines at once. It replaces the workflow around `br` (beads_rust), whose tracker state
lives in a file tracked on code branches: dirty checkouts, tracker-only commits, merge conflicts,
state split between worktrees and machines.

It is one binary, built from a Cargo workspace (Rust 1.99) whose crates each depend only on the
ones before them, which the compiler enforces (`docs/RUST-PORT.md` §2.1):

- `beadroll-core`: Python-compatible JSON and text, event validation, journals and their settling,
  causal order, replay into state, readiness and the other queries, issue views, Beads import
  planning and export. No filesystem, processes, clock or environment.
- `beadroll-git`: the only code that starts git: the runner (its own session, timeouts, output to
  unlinked files), the environments of the checkout and of `sync.git`, remote URL rules, the
  classification of push outcomes and local damage.
- `beadroll-store`: this machine: `BEAD_HOME`, the store's files, modes and lock, segments and torn
  tails, attempt, pending and receipt files, identity, the workspace a checkout resolves to, the
  failpoint registry.
- `beadroll-sync`: transactions, the link to the coordination remote, `synchronise` and the
  ownership protocol.
- `beadroll`: the `bead` binary, `commands` and `cli` (parsing, rendering, output streams, exit
  codes); `beadroll-test-support`: the sandbox the black-box tests run it in.

`cargo build --package beadroll` builds the working copy as `target/debug/bead`; `README.md`'s
*Install* has `cargo install`. Until 0.2.0 bead was a Python package of seven modules, which the
Rust implementation ports with nothing changed in behaviour except where Python's failed: the
structured errors below where Python died with a traceback (§5 row 26), and a coordination ref
named by its UTF-8 bytes under any locale (`roll-k2wy.18`). The last of the Python package is the
tag `python-0.1.0`, and its function names survive in the Rust ones below.

The design under test, in the order it is best read:

- **One journal per machine**, in append-only segment files
  (`$BEAD_HOME/<workspace>/events/<machine>/<segment>.jsonl`). A machine only appends to its own
  journal. A full segment is never rewritten.
- **Every event names the frontier its writer had seen** (`deps`; ids are UUIDv7). Current state
  is the replay of all journals in causal order (`causal_order`, `replay`, beadroll-core).
  Machines merge by exchanging files; nothing is text-merged.
- **One store per repository per machine**, outside every working tree (`find_workspace`,
  beadroll-store: tracked `.beadroll.json`, else a hash of the normalised origin URL, else a local
  id). Nothing is ever written inside a checkout.
- **Sync through one dedicated git ref** (`refs/beads/journal`) on one coordination remote,
  through a private bare repo (`sync.git`). The pushed history holds a manifest and the
  journals, never code. A push is a plain fast-forward whose parent is the fetched head, so the
  remote refuses it if anyone else published first (`Link::publish`, beadroll-sync).
- **Claims are persistent and settled online** (`coordinated`, beadroll-sync). Ownership events
  are not journal records: an *attempt* (the `claim`/`release`/`takeover` plus any edits made in
  the same command) is one file under `events/<machine>/claims/`, written first under `pending/`,
  where it counts for nothing anywhere until the remote holds it. A refused push drops the
  attempt; an unanswered one stays pending until a later contact finds it on the remote
  (confirmed) or fences it off with a commit on the same head (`settle_pending`). Attempt files
  are immutable and compared on sync; the accepted copy wins. A command whose push landed
  without an answer recognises its own accepted work (`settle_commands`) instead of redoing it.
  Ownership never expires; it ends only by `release`, `close` (by the owner) or
  `takeover --reason`. `--offline` claims are provisional and never displace a confirmed owner.
- **Concurrent writes are flagged, not merged**: each editable field keeps every value whose
  writer had not seen it replaced (`write_register`); the newest is shown and the alternatives
  appear under `conflicts`. A later write that has seen them all settles it. A disputed status
  makes an issue not ready, not claimable, and not "done" for dependents.
- **Labels and dependency edges are add-wins sets**: a removal removes only the additions its
  writer had seen.
- **Identity on every event**: opaque machine id, agent type and thread id (`detect_actor`,
  beadroll-store). **Place on every event**: branch and commit of the checkout the command ran in
  (`where`, `capture_where`).
- **Beads JSONL is an import/export format only** (`plan_import`, `export_records`,
  beadroll-core).

Read `README.md`, then each crate's `AGENTS.md` and code in the order above, then
`beadroll-test-support` and one test module.

## 2. Decisions you must not report as findings

These were decided deliberately. Report problems *with* them, not their existence.

- Claims never expire; a missing owner needs a human `takeover`. No heartbeats.
- The owner of a claim is the agent thread; a later session must `takeover` to continue.
- `update --status in_progress` is a claim, and `create --status in_progress` is refused;
  `update --status tombstone` is refused in favour of `delete`.
- `title`, `priority`, `issue_type` and `status` cannot be cleared.
- Recovery after a lost acknowledgement is per command (the receipt beside a pending attempt):
  a retry of the *same* command by the same actor is recognised; a different command after the
  crash is a new command and sees the accepted state.
- The Rust port preserves Python's receipt signature bytes and retirement rules while either
  implementation may use the store, including rollback. Sync settles attempts but can retain
  accepted `.command` receipts; an empty `pending_ownership` is not permission to discard them
  (`RUST-PORT.md` §2.3j).
- The display value for a conflicted field is the newest event id; no resolution command.
- Every local write (`create`, `update`, `comments add`, `dep`, `import`, `init`, and `close` by the
  owner) is published immediately, not as a conditional update; `--offline` keeps it local, and a
  remote that does not answer leaves it recorded and reported as unpublished (`unpublished` in every
  dict result and in `info`; a trailing line for humans) until a sync or an online command carries
  it. A close or release that names a claim superseded by a takeover is ignored and listed under
  `stale`, whichever was written first.
- A renumbered child key (`x.1` → `x.2` after two machines created children offline) changes
  under the machine that created it; events refer to issues by immutable id.
- One event with two different payloads is quarantined alone; nothing waits on it.
- The store lock is held while a sync or claim pushes (about a second against GitHub).
- Publishing is serialised on one ref; under heavy contention claims take seconds.
- No compaction beyond segment rotation. No persisted projection: every command replays the
  journal, 8 ms for 10,000 events and 0.26 s for 200,000 (`RUST-PORT.md` Appendix A).
- What each event had seen is one clock per event over chains, the reachability index of
  `state/seen.rs` (`RUST-PORT.md` §2.3c): every machine's journal is a chain, and each event of an
  attempt file joins a chain whose last event is among its ancestors, or starts one, so there are
  about as many chains as machines that write attempts. A clock holds the furthest position the
  event saw on each chain, and `saw` is `clock[chain] >= position`. Memory grows with events ×
  chains, not with events × ownership events as Python's bitsets did: 200,000 events, 55% of them
  in attempt files, replay in 0.33 s with a 958 MB peak, 747 MB of it the loaded journal itself
  (roll-dih8.11).
- Exported comment ids for comments made here come from a 48-bit hash range; a collision is a
  one-in-10^14 event and is then resolved by the ids alone.
- `dep cycles` stops after 200 cycles.
- `ready ID` and `ready --epic ID` are one scope, ID's subtree, recursive as br's `--epic` (the
  maintainer's decision of 2026-10-09; until then `--epic` was another name for `--parent`): every
  issue beneath ID through parent-child edges, at any depth and through every parent link, whatever
  lies in between; ID itself is never part of it. Readiness is judged on the whole tracker.
  `--parent ID` stays ID's direct children, as in br and `list --parent`. One scope at a time, each
  option given once (`USAGE`, as br refuses both), resolved before anything is filtered.
- A repository with neither a remote nor `.beadroll.json` gets a local tracker that cannot sync.
- Setting a tracker up has one path (the maintainer's decision of 2026-10-11, roll-dih8.13): the
  URL-derived workspace, nothing committed; a tracked `.beadroll.json` only for a coordination
  remote other than origin or for `"where": false`, committed on the default branch. `bead init`
  asks the remote first (`git ls-remote`, as a link asks, from the checkout, writing nothing in
  the store): a tracker already published there is joined, adopted as `sync` adopts it when it
  was published under another id, and once the remote has said it publishes one, any failure to
  join it fails `init`, which then starts nothing. One it cannot adopt is refused
  (`WORKSPACE_MISMATCH`), even under an explicit `BEAD_WORKSPACE`, since that tracker could never
  publish there. `init --tracked` names the tracker this checkout has, joined just now or here
  already (a tracked id the remote publishes, one an earlier `init` adopted), and mints a new id
  only where there is none, since a new id beside it would be a second tracker; it never pins a
  URL-derived one (`CONFIG`): that id would stop matching when the URL changed, and the file
  would then be refused as another repository's. An unreachable remote, one that publishes
  nothing, `--offline` or no remote: `init` starts the tracker as before, and publishes it only
  to the remote it chose, never again after it through `--remote` as typed. `info` and `prime`
  warn while `.beadroll.json` would not reach the other checkouts, comparing it with its commit as
  JSON, so line endings git converts are no change; where the tree lacks it and HEAD's copy is in
  use, that copy is checked against origin's default branch.
- A remote is kept as it was chosen (G22; the maintainer's course correction of 2026-10-11,
  roll-dih8.13: rounds of findings came from resolving the choice to a URL at once and using that
  string for transport, memory, comparison and advice, which made bead re-implement git's remote
  semantics piece by piece). The choice is the name of a remote the checkout configures (origin
  when none is chosen), or a URL or a path as given; it is what `meta.json` remembers, what is
  compared, and what advice gives back (`bead sync --remote origin`). A name is read for its URL
  only to reach the remote: the URL `git push NAME` is configured with (its first push URL, else
  its first URL, a password dropped), given to git as the URL of a remote defined for the command
  (`remote.bead-<token>.url`, and `.pushurl` for a remote's own push URL, through
  `GIT_CONFIG_COUNT`), never as an argument git would read as a remote's name, so that git
  rewrites it as for `git fetch NAME` and `git push NAME` (`insteadOf`, and `pushInsteadOf` except
  to a remote's own push URL), from the top of the checkout. A configured remote's name wins over
  a same-named path, in a value remembered or tracked too, as in 0.2.0. A URL or a path is read
  once: a relative path from the top of the checkout when it is stored and from the cwd when it
  was typed just now, through its symbolic links, whether it exists or not, except a spelling an
  `insteadOf` or `pushInsteadOf` rule is a prefix of (as written or as resolved), which is git's
  as written.
- Two choices are the same remote only as chosen: the same name, or the same URL so read; never
  two spellings taken for one place (`x` and `x.git` are two repositories, and two spellings git
  rewrites to one place are two choices). A tracked remote that is not the one the store last
  went through is refused (`CONFIG`) until changed deliberately (`bead sync --remote`, `bead init
  --tracked --remote`), with one allowance for older versions, which saved a name as git expanded
  it: a tracked name and a saved URL are the same where git's own answers agree (`git remote
  get-url` with and without `--push` for the name, `git ls-remote --get-url` for the URL, every
  rule applied once), each read as the link reads it, a relative path from the top of the
  checkout through its symbolic links, a password dropped as from every URL. Going back to 0.2.0 or Python, a store whose tracked and
  remembered remote are one name is refused there until `bead sync --remote NAME`, as they refused
  a name their own `init --offline --remote` saved.
- Origin is the name `origin`, never remembered: it is looked up each time, so `git remote set-url
  origin` is followed, as a remote chosen by another name is, and reached as `git push origin`
  reaches it, by the URL it is configured with (a relative path, or a spelling a rule rewrites,
  left to git as written), whether it is named or nothing is; only the URL-derived id reads a
  relative path from the top of the checkout first. Origin's own URL (the one `git push
  origin` is configured with) given as a URL spelled as configured (a password dropped from each),
  which git reaches alike, rules and all, is origin, reached as origin is, its own push URL
  included: `init --tracked` stores it as `origin`, and a sync given it forgets it. Any other
  spelling of the same place is a URL of its own (an absolute path for origin's relative one,
  which a rule can send elsewhere), and so is a relative path typed just now, read from where it
  was typed. A remote of another name is never origin, whatever its URL. A remembered value is origin when it is
  that name or, as older versions saved it, origin's URL: spelled alike, or, for two URLs that are
  not local paths and that no rule rewrites, the same repository however spelled (G25: ssh or
  https, `.git`), as the URL-derived id takes them. A tracker published under another id is
  adopted only through origin. `init --tracked` stores a relative path from the top of the
  checkout, and one a rule rewrites as the very spelling that reads back as written (`.`, `..`
  and symbolic links kept).
- Output is UTF-8 regardless of locale; human output escapes control characters; `--json` always
  wins the representation (`create --silent --json` gives `{"id": …}`); a closed stdout exits 0
  quietly.
- Passwords in remote URLs are dropped, never stored or used; the store is private by its own
  file modes (0700/0600), files written into the checkout follow the caller's umask.
- An import that contains one invalid record or dependency fails whole; nothing is written. A
  status bead does not set itself (`pinned`, a custom one) is kept verbatim, as Beads tools keep
  it; such an issue is neither ready nor done.
- Text from files and stdin is UTF-8 regardless of locale; OS errors are structured (`IO`); a
  git child runs in a separate session without the controlling terminal. A timed-out fetch is
  killed with its process group; a timed-out push kills only the client so its uncertain result
  can be settled by the protocol.
- A command receipt survives a *failed* later command by the same actor and is dropped only when
  a later command by that actor goes through: the remote accepted it, or it was a local write that
  was appended. A recognised retry still reports the claim as it is now.
- A push's outcome is git's own status line (`--porcelain`); a `[remote rejected]` that is the
  remote's verdict (a hook, a rule) is `REMOTE_REFUSED` and is not retried; one the remote could not
  take (`unpacker error`, `missing necessary objects`) is retried after a fresh fetch; a
  remote that has lost objects is `REMOTE_DAMAGED`.
- `sync.git` is a cache: when git reports a local read error in it (an empty or corrupt object), bead
  sets it aside as `sync.git.damaged-<time>` and makes it again, once per command, under the store
  lock; the second time in one command is `SYNC_REPO_DAMAGED`. A line the remote relayed is never
  taken as local damage.
- git's automatic maintenance is off in `sync.git` (`gc.auto=0`, no detached gc, `fetch.unpackLimit=1`
  so fetched objects stay packed); bead repacks it under the store lock when loose objects or packs
  pile up, and prunes only unreachable loose objects older than an hour (an in-flight fetch's are
  younger). The remote's own maintenance is the remote's business.
- Format 1 is frozen; changes are additive. An event an older bead cannot apply (a new operation,
  a new field, a newer `v`) is set aside, reported under `integrity`, carried through sync
  untouched and applied after an upgrade; a mixed fleet is safe. A breaking change, should one
  ever be needed, follows writers-lag-readers through the manifest; none is planned.
- Events nest at most 100 levels (`MAX_NESTING`); replay keeps looser type rules than writes
  (`write_problem`) so histories written before a rule mean the same on every machine.
- Integers have any size up to Python 3.11+'s default 4,300-digit decimal conversion limit on JSON
  input and output, not unlimited integer tokens (§5 row 2). The oracle pinned that default until
  the cutover; a machine whose Python ran with another limit had to resolve its store's
  compatibility before it upgraded (`RUST-PORT.md` §8).
- What a journal line, attempt file or manifest means does not depend on the Python reading it
  (`RUST-PORT.md` §5 rows 3, 6, 7, 22 and 23): JSON is read from bytes as UTF-8 with surrogates
  passed (no byte-order mark, UTF-16 or UTF-32 is guessed), at most 512 levels deep (counted before
  `json.loads`, below every interpreter's own limit), with NaN, Infinity and numbers beyond a float
  refused; ids and dependencies match the UUID shape whole; `complete_prefix` and the manifest read
  as strictly as everything else, and the manifest's format must be the integer 1.
- Key numbers, segment names and URL schemes are ASCII (§5 row 24): `\d` and `(?i)[a-z]` are never
  Unicode-aware there, so a renamed copy of `x.٣` keeps its own name and `httpſ://` is not
  `https://`. This departs from the plan's recommendation (keep), on the maintainer's decision.
- Shapes bead never writes, which made Python's replay or a query fail, or answer by interpreter
  version, are refused by validation (§5 rows 25 and 27 to 30): a `dep_add` meta that is not an
  object, a `dep_remove` key that is not a string, a create key whose number has 4,300 digits or
  more, an imported comment with a `uid` or a `where`, a create clearing `created_at`; a
  quarantined id's stand-in names its issue only by a string; an import refuses an escaped lone
  surrogate anywhere in a record. Replay has no last resort to reach.
- A renamed copy that finds every free key taken fails replay with `KEY_SPACE`, as in Python,
  whose replay raised it: beadroll-core's `replay` returns it.
- Where Python failed with a traceback, Rust answers with a structured error instead, each recorded
  as it lands (§5 row 26). In beadroll-core: `mint_key` under a parent whose children's numbers have
  more than 4,300 digits, or would count on to 4,301, is `VALIDATION` (`MintError::NumberTooLong`).
  In the store, sync and command line (Phase 2a): a `BEAD_NET_TIMEOUT`, `BEAD_ROTATE_BYTES`,
  `BEAD_REPACK_LOOSE` or `BEAD_REPACK_PACKS` that Python's `float()` or `int()` cannot read is
  `CONFIG` when bead starts, `--version` included; the last segment of this machine's journal, when
  its name is not a number (or the next one's would have more than 4,300 digits) and bead appends
  after it, is `IO`; a `published` count in `meta.json` that is not a number, or counts that make
  the unpublished count longer than 4,300 digits, is `IO`; a `GIT_CONFIG_COUNT` that `int()` cannot read where Python reads it
  (it is not digits and nothing was carried over, or its digits are superscripts) is `CONFIG`; an
  answer standard output refuses (other than a closed pipe or a hung-up terminal) is an `IO` error
  on standard error, exit 1; a `ps` line that names no command ends the nearest-harness walk.
- The Rust bead keeps CPython's semantics wherever Python's bead had them, as `RUST-PORT.md` §5
  rows 1 to 21 record (the rows not stated above): replay takes a boolean where an integer is due
  (`"v": true` is format 1; a `true` priority sorts as 1 and prints as `True`), as histories
  written before `write_problem` had it, while writes refuse one; a lone surrogate in a line,
  escaped or raw, reads as one U+FFFD
  and is reported; a duplicate key keeps its last value at its first key's place; whitespace,
  `strip()` and lower case are Python's Unicode ones where Python used them (`split_list`, comment
  emptiness, `search`, `normalize_remote`); a path is resolved as `os.path.realpath` resolves it,
  never by libc; `--estimate` and `--limit` read numbers as `int()` does (`" +5 "`, `5_000`);
  conflict values print as `repr()` prints them, and `True` as `True`; an OS error reads
  `"{strerror}: {path}"`, a rename's naming its target; the JSON of each output and file is the
  `json.dumps` dialect Python wrote it in; the command line is argparse's, its errors and help
  pages included (§2.3h); SIGXFSZ is ignored, so a file-size limit is an `IO` error, and standard
  input closed at exec is no stream, `-` then an `IO` error; a sync or ownership command makes 12
  rounds with `U(0.1, 0.4) × attempt` seconds between them; the names of fetch refs, temporary
  files and set-aside copies are Python's.
- beadroll-core was proven equal to Python by `cargo xtask diff` until the cutover: replay reports
  and JSON, over the journals the Python tests built and generated histories, valid and hostile.
  A difference was fixed in Rust or decided as a §5 row; the harness kept no list of expected
  differences. Since the cutover the goldens (`crates/beadroll/tests/golden/`) and the answers
  `parity` pins hold what Python's bead printed.
- The binary has failpoints in every build, activated only by `BEAD_FAILPOINTS` and the two
  crash hooks Python had (`BEAD_TEST_CRASH_BEFORE_PUSH`/`AFTER_PUSH`, which are aliases); a spec
  that cannot fire as written is `CONFIG` (`RUST-PORT.md` §2.3g, §4.5; the maintainer's decision
  of 2026-10-09). In-crate tests pass the registry in `Settings`; `error(CODE)` at
  `ownership.after-confirm` is their in-process form of Q03's interruption.
- The Rust binary starts fewer gits than Python (Phase 4, `RUST-PORT.md` Appendix B): one combined
  `rev-parse` for the repository's top, common directory and format, and in `sync.git` a
  long-lived `cat-file --batch-command`, `hash-object --stdin-paths` and `mktree --batch`, and an
  `update-ref --stdin` for the ref after a push, with the listing and `TIP^{tree}` worked out
  in-process as git works them out. Whatever is not certain goes to the git Python runs for it,
  and its answer or failure is the one given; `sync.git` gets the same objects and refs. What can
  change while bead runs is asked by Python's own call where Python asks it: origin's URLs each
  time the workspace is found, the transport settings each time a link is made, HEAD and its
  branch (one `rev-parse` where its answer is certain) each time a transaction starts. The two
  refs after a fetch stay Python's two `update-ref`s, at Python's pace: as one transaction they
  deleted the fetch's own ref while concurrent fetches of the store still listed every ref, and git
  failed those (G16; Appendix B). What still differs: which gits run (one more `rev-parse` where a
  combined one cannot answer: a bare repository or none at discovery, no commit yet or a branch
  name git finds ambiguous when a transaction starts); origin's `config` runs, read-only, beside
  the first `rev-parse`, so also in discovery's early-error paths where Python stops before asking
  for origin; and a SIGKILL while a blob is hashed can leave a file in the temporary directory (the
  attempt's or the manifest's content, never in the store).
- A fetch that loses to another process's fetch of the store is tried again (roll-dih8.12): git's
  checks of a fetch read every ref, and every process fetches into a ref of its own under
  `refs/beads/fetch/` that it deletes after, so git can find such a ref gone between listing it
  and reading it (`bad object refs/beads/fetch/…`), or the directory of its own removed while it
  makes it (`cannot lock ref 'refs/beads/fetch/…'`), and it fails the fetch though the remote
  answered. Only git's own lines count, never one the remote relayed. Five attempts in all, a
  wait of `U(10, 50) ms × attempt` before each new one, short because a claim fetches under the
  store lock; after the fifth the fetch fails as it did before (`REMOTE_UNREACHABLE`, git's
  words). Chosen over `transfer.hideRefs=refs/beads/fetch`, which keeps those refs out of the
  check only where git's fetch honours hidden refs (2.55 does, 2.40, the oldest supported, does
  not) and leaves the lock form as it is.
- `BEAD_HOME` must be absolute; the store's modes are 0700/0600 whatever the umask.
- Only directories bead creates get private modes; a user-supplied `BEAD_HOME` is chmodded 0700
  on first write (reviewer observation, kept).
- origin is never remembered as the coordination remote; only a remote chosen otherwise is: by
  its name, also one that shares origin's URL (its own push URL, or a `pushInsteadOf` rule, can
  send the two apart), or by a URL other than origin's (G25: in any spelling of its repository on
  a forge). A tracked config seeds a new store's remote but never silently moves an existing
  store's.
- bead's pushes skip the user's client hooks (`--no-verify`); the remote's hooks still apply.
- The remote string reaches git after `--`; a tracked workspace id shaped like a URL-derived id
  that is not this origin's is refused.

## 3. Baseline

Do not take the baseline from a document; run it:

```sh
./check.sh            # every test but the slow models, the doctests and the infra/ drivers' tests
./check.sh --stress   # also the randomised multi-machine models (several minutes)
./check.sh --real     # also import/export of the real tracker exports on this Mac
```

`check.sh` runs `cargo xtask check` (it needs rustup and cargo-nextest): it prints one line per
step, gives each a scratch `BEAD_HOME`, and exits non-zero on any failure, including a failed
round-trip command or differing exported records with `--real`. A failed step keeps its full
output in `target/check/`, and a failed round trip shows its whole report; its imports run
offline in the scratch store. The default sweep includes owner close/delete retries after an
interrupted publish; the full sweep (`BEAD_SWEEP=full`) has no expected-failure exemptions. Every
schedule must show that its fault fired (the operation's exit 98 or 99, its SIGKILL, or the
remote's stall hook started by its push), so none passes without reaching its boundary.
xtask's `check::check_script`, the sweep's `sweep_failure_reporting` and
`infra/test_roundtrip.py` exercise these failure paths with synthetic results in the fast
baseline. Record the baseline output in your report.
The real-infrastructure scripts (four machines, a throwaway GitHub repository) are
under `infra/`; their last results are summarised in `README.md`.

## 4. Rules

- Work in a scratch directory. **Always set `BEAD_HOME`** to a scratch path before running `bead`;
  the default (`~/.local/state/beadroll`) holds the project's own real tracker (this repository
  dogfoods `bead`; its `.beadroll.json` names that workspace) and must not be touched. The tests
  give every machine a scratch `BEAD_HOME`; do it yourself for anything else.
- Run `bead` only inside repositories you created for the test, or with `BEAD_WORKSPACE` set.
  Never run it inside the real repositories under `~/Documents` (the origin URL would route you
  to a real workspace id). Reading their `.beads/issues.jsonl` files is fine; do not copy their
  contents into any remote.
- Local bare repositories are the default remote. The only real remote you may push to is the
  throwaway repository the maintainer provides in `$BEAD_TEST_REMOTE` (private, synthetic data).
  Do not create other repositories or refs anywhere, and do not push to `main` there.
- Do not modify bead's code (`crates/`), `README.md`, `FINDINGS.md`, existing tests or earlier
  reports. Proposed fixes go in your report as unified diffs.
- Do not change SSH, firewall, git or shell configuration on any machine. Do not run `br` or
  `bv` inside real repositories (`br where` writes lock files).
- Starting Codex or Claude Code sessions from a test is for identity detection only: at most a
  couple, and say so in the report.
- Inject faults with failpoints (`BEAD_FAILPOINTS`, `RUST-PORT.md` §4.5), never by patching bead
  or the clock: `backoff@always=skip` skips bead's wait between attempts, never a shortened sleep;
  `pause-until(PATH)` holds bead at a point until the test touches `PATH.go`; `exit(N)`,
  `sigkill`, `errno(...)` and `fail-reply` stop or fail it there; `clock`'s `offset` and `at` move
  its clock. A spec that cannot fire as written is `CONFIG`, so a fault never silently misses.
  Tests that call the crates in-process pass the registry in `Settings`.
- Report outcomes faithfully: a test you could not run is "not run", not "passed".

## 5. Checklist

Work through these; add your own. For each, say what you did, what you expected, what
happened, and attach the reproduction (command or test).

**Replay and causality**
- Same event set → same projection, conflicts, claims and readiness under every delivery
  order, duplicate delivery, and journal enumeration order (beadroll-core's `replay` tests permute
  three to six machines; try more, and thousands of events).
- Ancestry (`State::saw`) depends on each event depending, directly or not, on its predecessor
  in the same journal, and on the chains the events of attempt files join (`state/seen.rs`): an
  event joins a chain only when the chain's last event is among its ancestors, and its clock
  holds the furthest position it saw on every chain. Construct journals where that fails
  (hand-written events, ownership events whose ancestors cross chains) and see what replay does.
- Events whose causes are missing must wait (`pending`), never be treated as concurrent.
- Unknown `op`, unknown `v`, missing fields, non-UUID ids, deps that are not lists: is the
  damage scoped to the event?

**Journal integrity**
- Concurrent writers on one machine (`writing` uses `flock`); readers take no lock and must
  tolerate a torn tail and a half-written attempt file. Kill `-9` a writer at every boundary.
- Segment rotation (`active_segment`, `BEAD_ROTATE_BYTES`) under two local writers.
- Merge rule (`Link::merge`): one side must be a prefix of the other. Own-journal fork error,
  foreign-fork report, a remote truncated or rewritten (`git push --force` to the ref), an
  extra segment, a segment deleted locally.
- Duplicates: the same id in two journals with identical payloads; reused ids across
  workspaces.

**Claims**
- Try to produce two confirmed owners of one issue on any pair of machines, by any
  interleaving: simultaneous claims, claim during another's release, takeover during release,
  two takeovers at once, claim after a forced close, claim of a tombstoned issue, claim while
  the status is disputed, offline claim then online claim by the same thread, release of a
  provisional claim.
- `generation` must be monotonic on every machine after any interleaving, including a claimant
  whose fetch was stale.
- Lost acknowledgement at every point: `BEAD_TEST_CRASH_BEFORE_PUSH`, `BEAD_TEST_CRASH_AFTER_PUSH`,
  a push killed mid-flight, a push that times out (`BEAD_NET_TIMEOUT`) but was applied, a
  receive hook that stalls (see the `protocol` tests). Then: a retry of the same command, a
  different command, a `sync`, from the same and from a new process.
- The `superseded` pre-pass in `replay`: find a case where a takeover rejects a legitimate
  write, or where a hand-written takeover corrupts ownership.

**Fields, labels, dependencies**
- Concurrent writes to every register, including `None` (a clear) against a value.
- Label add/remove and dependency add/remove interleavings across three machines; a cycle
  formed by merge and then partially removed.
- Readiness against `br` on the real trackers (`infra/roundtrip.py` imports them; compare
  `ready`/`blocked`), especially disputed statuses, tombstoned blockers, multi-parent issues.

**Sync protocol**
- Interleave local writes with a slow push. Manifest and workspace checks. Ref validation.
  Retry exhaustion against a remote that always moves. Two machines sharing a machine id.

**Beads compatibility**
- Round-trip every real tracker export, including those `br` rejects. Export after mutations:
  comment ids, dependency metadata, tombstones, unknown fields in `extra`. Feed exports to
  `bv --robot-triage`, `--robot-plan`, `--robot-insights`.

**Identity and privacy**
- Grep every event ever written for absolute paths, hostnames, usernames, or profile
  directory names. Only opaque machine ids, thread ids, branch names and commit hashes should
  appear.
- `detect_actor`: `CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`/`CODEX_SESSION_ID`, nested
  harnesses (process tree), a human shell, `--actor`, `BEAD_AGENT`, a sandbox without `ps`.

**Performance and scale**
- Synthetic journals of 10k, 50k and 200k events, and ownership-heavy histories: replay time,
  memory, `show` latency. Growth of the remote over 1,000 syncs with and without `git gc`.

**CLI**
- `--json` and `--actor` anywhere in argv; multi-id commands; `--description-file -`; exit
  codes and the JSON error envelope on stderr; `list` filters and `--limit 0`.

## 6. Stress-test plan

Simulate machines locally: each is its own `BEAD_HOME` and its own clone of one local bare
repository. `beadroll-test-support` (`Sandbox`, `Machine`) shows the pattern;
`crates/beadroll/tests/cli/stress/` has two randomised models to extend.

1. **Randomised model check.** 6 machines, 2,000+ operations drawn from the real distribution
   (show 32%, update 12%, list 9%, ready 6%, close 5%, create 5%, dep 5%, comments 4%,
   claim/release/takeover, sync every few ops), with random partitions. Keep a reference model
   of ownership from the commands that were *accepted online*, and assert after every full sync
   round: identical exports and per-issue `show` everywhere; at most one confirmed owner per
   issue, equal to the model; every accepted claim visible everywhere; no `waiting_events`,
   `integrity` entries or `forked_journals`; readiness consistent with blockers.
2. **Fault injection.** `kill -9` during append, fetch, push and confirmation (the failpoints'
   `sigkill`, `torn-sigkill`, `exit(N)`); the crash hooks; disk full (`errno(ENOSPC)`); read-only
   store mid-run; clock skew (the `clock` failpoint's `offset`).
3. **Hostile remote.** A clone that force-pushes old or foreign commits to `refs/beads/journal`,
   deletes the ref, or replaces the manifest.
4. **Optional, with the real remote.** `infra/drive.py` and `infra/soak.py`; see their
   docstrings for the setup on the four machines and the reset procedure. Bursts of SSH
   connections to `github.com:22` from the LAN machines have been throttled before; if you see
   `REMOTE_UNREACHABLE` with connect timeouts, stop and report rather than switching endpoints.

## 7. Deliverables

1. **A report** at `reviews/<date>-<n>-<name>.md` (n = the next review number). Findings ranked
   by severity (data loss or divergence > wrong ownership > wrong readiness or conflicts >
   robustness > usability), each with an id of the form `<letter><nn>` using a letter not yet
   used in `FINDINGS.md`, a one-line claim, a reproduction and, where you can, a proposed diff.
   Design observations clearly separated from bugs, and from the accepted decisions in §2.
   Exactly which checks in §5 and §6 you ran, with numbers, and which you did not. Any side
   effects you caused anywhere, even if cleaned up. The `check.sh` output you started from.
2. **Tests**, in a new module `review<n>` of the `beadroll` crate's test binary
   (`crates/beadroll/tests/cli/review<n>.rs`, declared in its `main.rs`), built on
   `beadroll-test-support`: every bug as a failing test, every property you verified as a passing
   one, each test's doc comment naming the finding id. The maintainer folds them into the subject
   modules when the findings are closed; do not edit the existing modules.
3. **Proposed ledger rows** for `FINDINGS.md`, one per finding, at the end of the report.
4. Your scripts, kept runnable, in your scratch directory.
