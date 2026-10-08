# Reviewing `bead`

A brief for an independent reviewer: an agent or a person who did not write this, whose job is
to find out whether the design holds, to break it if they can, and to report with
reproductions. Reviewers do not extend the tool.

Seven reviews have run from earlier versions of this brief. Their reports are kept verbatim in
`reviews/`; every finding and what became of it is in `FINDINGS.md`. Start from what they did
not cover.

## 1. What this is

`beadroll.py` (one Python 3.12+ module, standard library only; `./bead` links to it, and
`uv tool install .` installs it as the `bead` command) is a proof-of-concept issue tracker
for coding agents that work from many git worktrees and several machines at once. It replaces
the workflow around `br` (beads_rust), whose tracker state lives in a file tracked on code
branches: dirty checkouts, tracker-only commits, merge conflicts, state split between worktrees
and machines.

The design under test, in the order it is best read:

- **One journal per machine**, in append-only segment files
  (`$BEAD_HOME/<workspace>/events/<machine>/<segment>.jsonl`). A machine only appends to its own
  journal. A full segment is never rewritten.
- **Every event names the frontier its writer had seen** (`deps`; ids are UUIDv7). Current state
  is the replay of all journals in causal order (`causal_order`, `replay`). Machines merge by
  exchanging files; nothing is text-merged.
- **One store per repository per machine**, outside every working tree (`find_workspace`:
  tracked `.beadroll.json`, else a hash of the normalised origin URL, else a local id). Nothing is
  ever written inside a checkout.
- **Sync through one dedicated git ref** (`refs/beads/journal`) on one coordination remote,
  through a private bare repo (`sync.git`). The pushed history holds a manifest and the
  journals, never code. A push is a plain fast-forward whose parent is the fetched head, so the
  remote refuses it if anyone else published first (`Link.publish`).
- **Claims are persistent and settled online** (`coordinated`). Ownership events are not
  journal records: an *attempt* (the `claim`/`release`/`takeover` plus any edits made in the
  same command) is one file under `events/<machine>/claims/`, written first under `pending/`,
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
- **Identity on every event**: opaque machine id, agent type and thread id (`detect_actor`).
  **Place on every event**: branch and commit of the checkout the command ran in (`where`).
- **Beads JSONL is an import/export format only** (`import_records`, `export_records`).

Read `README.md`, then `bead` top to bottom, then `tests/support.py` and one test module.

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
- The display value for a conflicted field is the newest event id; no resolution command.
- `close` by the owner is a local write published immediately, not a conditional update; a
  close or release that names a claim superseded by a takeover is ignored and listed under
  `stale`, whichever was written first.
- A renumbered child key (`x.1` → `x.2` after two machines created children offline) changes
  under the machine that created it; events refer to issues by immutable id.
- One event with two different payloads is quarantined alone; nothing waits on it.
- The store lock is held while a sync or claim pushes (about a second against GitHub).
- Publishing is serialised on one ref; under heavy contention claims take seconds.
- No compaction beyond segment rotation. No persisted projection (replay is ~80 ms).
- Ancestry over ownership events is a bitset per event (memory ~ events × ownership events / 8);
  a permanent implementation would use a reachability index.
- Exported comment ids for comments made here come from a 48-bit hash range; a collision is a
  one-in-10^14 event and is then resolved by the ids alone.
- `dep cycles` stops after 200 cycles.
- A repository with neither a remote nor `.beadroll.json` gets a local tracker that cannot sync.
- Output is UTF-8 regardless of locale; human output escapes control characters; `--json` always
  wins the representation (`create --silent --json` gives `{"id": …}`); a closed stdout exits 0
  quietly.
- Passwords in remote URLs are dropped, never stored or used; the store is private by its own
  file modes (0700/0600), files written into the checkout follow the caller's umask.
- An import that contains one invalid record or dependency fails whole; nothing is written. A
  status bead does not set itself (`pinned`, a custom one) is kept verbatim, as Beads tools keep
  it; such an issue is neither ready nor done.
- Text from files and stdin is UTF-8 regardless of locale; OS errors are structured (`IO`); a
  timed-out `git` is killed with its process group.
- A command receipt survives a *failed* later command by the same actor and is dropped only when
  a later command by that actor goes through: the remote accepted it, or it was a local write that
  was appended. A recognised retry still reports the claim as it is now.
- A push's outcome is git's own status line (`--porcelain`); a `[remote rejected]` that is the
  remote's verdict (a hook, a rule) is `REMOTE_REFUSED` and is not retried; one the remote could not
  take (`unpacker error`, `missing necessary objects`) is retried after a fresh fetch; a
  remote that has lost objects is `REMOTE_DAMAGED`.
- Format 1 is frozen; changes are additive. An event an older bead cannot apply (a new operation,
  a new field, a newer `v`) is set aside, reported under `integrity`, carried through sync
  untouched and applied after an upgrade; a mixed fleet is safe. A breaking change, should one
  ever be needed, follows writers-lag-readers through the manifest; none is planned.
- Events nest at most 100 levels (`MAX_NESTING`); replay keeps looser type rules than writes
  (`write_problem`) so histories written before a rule mean the same on every machine.
- `BEAD_HOME` must be absolute; the store's modes are 0700/0600 whatever the umask.
- Only directories bead creates get private modes; a user-supplied `BEAD_HOME` is chmodded 0700
  on first write (reviewer observation, kept).
- origin is never remembered as the coordination remote; only a remote named otherwise is. A
  tracked config seeds a new store's remote but never silently moves an existing store's.
- bead's pushes skip the user's client hooks (`--no-verify`); the remote's hooks still apply.
- The remote string reaches git after `--`; a tracked workspace id shaped like a URL-derived id
  that is not this origin's is refused.

## 3. Baseline

Do not take the baseline from a document; run it:

```sh
./check.sh            # all test modules except the slow models, with a scratch BEAD_HOME
./check.sh --stress   # also the randomised multi-machine models (several minutes)
./check.sh --real     # also import/export of the real tracker exports on this Mac
```

`check.sh` prints one line per module and exits non-zero on any failure. Record its output in
your report. The real-infrastructure scripts (four machines, a throwaway GitHub repository) are
under `infra/`; their last results are summarised in `README.md`.

## 4. Rules

- Work in a scratch directory. **Always set `BEAD_HOME`** to a scratch path before running `bead`;
  the default (`~/.local/state/beadroll`) holds the project's own real tracker (this repository
  dogfoods `bead`; its `.beadroll.json` names that workspace) and must not be touched. `tests/__init__.py`
  sets a scratch `BEAD_HOME` for the test modules; do it yourself for anything else.
- Run `bead` only inside repositories you created for the test, or with `BEAD_WORKSPACE` set.
  Never run it inside the real repositories under `~/Documents` (the origin URL would route you
  to a real workspace id). Reading their `.beads/issues.jsonl` files is fine; do not copy their
  contents into any remote.
- Local bare repositories are the default remote. The only real remote you may push to is
  `$BEAD_TEST_REMOTE` (throwaway, private, synthetic data; `gh` is
  authenticated). Do not create other repositories or refs anywhere, and do not push to `main`
  there.
- Do not modify `bead`, `README.md`, `FINDINGS.md`, existing tests or earlier reports. Proposed
  fixes go in your report as unified diffs.
- Do not change SSH, firewall, git or shell configuration on any machine. Do not run `br` or
  `bv` inside real repositories (`br where` writes lock files).
- Running Codex (`codex exec`) or Claude Code sessions costs the owner money; do it only for an
  identity-detection check, at most twice, and say so in the report.
- In-process tests that skip bead's wait between attempts replace `bead.pause`, never the
  global `time.sleep` (`subprocess` polls through it; a callback there deadlocks on the store
  lock under load).
- Report outcomes faithfully: a test you could not run is "not run", not "passed".

## 5. Checklist

Work through these; add your own. For each, say what you did, what you expected, what
happened, and attach the reproduction (command or test).

**Replay and causality**
- Same event set → same projection, conflicts, claims and readiness under every delivery
  order, duplicate delivery, and journal enumeration order (`tests/test_replay.py` permutes
  three to six machines; try more, and thousands of events).
- Version-vector ancestry (`State.saw`) depends on each event depending, directly or not, on
  its predecessor in the same journal, and on bitsets for ownership events. Construct journals
  where that fails (hand-written events) and see what replay does.
- Events whose causes are missing must wait (`pending`), never be treated as concurrent.
- Unknown `op`, unknown `v`, missing fields, non-UUID ids, deps that are not lists: is the
  damage scoped to the event?

**Journal integrity**
- Concurrent writers on one machine (`writing` uses `flock`); readers take no lock and must
  tolerate a torn tail and a half-written attempt file. Kill `-9` a writer at every boundary.
- Segment rotation (`active_segment`, `BEAD_ROTATE_BYTES`) under two local writers.
- Merge rule (`Link.merge`): one side must be a prefix of the other. Own-journal fork error,
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
  receive hook that stalls (see `test_protocol`). Then: a retry of the same command, a
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
repository. `tests/support.py` (`Sandbox`, `Machine`) shows the pattern; `tests/test_stress.py`
has two randomised models to extend.

1. **Randomised model check.** 6 machines, 2,000+ operations drawn from the real distribution
   (show 32%, update 12%, list 9%, ready 6%, close 5%, create 5%, dep 5%, comments 4%,
   claim/release/takeover, sync every few ops), with random partitions. Keep a reference model
   of ownership from the commands that were *accepted online*, and assert after every full sync
   round: identical exports and per-issue `show` everywhere; at most one confirmed owner per
   issue, equal to the model; every accepted claim visible everywhere; no `waiting_events`,
   `integrity` entries or `forked_journals`; readiness consistent with blockers.
2. **Fault injection.** `kill -9` during append, fetch, push and confirmation; the crash hooks;
   disk full; read-only store mid-run; clock skew (offset `time.time_ns`).
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
2. **Tests**, in a new module `tests/test_review<n>.py` that imports `tests.support`: every bug
   as a failing test, every property you verified as a passing one, each test's docstring
   naming the finding id. The maintainer folds them into the subject modules when the findings
   are closed; do not edit the existing modules.
3. **Proposed ledger rows** for `FINDINGS.md`, one per finding, at the end of the report.
4. Your scripts, kept runnable, in your scratch directory.
