//! The texts bead prints verbatim: the overview (Python's `OVERVIEW`), shown for `bead`, `bead
//! -h` and `bead --help`.

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
