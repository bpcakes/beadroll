"""The command line: parser, dispatch, the texts an agent reads."""
from __future__ import annotations

import argparse
import contextlib
import json
import os
import sys

from .base import (BeadError, FORMAT, __version__, emit, silence, visible)
from .machine import (actor_label, detect_actor, find_workspace, replica_id)
from .model import (conflicts, view)
from .sync import (publish_soon, unpublished_count)
from .queries import (is_blocked)
from .commands import (
    cmd_blocked, cmd_claim, cmd_claims, cmd_comments, cmd_create, cmd_dep,
    cmd_export, cmd_import, cmd_info, cmd_init, cmd_lint, cmd_list,
    cmd_log, cmd_machines, cmd_ready, cmd_release, cmd_search, cmd_show,
    cmd_sync, cmd_takeover, cmd_update, need_tracker, transition,
)


OVERVIEW = """\
usage: bead [--json] [--actor NAME] COMMAND [ARGS...]

bead: a Beads-compatible issue tracker for agent crews working across worktrees and machines.
State lives outside the checkout, sync goes through a git ref, and a claim is exclusive once
the remote accepts it. Every event records machine, agent, thread, branch and commit.

Find work      ready [--epic ID]             open issues with nothing blocking them and no owner
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

Flags          --json anywhere for machine output (errors: {"error":{"code","message"}} on stderr)
               --actor NAME to name yourself; --offline for a provisional claim; -- ends options
Help           bead help COMMAND, or bead COMMAND --help; bead --version
"""


PRIME = """\
beadroll workflow: read once per session, then act.
{context}
1. Find work     bead ready --json          unblocked issues nobody holds (--epic ID to scope)
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
"""


ONBOARD = """\
## Work tracking (bead)

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
"""


def cmd_help(args, ws, actor):
    """The overview, or one command's usage."""
    if not args.command_name:
        return {"help": OVERVIEW}, OVERVIEW.rstrip("\n").split("\n")
    parser = build_parser()
    choices = next(a for a in parser._actions if isinstance(a, argparse._SubParsersAction)).choices
    if args.command_name not in choices:
        raise BeadError("USAGE", f"unknown command {args.command_name!r}; try `bead help`")
    text = choices[args.command_name].format_help()
    return {"help": text}, text.rstrip("\n").split("\n")


def cmd_prime(args, ws, actor):
    """What an agent needs to know to work here, with where things stand right now."""
    replica = replica_id(create=False)
    me = actor_label(actor, replica or "?")
    context = [f"You are {me} (machine, agent, thread)."]
    if ws is not None:
        try:
            st = view(ws)
            need_tracker(st)
            held = [i for i in st.issues.values() if i["claim"] and i["claim"]["owner"] == me]
            ready = [i for i in st.issues.values() if i["status"] == "open" and not i["claim"]
                     and not is_blocked(st, i) and "status" not in conflicts(st, i)]
            waiting = sum(len(i["unconfirmed"]) for i in st.issues.values())
            context.append(f"Tracker {st.prefix!r}: {len(ready)} ready, {len(held)} held by you"
                           + (f" ({', '.join(i['id'] for i in held)})" if held else "")
                           + (f", {waiting} attempt(s) of this machine awaiting the remote" if waiting else "")
                           + f". Last sync: {ws.meta().get('last_sync') or 'never'}.")
        except BeadError as error:
            context.append(f"No tracker here yet ({error.code}): run `bead init` or `bead sync`.")
    else:
        context.append("Not inside a git repository: the commands below work in any checkout of a tracked repo.")
    text = PRIME.format(context="\n".join(context) + "\n")
    return {"prime": text, "actor": me}, text.rstrip("\n").split("\n")


def cmd_onboard(args, ws, actor):
    """A snippet for the repository's agent instructions file."""
    return {"onboard": ONBOARD}, ONBOARD.rstrip("\n").split("\n")


class Parser(argparse.ArgumentParser):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.color = False  # Python 3.14 colours help on a terminal or under FORCE_COLOR; bead's help is plain text

    def error(self, message):
        raise BeadError("USAGE", message)


def build_parser() -> argparse.ArgumentParser:
    parser = Parser(prog="bead", description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True, parser_class=Parser)

    def add(name, func, help_text):
        cmd = sub.add_parser(name, help=help_text)
        cmd.set_defaults(func=func)
        return cmd

    def coordination(cmd):
        cmd.add_argument("--offline", action="store_true",
                         help="do not contact the remote; a claim made this way is provisional")
        cmd.add_argument("--remote", help="coordination remote (name or URL) for this call")

    def content_flags(cmd):
        cmd.add_argument("-d", "--description")
        cmd.add_argument("--description-file", dest="description_file")
        cmd.add_argument("--acceptance-criteria", "--acceptance", dest="acceptance_criteria")
        cmd.add_argument("--notes")
        cmd.add_argument("--design")
        cmd.add_argument("-p", "--priority")
        cmd.add_argument("-t", "--type")
        cmd.add_argument("--status", "-s")
        cmd.add_argument("--assignee")
        cmd.add_argument("--estimate", type=int)
        cmd.add_argument("--external-ref", dest="external_ref")
        cmd.add_argument("--parent")

    cmd = add("init", cmd_init, "create the tracker for this repository")
    cmd.add_argument("--prefix")
    cmd.add_argument("--import", dest="import_path", metavar="BEADS_JSONL")
    cmd.add_argument("--tracked", action="store_true", help="write .beadroll.json with an explicit workspace id")
    cmd.add_argument("--remote", help="coordination remote (name or URL) if it is not origin")
    cmd = add("import", cmd_import, "import a Beads JSONL export")
    cmd.add_argument("path")
    cmd = add("export", cmd_export, "write Beads-compatible JSONL")
    cmd.add_argument("-o", "--output")

    cmd = add("create", cmd_create, "create an issue")
    cmd.add_argument("title_pos", nargs="?", metavar="TITLE")
    cmd.add_argument("--title")
    content_flags(cmd)
    cmd.add_argument("-l", "--labels", "--label", action="append")
    cmd.add_argument("--deps", action="append", help="comma list of [type:]ISSUE")
    cmd.add_argument("--silent", action="store_true", help="print only the new id")

    cmd = add("update", cmd_update, "change fields, labels or parent; --claim or --status in_progress claims")
    cmd.add_argument("ids", nargs="+")
    cmd.add_argument("--title")
    content_flags(cmd)
    cmd.add_argument("--claim", action="store_true")
    cmd.add_argument("--force", action="store_true", help="change the status of an issue someone else has claimed")
    cmd.add_argument("--add-label", action="append")
    cmd.add_argument("--remove-label", action="append")
    cmd.add_argument("--set-labels")
    coordination(cmd)

    cmd = add("claim", cmd_claim, "take ownership; it lasts until release, close or takeover")
    cmd.add_argument("ids", nargs="+")
    coordination(cmd)
    cmd = add("release", cmd_release, "give up your claim")
    cmd.add_argument("ids", nargs="+")
    cmd.add_argument("-r", "--reason")
    coordination(cmd)
    cmd = add("takeover", cmd_takeover, "replace the current owner after investigating; needs --reason")
    cmd.add_argument("ids", nargs="+")
    cmd.add_argument("-r", "--reason")
    coordination(cmd)
    add("claims", cmd_claims, "who holds what, and when each owner was last active")

    for name, op, verb in (("close", "close", "Closed"), ("reopen", "reopen", "Reopened"), ("delete", "delete", "Deleted")):
        cmd = add(name, transition(op, verb), f"{name} issues")
        cmd.add_argument("ids", nargs="+")
        cmd.add_argument("-r", "--reason")
        cmd.add_argument("--force", action="store_true", help="act on an issue someone else has claimed")
        coordination(cmd)

    cmd = add("show", cmd_show, "full detail for one or more issues")
    cmd.add_argument("ids", nargs="+")

    def filters(cmd, limit):
        cmd.add_argument("-t", "--type", action="append")
        cmd.add_argument("-l", "--label", action="append")
        cmd.add_argument("-p", "--priority")
        cmd.add_argument("--assignee")
        cmd.add_argument("--parent")
        cmd.add_argument("--limit", type=int, default=limit)

    cmd = add("list", cmd_list, "list issues (open ones by default)")
    cmd.add_argument("-s", "--status", action="append")
    cmd.add_argument("-a", "--all", action="store_true")
    cmd.add_argument("--deferred", action="store_true")
    cmd.add_argument("--title-contains")
    cmd.add_argument("--id", action="append")
    filters(cmd, 50)
    cmd = add("ready", cmd_ready, "open issues with nothing blocking them")
    cmd.add_argument("--epic")
    filters(cmd, 0)
    add("blocked", cmd_blocked, "issues waiting on something")
    cmd = add("search", cmd_search, "text search over ids, titles, bodies and comments")
    cmd.add_argument("query")
    cmd.add_argument("-a", "--all", action="store_true")

    cmd = add("dep", cmd_dep, "dependencies: dep add A B [--type blocks|parent-child], dep remove A B, dep list ID, dep tree ID, dep cycles")
    cmd.add_argument("words", nargs="*")
    cmd.add_argument("--type")
    cmd.add_argument("--blocking-only", action="store_true")
    cmd = add("comments", cmd_comments, "read an issue's comments, or `comments add ID TEXT` to record one")
    cmd.add_argument("words", nargs="*")
    cmd.add_argument("-m", "--message")
    cmd.add_argument("-f", "--file")
    cmd.add_argument("--author")
    for name in ("init", "import", "create", "dep", "comments"):  # local writes publish at once unless told not to
        sub.choices[name].add_argument("--offline", action="store_true",
                                       help="record locally only; the next sync or online command publishes it")
    add("lint", cmd_lint, "open work items without acceptance criteria")

    cmd = add("sync", cmd_sync, "exchange the journal with other machines through the coordination ref")
    cmd.add_argument("--remote")
    cmd = add("log", cmd_log, "who did what: machine, agent, thread")
    cmd.add_argument("key", nargs="?")
    cmd.add_argument("--limit", type=int, default=30)
    add("info", cmd_info, "where the tracker lives and who you are")
    cmd = add("machines", cmd_machines, "machines seen in the journal")
    cmd.add_argument("--name", metavar="ID=ALIAS", help="give a machine a local display name")
    cmd = add("help", cmd_help, "this overview, or `help COMMAND` for one command's usage")
    cmd.add_argument("command_name", nargs="?", metavar="COMMAND")
    cmd.set_defaults(workspace_free=True)
    add("prime", cmd_prime, "the workflow an agent should load at the start of a session").set_defaults(workspace_free=True)
    add("onboard", cmd_onboard, "a snippet for CLAUDE.md / AGENTS.md").set_defaults(workspace_free=True)
    return parser


def main(argv=None) -> int:
    for name in ("stdout", "stderr"):  # started with the descriptor closed (`>&-`): nobody is reading
        if getattr(sys, name) is None:
            setattr(sys, name, open(os.devnull, "w", encoding="utf-8"))
    for stream in (sys.stdout, sys.stderr):  # the output contract is UTF-8, whatever the locale says
        with contextlib.suppress(Exception):
            stream.reconfigure(encoding="utf-8", errors="replace")
    try:
        return run(argv)
    except BrokenPipeError:  # the reader went away mid-write: nothing to say, and no Python noise about it
        silence(sys.stdout)
        return 0


def run(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    cut = argv.index("--") if "--" in argv else len(argv)
    head, tail = argv[:cut], argv[cut:]  # everything after `--` is data, never an option
    as_json = "--json" in head
    head = [a for a in head if a != "--json"]
    name = None
    for index, arg in enumerate(head):
        if arg == "--actor" and index + 1 < len(head):
            name = head[index + 1]
            del head[index:index + 2]
            break
        if arg.startswith("--actor="):
            name = arg.split("=", 1)[1]
            del head[index]
            break
    if head + tail in (["--version"], ["-V"]):
        emit(json.dumps({"version": __version__, "format": FORMAT}) if as_json else f"bead {__version__}")
        return 0
    if not head + tail or head + tail in (["-h"], ["--help"]):
        emit(json.dumps({"help": OVERVIEW}) if as_json else OVERVIEW.rstrip("\n"))
        return 0
    try:
        try:
            args = build_parser().parse_args(head + tail)
        except SystemExit as done:  # COMMAND --help: argparse wrote the page; deliver it like any output
            emit(None)
            return done.code or 0
        if getattr(args, "workspace_free", False):
            try:
                ws = find_workspace()
            except BeadError:
                ws = None  # help, prime and onboard work anywhere
        else:
            ws = find_workspace()
        actor = detect_actor(name)
        result, lines = args.func(args, ws, actor)
        if ws is not None and ws.wrote and not getattr(args, "offline", False):
            warning = publish_soon(ws, args, actor)  # a local write is published at once, as a close is
            if warning and lines is not None:
                lines = [*lines, warning]
        behind = unpublished_count(ws) if ws is not None and not getattr(args, "workspace_free", False) else None
        if behind:  # after --offline, a remote that did not answer, or on a store that has never published
            if isinstance(result, dict):
                result["unpublished"] = behind
            if lines is not None:
                lines = [*lines, f"{behind} event{'s' if behind != 1 else ''} not yet published: bead sync"]
    except BrokenPipeError:
        raise
    except BeadError as error:
        return report_error(error, as_json)
    except OSError as error:  # a read-only store, a full volume, a missing file: an answer, not a traceback
        where = error.filename2 or error.filename  # a failed rename names its target, not our temporary file
        return report_error(BeadError("IO", f"{error.strerror or error}: {where}" if where else str(error)), as_json)
    try:
        if result is None and lines is None:
            emit(None)
            return 0
        emit(json.dumps(result, ensure_ascii=False) if as_json else "\n".join(visible(line) for line in lines))
    except BrokenPipeError:
        silence(sys.stdout)
    return 0


def report_error(error: BeadError, as_json: bool) -> int:
    try:
        emit(json.dumps({"error": {"code": error.code, "message": str(error)}}) if as_json
             else f"Error: {visible(str(error))}", sys.stderr)
    except BrokenPipeError:
        silence(sys.stderr)  # the status is the command's, not the pipe's
    return 2 if error.code == "USAGE" else 1
