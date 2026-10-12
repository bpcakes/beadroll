//! The texts bead prints: the overview (`OVERVIEW`) for `bead`, `bead -h` and `bead --help`; the
//! workflow (`PRIME_TITLE` and `PRIME`, with where the caller stands between them) for `bead
//! prime`; the ends of the pages of `create`, `dep` and `comments`; all laid out for the reader's
//! width when printed (`layout`). And the agent instructions snippet (`ONBOARD`) for `bead
//! onboard`, verbatim: it is also this repository's `AGENTS.md`.

use super::layout::{BLANK, Block, Row, row};

/// The overview of every command.
pub(crate) const OVERVIEW: &[Block] = &[
    Block::Text("Usage: bead [--json] [--actor NAME] [--color WHEN] COMMAND [ARGS...]"),
    Block::Text(
        "bead: a Beads-compatible issue tracker for agent crews working across worktrees and \
         machines. State lives outside the checkout, sync goes through a git ref, and a claim is \
         exclusive once the remote accepts it. Every event records machine, agent, thread, \
         branch and commit.",
    ),
    Block::Rows(COMMANDS),
];

/// The overview's commands, with the flags every command takes.
const COMMANDS: &[Row] = &[
    row(
        "Find work",
        "ready [ID]",
        "unblocked open issues nobody holds; ID: only its subtree",
    ),
    row(
        "",
        "list [-s STATUS] [-a]",
        "issues (open ones by default); --parent, -l, -t, -p filters",
    ),
    row(
        "",
        "show ID...",
        "full detail: body, acceptance criteria, deps, comments, claim",
    ),
    row("", "search TEXT   blocked   dep tree ID   dep cycles", ""),
    row(
        "Do work",
        "claim ID...",
        "take ownership (or: update ID --status in_progress)",
    ),
    row("", "comments add ID TEXT", "record progress and decisions"),
    row(
        "",
        "update ID [--title T] [--notes N] [--add-label L] [--parent P] [--status S]",
        "",
    ),
    row(
        "",
        "close ID -r REASON",
        "finish: ends your claim and publishes at once",
    ),
    row(
        "",
        "release ID   takeover ID -r REASON   reopen ID   delete ID -r REASON",
        "",
    ),
    row(
        "Shape work",
        "create TITLE [-t TYPE] [-p 0-4] [--parent P] [--deps blocks:ID] [-l LABEL]",
        "",
    ),
    row(
        "",
        "dep add A B [--type blocks|parent-child]   dep remove A B   lint",
        "",
    ),
    row(
        "Coordinate",
        "sync",
        "exchange journals with the other machines",
    ),
    row("", "claims", "who holds what, on which branch, since when"),
    row("", "log ID   info   machines", ""),
    row(
        "Agents",
        "prime",
        "the workflow to load at the start of a session",
    ),
    row("", "onboard", "a snippet for CLAUDE.md / AGENTS.md"),
    row(
        "Data",
        "init [--prefix P] [--tracked] [--import FILE]   import FILE   export [-o FILE]",
        "",
    ),
    BLANK,
    row(
        "Flags",
        "--json",
        "anywhere, for machine output; errors are {\"error\":{\"code\",\"message\"}} on stderr",
    ),
    row("", "--actor NAME", "to name yourself"),
    row(
        "",
        "--color WHEN",
        "auto (the default: on a terminal, unless NO_COLOR), always or never",
    ),
    row(
        "",
        "--offline",
        "record locally only; a claim made so is provisional",
    ),
    row("", "--", "ends options"),
    row(
        "Help",
        "help COMMAND",
        "one command's page, as bead COMMAND --help shows it",
    ),
    row("", "--version", "this bead's version"),
];

/// The first line of `bead prime`; where the caller stands follows it.
pub(crate) const PRIME_TITLE: &str = "beadroll workflow: read once per session, then act.";

/// The workflow an agent loads at the start of a session, after where it stands.
pub(crate) const PRIME: &[Block] = &[Block::Rows(STEPS), Block::Rows(RULES)];

/// The workflow's steps.
const STEPS: &[Row] = &[
    row(
        "1. Find work",
        "bead ready [ID] --json",
        "unblocked issues nobody holds (ID: only its subtree)",
    ),
    row(
        "",
        "bead show ID --json",
        "description, acceptance criteria, deps, comments, who holds it",
    ),
    row(
        "",
        "bead claims",
        "what others hold, on which branch, since when",
    ),
    row(
        "2. Take it",
        "bead claim ID",
        "exclusive once the remote accepts it; ALREADY_CLAIMED: pick another (bead update ID \
         --status in_progress does the same)",
    ),
    row(
        "3. Work",
        "bead comments add ID \"...\"",
        "progress and decisions, for the next session and other agents",
    ),
    row(
        "",
        "bead update ID --notes \"...\" | --add-label L | --parent P",
        "",
    ),
    row(
        "",
        "bead create \"title\" --parent ID -t task",
        "split off follow-ups",
    ),
    row(
        "4. Finish",
        "bead close ID -r \"what was done\"",
        "ends your claim and publishes at once",
    ),
    row("", "bead release ID", "hand it back unfinished"),
    row(
        "",
        "bead sync",
        "catch up: after --offline, after \"not yet published\", after a long gap",
    ),
];

/// The rules the steps keep, and the error codes.
const RULES: &[Row] = &[
    row(
        "Rules",
        "",
        "Ownership belongs to your agent thread; a new session is a new owner. Continue your own \
         earlier work with bead takeover ID -r \"same task, new session\". Never --force or take \
         over someone else's work without a reason you have verified.",
    ),
    row(
        "",
        "",
        "A conflict (two writers, same field) is listed under \"conflicts\" in show; a write that \
         has seen both sides settles it. --offline claims are provisional, never exclusive.",
    ),
    row(
        "",
        "",
        "Use --json for machine output. Error codes: ALREADY_CLAIMED, CLAIMED_BY_OTHER, NOT_OWNER, \
         STATUS_CONFLICT, CYCLE, VALIDATION, ISSUE_NOT_FOUND, REMOTE_UNREACHABLE (nothing is lost; \
         retry later), REMOTE_REFUSED (the remote refuses bead's ref: tell a human; retrying will \
         not help), REMOTE_DAMAGED (the remote lost objects: tell a human; retrying will not \
         help), SYNC_REPO_DAMAGED (this machine's sync repository is damaged again after a \
         rebuild: delete it, nothing in it is the only copy of anything, and run the command \
         again), UNRESOLVED (an attempt of yours awaits the remote: run bead sync), \
         SYNC_CONFLICT, IO (the store or a file could not be read or written; nothing \
         half-done).",
    ),
];

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

/// The end of `bead create --help`: what a new issue is, and what `--deps` means.
pub(crate) const CREATE_EPILOG: &[Block] = &[
    Block::Text("A new issue is an open task at priority 2 unless -s, -t or -p say otherwise."),
    Block::Text(
        "--deps names the issues the new one depends on. ISSUE (or blocks:ISSUE) keeps it from \
         being ready until ISSUE is done: closed or deleted. Another type, such as \
         related:ISSUE or discovered-from:ISSUE, records a link that blocks nothing.",
    ),
];

/// The end of `bead dep --help`: its forms, and what each type of dependency does.
pub(crate) const DEP_EPILOG: &[Block] = &[
    Block::Rows(&[
        row(
            "forms:",
            "dep add A B",
            "A depends on B; --type TYPE, blocks by default",
        ),
        row(
            "",
            "dep remove A B",
            "take that back; with --type, only that type",
        ),
        row(
            "",
            "dep [list] A",
            "what A depends on, and what depends on A",
        ),
        row(
            "",
            "dep tree A",
            "what A depends on, what that depends on, and so on",
        ),
        row(
            "",
            "dep cycles",
            "dependency cycles; --blocking-only: blocks only",
        ),
    ]),
    Block::Text(
        "types: blocks keeps A from being ready until B is done (closed or deleted); \
         parent-child makes A a child of B: what holds B holds A, and an epic B waits for A; any \
         other word, such as related, records a link that blocks nothing.",
    ),
];

/// The end of `bead comments --help`: its forms.
pub(crate) const COMMENTS_EPILOG: &[Block] = &[Block::Rows(&[
    row("forms:", "comments [list] ISSUE", "its comments"),
    row(
        "",
        "comments add ISSUE TEXT ...",
        "record one; or its text from -m or -f",
    ),
])];
