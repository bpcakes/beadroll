//! The texts bead prints verbatim, copied from Python's `cli.py`: the overview (`OVERVIEW`), shown
//! for `bead`, `bead -h` and `bead --help`; the workflow `bead prime` prints (`PRIME`, its
//! `{context}` filled in); and the snippet `bead onboard` prints for an agent instructions file
//! (`ONBOARD`), which this repository's `AGENTS.md` holds.

/// The overview of every command.
pub(crate) const OVERVIEW: &str = "\
usage: bead [--json] [--actor NAME] COMMAND [ARGS...]

bead: a Beads-compatible issue tracker for agent crews working across worktrees and machines.
State lives outside the checkout, sync goes through a git ref, and a claim is exclusive once
the remote accepts it. Every event records machine, agent, thread, branch and commit.

Find work      ready [ID]                    unblocked open issues nobody holds; ID: only its subtree
               list [-s STATUS] [-a]         issues (open ones by default); --parent, -l, -t, -p filters
               show ID...                    full detail: body, acceptance criteria, deps, comments, claim
               search TEXT   blocked   dep tree ID   dep cycles
Do work        claim ID...                   take ownership (or: update ID --status in_progress)
               comments add ID TEXT          record progress and decisions
               update ID [--title T] [--notes N] [--add-label L] [--parent P] [--status S]
               close ID -r REASON            finish: ends your claim and publishes at once
               release ID   takeover ID -r REASON   reopen ID   delete ID -r REASON
Shape work     create TITLE [-t TYPE] [-p 0-4] [--parent P] [--deps blocks:ID] [-l LABEL]
               dep add A B [--type blocks|parent-child]   dep remove A B   lint
Coordinate     sync                          exchange journals with the other machines
               claims                        who holds what, on which branch, since when
               log ID   info   machines
Agents         prime                         the workflow to load at the start of a session
               onboard                       a snippet for CLAUDE.md / AGENTS.md
Data           init [--prefix P] [--tracked] [--import FILE]   import FILE   export [-o FILE]

Flags          --json anywhere for machine output (errors: {\"error\":{\"code\",\"message\"}} on stderr)
               --actor NAME to name yourself; --offline for a provisional claim; -- ends options
Help           bead help COMMAND, or bead COMMAND --help; bead --version
";

/// The workflow an agent loads at the start of a session; `{context}` is where it stands.
pub(crate) const PRIME: &str = r#"beadroll workflow: read once per session, then act.
{context}
1. Find work     bead ready [ID] --json     unblocked issues nobody holds (ID: only its subtree)
                 bead show ID --json        description, acceptance criteria, deps, comments, who holds it
                 bead claims                what others hold, on which branch, since when
2. Take it       bead claim ID              exclusive once the remote accepts it; ALREADY_CLAIMED: pick another
                                            (bead update ID --status in_progress does the same)
3. Work          bead comments add ID "..." progress and decisions, for the next session and other agents
                 bead update ID --notes "..." | --add-label L | --parent P
                 bead create "title" --parent ID -t task     split off follow-ups
4. Finish        bead close ID -r "what was done"   ends your claim and publishes at once
                 bead release ID                    hand it back unfinished
                 bead sync                          catch up: after --offline, after "not yet published", after a long gap

Rules  Ownership belongs to your agent thread; a new session is a new owner. Continue your own
       earlier work with bead takeover ID -r "same task, new session". Never --force or take over
       someone else's work without a reason you have verified.
       A conflict (two writers, same field) is listed under "conflicts" in show; a write that has
       seen both sides settles it. --offline claims are provisional, never exclusive.
       Use --json for machine output. Error codes: ALREADY_CLAIMED, CLAIMED_BY_OTHER, NOT_OWNER,
       STATUS_CONFLICT, CYCLE, VALIDATION, ISSUE_NOT_FOUND, REMOTE_UNREACHABLE (nothing is lost;
       retry later), REMOTE_REFUSED (the remote refuses bead's ref: tell a human; retrying will not help),
       REMOTE_DAMAGED (the remote lost objects: tell a human; retrying will not help),
       SYNC_REPO_DAMAGED (this machine's sync repository is damaged again after a rebuild: delete
       it, nothing in it is the only copy of anything, and run the command again),
       UNRESOLVED (an attempt of yours awaits the remote: run bead sync), SYNC_CONFLICT,
       IO (the store or a file could not be read or written; nothing half-done).
"#;

/// A snippet for an agent instructions file (`CLAUDE.md`, `AGENTS.md`).
pub(crate) const ONBOARD: &str = r#"## Work tracking (bead)

This repository tracks its work with `bead` (beadroll): one tracker shared by every worktree,
clone and machine of this repository, stored outside the working tree, never on a code branch.

- Start every session with `bead prime` and follow it. It prints the current workflow, who you
  are and what you hold; it is the source of truth, not this file.
- Find work with `bead ready`, claim an issue before working on it, record progress and decisions
  with `bead comments add`, split follow-ups with `bead create "title" --parent ID`, and close with
  `bead close ID -r "what was done"`. Every write is published at once; when a command says
  "not yet published" (`--offline`, or the remote did not answer), run `bead sync` before you stop.
- A claim belongs to your agent thread and lasts until release, close or takeover. A new session
  continues its own earlier work with `bead takeover ID -r "same task, new session"`; never take
  over someone else's work without a reason you have verified.
- Add `--json` for machine-readable output. Errors are structured; `bead prime` lists the codes
  and what to do about each.
- If a `.beads/` directory is still here, it is history: do not edit it or run `br`/`bd` in this
  repository. `bead export` writes Beads JSONL when another tool needs it.

<!-- Project specifics go here: the issue prefix and the epics to start from, what "done" means
     (tests, docs, merged where), and the branch or PR conventions a close reason should name. -->
"#;
