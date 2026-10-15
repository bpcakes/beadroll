# Review 16: the performance work of roll-lqjj.15 to roll-lqjj.20

A Claude (Opus 5.5) review of `4d7e03e..8683332`, asked for by the maintainer on 2026-10-15 as a
follow-up to that range: whether there is anything to improve, deeper (the same areas), wider
(where a command's time goes now) and more ambitious (structure). The range made import planning,
the projection's finishing pass, waiting imports, readiness and the dependents lookup cost what
they touch rather than the square of it (Z01 to Z05), and made the caches trust a file only once
it was settled (Y01, M01, M02). Its findings are X01 to X17 in `../FINDINGS.md`; each fix is a
child of `roll-lqjj` with a pull request of its own (roll-lqjj.24 to roll-lqjj.34), and the two
design findings are roll-lqjj.35 and roll-lqjj.36.

## How it was done

Every changed source file of the range was read, and the code each change calls. Four tracks ran
beside the main reading, each in a scratch clone of 8683332: a sweep of every command and what it
calls for work that grows faster than the tracker; `bead show`'s `merged`; `bead blocked` and the
details of several issues on deep hierarchies; and an adversarial pass over the range's new code
looking for bugs. Measurements are release builds on Linux 7.1.5, x86_64, 64 cores, ext4 on NVMe,
Rust 1.99.0, the fastest of three to five runs unless a median is given; other processes ran on the
machine at the same time, so figures compared with each other were taken back to back. In-process
probes (`decode_state` by phase, a write cycle by phase, the listing sort) ran as ignored tests of
xtask in a scratch clone; end-to-end runs used `cargo xtask timing --read N` with
`BEAD_UNDER_TEST`, and scripts that build trackers by `bead init`, `bead import` and `bead close`
in scratch repositories with a scratch `BEAD_HOME`. Nothing in the repository was modified and
nothing was pushed by the review itself; every `bead` ran in a scratch store.

What ran: the workspace suite (`cargo nextest run --workspace`) on 8683332 with the prototype of
X05, X10 and X12 applied, 905 passed and 3 skipped; `beadroll-core` and `beadroll-store` (226)
in the adversarial track; `beadroll-core` (170), `beadroll` (541 passed, 3 skipped) and `xtask`
(58) with the prototype of X01 and X02; the workspace suite (906) with the prototype of X08; and
`beadroll-core` (169) with the prototype of X03. `./check.sh` itself was not run by the review.

## The range itself

No correctness bug was found. Checked and found sound:

- The awaiting index (`state/awaiting.rs`, `replay/apply/create.rs`, removal by key): binding by
  key only, since kept entries name keys `by_key` lacks or their own issue's; one key per create,
  entries taken in number order; `numbers` aligned with `dangling` through take, settle and decode;
  every behaviour of the scan it replaced, taken-back entries and renames included. Both
  projection harnesses ran at 600 seeds of 80 to 90 steps, reading back half the time.
- `finish_some`: of every operation, only the label operations and a create change labels, and only
  a create and the two dependency operations change edges; settling covers the issues a bind
  outside the batch touched.
- The projection's ids as a set: encoded ascending, refused when not.
- `readiness()`, `is_blocked` and `open_blockers` agree on 40,000 random graphs, self-parents,
  edges to non-issues, removed edges, pinned, disputed and done issues and epics in cycles included;
  `EventId::parse` accepts only the canonical lowercase form, so the children index keyed by text
  cannot disagree with lookups that parse.
- `dependents_of` agrees with `detail` on 40,000 random graphs; `found_in`'s pointer equality is
  sound because the borrow pins the state.
- Import planning: the first record of a key wins as `position` did; the set of listed pairs, the
  empty-list skip and the shared `resolves` are equivalent.
- The cache rule: memo and projection take their own moment from one open file; `settled_before`
  needs both times older than the cache's, so `touch -d`, `cp -p` and `rsync -t` are caught by the
  change time; every identity a cache trusts passes the rule or a sum.

What Z02 left, the parent-child index built again for any batch that changes an edge, costs
0.4 ms at 200,000 events and is not worth more work.

## Findings

Ranked as `REVIEWING.md` §7 ranks them; within a rank, by what a user meets first.

### Robustness

**X14. A `where.commit` from any machine reaches git unchecked.** `merged_into_default`
(`crates/beadroll-store/src/place.rs:43`) gives the text of the event's `where.commit` to `git
merge-base --is-ancestor` as an argument. A commit text holding a NUL fails `bead show` of that
issue, alone or among others, with `{"error": {"code": "IO", "message": "nul byte found in
provided data: git"}}` (reproduced by appending such a close to a scratch journal); one starting
with `-` is read as an option (exit 129, so `unknown` today, harmless only because `merge-base`
has no dangerous option); a revision expression is evaluated: `HEAD~1` answers merged, `:/regex`
searches every commit message. Validation only asks for a string. bead itself records `git
rev-parse HEAD`.

**X13. A claims listing taken before the segments can be used after them.** `current()` lists the
claims directories (`crates/beadroll-store/src/projection.rs:159`) before any segment is read;
`appended()` (`:410`) and `load_listed` list them again "once the segments are read", through the
shared listing, which reuses the first while the directory keeps its length and times. Y01's
reasoning, that a file another process adds within one tick of a listing needs nothing, holds for
one listing, not for this order: a record appended after an attempt file landed in that tick is
then replayed without its cause and is pending for one read (an unlocked reader then saves a
projection that is not open, and the next command replays). Shown with a simulated same-tick
listing; not reproduced on Linux 7.1, whose multigrain timestamps moved every identity in 20,000
tries: kernels before 6.13 are where it can happen. Locked phases are safe: taking the lock clears
the listing.

### Cost (optimized: the behaviour stays, the work goes)

**X04. One integrity entry switches the projection off for good.** `Projection::extend` declines
whenever the state holds any integrity entry (`crates/beadroll-core/src/projection.rs:116`,
`projection/extend.rs:240`). A newer bead's operation during a rolling upgrade, which `REVIEWING.md`
§2 calls safe, a quarantined duplicate or a copied machine identity therefore makes every later
write replay the whole journal in the next command. Worse, `close`, `reopen` and `delete` read
`tx.state()` after every event they emit (`crates/beadroll/src/commands/transition.rs:73`), and
`Tx::state` replays the journal again each time once the projection is gone (`tx.rs:199`): with one
`future_op` event in a journal of its own, a `close` of 60 issues of 4,000 took 10.26 s against
0.068 s on the same tracker without it, and of 20 issues of 16,000 17.1 s; every later `list` after
a write 0.289 s against about 0.08 s. An experiment letting the projection extend whenever nothing
is pending, with unknown operations and invalid payloads added to the differential test's
generator, matched replay on 13,088 extensions over 400 seeds, and the same `close` took 0.074 s;
the one test that fails pins the current rule. Quarantined duplicates, chain breaks, unreadable
lines and refused takeovers were not in that generator.

**X01. `bead blocked` builds each held issue's list by walking its ancestry.**
`crates/beadroll/src/commands/queries.rs:28` calls `State::open_blockers` for every held issue, so a
deep hierarchy costs issues times depth. Z04 judged readiness once but left the lists where they
are printed. A held chain of 4,000 took 2.05 s and of 16,000 43.2 s; making every list at once
(groups of issues that reach each other up not-done parents, found by Tarjan's algorithm, each
group's set made from the groups above it and shared where it adds nothing) took 0.062 s and
0.256 s, printing the same bytes; in process the lists of a chain of 16,000 took 35.8 s one by one
and 9.8 ms at once, and of a lattice whose rows list 64 gates each (1,017,600 entries) 43 ms.

**X02. The details of several issues judge `blocked` one by one.** `State::detail_with`
(`crates/beadroll-core/src/view.rs:146`) asks `State::is_blocked`, which walks the issue's
ancestors, so `show`, `update`, `claim` and `takeover` of many issues in a hierarchy cost issues
times depth; Z05's probe used a graph without parents and could not see it. `bead show` of every
issue of a held chain of 4,000 took 1.48 s and of 16,000 28.7 s; with one `Readiness` for the
batch, 0.146 s and 0.617 s.

**X08. `bead show` starts about five gits for every closed issue.** For each closed issue with a
recorded commit, `merged_into_default` resolves the default branch again (`symbolic-ref`, two
`rev-parse --verify`) and asks both refs with `merge-base --is-ancestor` even after one said yes.
On a tracker of 2,303 issues: one closed issue 84.5 ms (8 gits), 100 closed on distinct commits
4.1 s (503 gits), 1,000 29.8 s (5,003), 300 merged and 300 unmerged 17.4 s. Resolved once a
command, each distinct commit asked once, and hex commits asked together (one `cat-file
--batch-check` and one `rev-list --stdin` a ref, a commit every ref lists asked again alone, since
`rev-list` stops early by commit date and can misjudge under clock skew): 64.9 ms, 80 ms (9 gits),
131 ms (9) and 1.59 s, every answer the same.

**X05. The projection and its text file are fsynced like the journal.** `write_atomic` always
fsyncs (`crates/beadroll-store/src/fs.rs:91`), and the text file's appends are fsynced too, so the
command after every write flushes two caches that are read only against the sums kept of them and
rebuilt whenever missing, cut or damaged. Interleaved, 30 runs each, a write and then a `show`:
the `show` took 39.1 ms (median) on 300 issues and 9.5 ms with both unflushed, 74.4 ms and 42.1 ms
on 5,000; the writes themselves did not move. An online write fsyncs six files, four of them caches
(the text file, the projection, the journal memo twice: 12 to 21 ms each here, one 848 ms under
load). Plain fsync is cheap on APFS, so the gain is Linux's.

**X06. An online write decodes the projection four or five times, and four commands clone the
state.** `Tx::new`; the command's answer; `synchronise`'s `known_renames`, a decode to count renames
(`crates/beadroll-sync/src/synchronise.rs:136`); `Link::merge`'s check of its own chain
(`link/merge.rs:117`); and `synchronise`'s final view for the `Synced` report, which `publish_to`
drops (`synchronise.rs:427`). `update`, `dep`, `claim` and `release` clone the whole `State` to read
it while they emit (`commands/update.rs:169`, `dep.rs:168`, `ownership.rs:59` and `:257`: 49 ms at
20,000 issues). On 20,000 imported issues an online `comments add` took 1.10 s against 0.30 s
offline, an online `update` 1.45 s with five reads of the projection, and a sync that found nothing
new 0.60 s with three. The first decode in a process costs about three times a warm one (30,662
page faults for a `list --limit 1`).

**X07. Every attempt file is paid for by every command, several times.** Attempt files are never
compacted (a decision this review keeps), and a command pays for each: `held()` stats it in
`current()` and again in `appended()`, the link stats it for the memo and the blobs, `meta.json`'s
`published` map (one entry a file) is parsed two to four times and cloned, and the memo once more.
On the store of `timing --read 200000 --ownership` (91,952 attempt files, 14,578 issues; `meta.json`
7.0 MB, memo 15.6 MB, projection 31.3 MB): `list` 1.04 s against 0.19 s for the plain history, an
online write 6.1 s, a sync that finds nothing new 4.5 s; 184,190 statx for a `show` after a write,
461,078 for the sync, 645,148 for the online write. At 20,000 events with 9,132 attempt files,
`list` takes 0.097 s against 0.024 s. The stores on this machine hold 2 to 15% as many attempt files
as journal lines.

**X03. Replay builds every key once for each renamed copy.** `State::free_key`
(`crates/beadroll-core/src/state.rs:275`) collects `claimed_keys()`, every issue's key and every
key an import waits for, for each copy two machines minted alike. Two machines importing the same
export offline and then syncing rename every copy: at 8,000 copies the second machine's sync took
6.57 s and a cold replay 4.74 s, against 1.15 s for the same creates with nothing renamed, the
excess growing about four times a doubling. Asking `by_key` and the awaiting index (roll-lqjj.18)
of each candidate instead: 2.71 s and 1.12 s, the keys the same.

**X09. `log`, `info` and `machines` load the whole journal, and `log` replays it.**
`crates/beadroll/src/commands/history.rs:29-32` loads, clones and replays the journal to name keys;
`info.rs:24` loads it for counts and the frontier; `history.rs:132` for each machine's totals. On
20,000 issues: `log ex-5 --limit 1` 1.47 s, `info` 1.24 s, `machines` 0.91 s, at 212 MB resident,
where `list --limit 1` takes 0.26 s at 110 MB.

**X10. Every command frees its whole state before it exits, and a write frees its projection
under the store lock.** At 200,000 events a reading command decodes about 100 ms of state and frees
it in 50 to 85 ms just before the process ends; kept until exit, `list` took 0.154 s instead of
0.205 s, `show` 0.154 s instead of 0.203 s and `ready` 0.208 s instead of 0.248 s. `writing()`
(`crates/beadroll-sync/src/tx.rs:252`) frees the transaction, which holds the whole projection
(83 ms to free at 200,000 events), inside `locked()`, where other commands wait for the lock.

**X11. Discovery's third git waits for the other two.** Every command starts `git config
--get-regexp ^remote\.origin\.(push)?url$` and the combined `rev-parse` together, then `git
cat-file --batch` for the committed `.beadroll.json` (`crates/beadroll-store/src/discover.rs:56`)
once both have answered; a write then adds `rev-parse HEAD --abbrev-ref HEAD`. A git process costs
1.2 to 1.6 ms here, `bead --version` 0.8 ms, and `bead list` on a tracker of one issue 6.2 ms.

**X12. The listing sort allocates a string for each comparison.** `sort_for_listing`
(`crates/beadroll-core/src/query.rs:303`) copies each issue's creation time to compare it: 4.28 ms
for 19,939 issues, 3.10 ms comparing borrowed text.

### Process

**X15. Nothing guards the work Z01 to Z05 removed.** Their tests compare answers and timings never
gate, so a change that makes import planning, `finish_some`, the awaiting index, readiness or the
dependents lookup quadratic again passes every test.

### Design (no pull request yet)

**X16. A reading command decodes the whole state, whatever it prints.** At 200,000 events (19,939
issues, a 19.4 MB projection: state 14.0 MB, index 5.4 MB, texts 2.0 MB), `decode_state` takes
about 101 ms: the issues most of it, the named events' records about 24 ms, relabelling every issue
5 ms, the children index 0.4 ms; freeing it 85 ms. A decoded issue keeps each short field twice,
in its fields and its register (title, priority, assignee, status), each dependency target as a
36-byte uuid string with a metadata object of two strings, and the state section carries every named
event's causes, which only extension reads. An outline part for what `list`, `ready`, `blocked` and
`prime` read, typed edge targets with their kinds held once, and short fields shown from their
registers as long fields are (roll-lqjj.10) would let a read cost what it prints. Related:
roll-lqjj.21.

**X17. Every write cycle re-encodes and rewrites the whole projection.** At 200,000 events a write
cycle decodes the projection with its index (136 ms), applies the new record (0.3 ms: Z02's work),
encodes it all (96 ms) and writes 19 MB, the ids of every event (3.2 MB) and the knowledge among
them, then frees it (83 ms).

## Where the time goes now

`cargo xtask timing --read N`, 8683332:

| events | list | show | ready | local write |
|---|---|---|---|---|
| 1,000 | 7.9 ms | 7.7 ms | 7.9 ms | 25-38 ms |
| 20,000 | 23 ms | 23 ms | 26 ms | 47-61 ms |
| 200,000 | 0.205 s | 0.203 s | 0.248 s | 0.283-0.297 s |

On a small tracker a command's time is its processes and its flushes: three gits for every command
and a fourth for a write (X11), one fsync for the journal and two more for the caches in the next
command (X05). At 200,000 events it is the projection: about 100 ms to decode, 50 to 85 ms to free
(X10, X16), and for a write a decode with the index, an encode and a rewrite of 19 MB (X17). The
cliffs are elsewhere: an integrity entry (X04), many attempt files (X07), closed issues shown
together (X08), deep hierarchies (X01, X02).

## Side effects

Scratch clones, stores and repositories under the session's temporary directory and
`~/.cache/beadroll-review16`; the issues roll-lqjj.23 to roll-lqjj.36 in this repository's tracker;
and, for the fixes, one branch and pull request each.
