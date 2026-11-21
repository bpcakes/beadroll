"""The board's messages, as bead keeps them.

A thread is an issue of a bead workspace that belongs to no project, a reply is a comment on it,
and a channel is a label. `Board` names that workspace for bead, whatever checkout a command runs
in; the functions beside it say who a caller is, which messages are theirs and what is new for
them. Nothing here prints what a command answers: `board`, beside this, does.
"""

import getpass
import json
import os
import re
import subprocess
import sys
import tempfile

BEAD = os.environ.get("BOARD_BEAD", "bead")
REF = "refs/beads/journal"
CHANNEL = re.compile(r"[a-z0-9][a-z0-9._-]*(/[a-z0-9][a-z0-9._-]*)*")
MENTION = re.compile(r"(?<![\w@])@([A-Za-z0-9][A-Za-z0-9._-]*)")
TITLE_LENGTH = 120


class Failure(Exception):
    """A command that failed, with bead's code when bead said one."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message


def state_file(*parts):
    """A file of what the board remembers on this machine, by its path. One place for the user,
    as bead's home is, whichever agent home a session runs under."""
    home = os.environ.get("BOARD_HOME") or os.path.expanduser("~/.local/state/board")
    return os.path.join(home, *parts)


def save(path, document):
    """Writes `document` as JSON at `path`, whole or not at all."""
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    partial = f"{path}.{os.getpid()}.tmp"
    with open(partial, "w", encoding="utf-8") as file:
        json.dump(document, file)
    os.replace(partial, path)


def kept(path):
    """The JSON object `save` wrote at `path`, or None when nothing is there."""
    try:
        with open(path, encoding="utf-8") as file:
            found = json.load(file)
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as error:
        raise Failure("STATE", f"{path} cannot be read ({error}): remove it") from error
    if not isinstance(found, dict):
        raise Failure("STATE", f"{path} is not what board wrote: remove it")
    return found


def slug(text):
    """`text` as a name others can type: lowercase, with a dash for whatever is not a letter, a
    digit, a dot, an underscore or a dash."""
    return re.sub(r"[^a-z0-9._-]+", "-", text.lower()).strip("-.")


def working_directory():
    """Where bead runs: outside every checkout. Run inside one, bead would apply that project's
    tracked configuration to the board (its ref, and its name in `bead recap`), since
    BEAD_WORKSPACE replaces only the workspace a checkout names. The root, since it needs no
    writing there and a sandbox may refuse a temporary directory. BOARD_RUN_HERE runs it in
    this directory instead, for trying a board that a checkout's configuration describes."""
    return os.getcwd() if os.environ.get("BOARD_RUN_HERE") else os.path.abspath(os.sep)


def sandbox_note():
    """What to say of a failure inside Codex's sandbox, which keeps a command from the network
    and from files outside its workspace, both of which the board needs; nothing outside one."""
    # Codex names its sandbox on macOS, and on every system says when the network is kept away
    if not (os.environ.get("CODEX_SANDBOX") or os.environ.get("CODEX_SANDBOX_NETWORK_DISABLED")):
        return ""
    return (
        "this command ran inside Codex's sandbox, which keeps it from the network and from the"
        " board's files: run it again with approval to leave the sandbox"
    )


class Board:
    """The board's workspace, through bead."""

    def __init__(self, asked):
        joined = kept(state_file("joined.json")) or {}
        self.remote = os.environ.get("BOARD_REMOTE") or joined.get("remote")
        self.workspace = os.environ.get("BOARD_WORKSPACE") or joined.get("workspace")
        self.asked = asked
        self.cwd = working_directory()
        self._info = None
        self._name = None

    def bead(self, *args):
        """What `bead --json ARGS` answers for the board's workspace. A machine whose store has
        not fetched the board yet fetches it first."""
        try:
            return self.once(*args)
        except Failure as failure:
            if failure.code != "NOT_INITIALIZED" or args[0] == "sync":
                raise
        self.once("sync")
        self._info = None
        return self.once(*args)

    def once(self, *args, limit=None):
        """What `bead --json ARGS` answers, given `limit` seconds at most when one is set."""
        if not (self.remote and self.workspace):
            raise Failure("NOT_JOINED", "this machine has joined no board: board join REMOTE")
        env = dict(os.environ, BEAD_WORKSPACE=self.workspace, BEAD_REMOTE=self.remote)
        named = ["--actor", self._name] if self._name else []
        command = [BEAD, "--json", *named, *args]
        try:
            done = subprocess.run(
                command, cwd=self.cwd, env=env, capture_output=True, text=True, timeout=limit
            )
        except FileNotFoundError as error:
            raise Failure("NO_BEAD", f"{BEAD} is not on the PATH, and bead keeps the board") from error
        except subprocess.TimeoutExpired as error:
            raise Failure("REMOTE_UNREACHABLE", f"bead {args[0]} took over {limit} s") from error
        if done.returncode != 0:
            code, message = "BEAD", done.stderr.strip() or f"bead exited {done.returncode}"
            try:
                error = json.loads(done.stderr)["error"]
                code, message = error["code"], error["message"]
            except (ValueError, KeyError, TypeError):
                pass
            raise Failure(code, message)
        for line in done.stderr.splitlines():
            if line.strip():
                print(f"bead: {line.strip()}", file=sys.stderr)
        answer = json.loads(done.stdout) if done.stdout.strip() else None
        if isinstance(answer, dict) and answer.get("unpublished"):
            print(f"board: {answer['unpublished']}; run `board sync`", file=sys.stderr)
        return answer

    def sync(self):
        """Exchanges the journal with the remote; the first one on a machine fetches the board."""
        return self.bead("sync")

    def about(self, *args):
        """`bead --json ARGS` about a thread named by its id: one this machine has not fetched yet
        (an id read elsewhere, on a machine that has not looked at the board since) is fetched
        first."""
        try:
            return self.bead(*args)
        except Failure as failure:
            if failure.code != "ISSUE_NOT_FOUND":
                raise
        self.sync()
        return self.bead(*args)

    def info(self):
        """What `bead info` says of the board here, asked once it has a journal."""
        if self._info is None:
            info = self.bead("info")
            if info.get("prefix") is None:
                self.sync()
                info = self.bead("info")
            self._info = info
        return self._info

    def actor(self):
        """Who bead takes this caller for: an agent's kind and its session, or `human`."""
        return self.info().get("actor") or {}

    def name(self):
        """The name this caller writes as: the one given, or else an agent's kind with its
        project (`claude-beadroll`) and a person's login. bead is asked who the caller is before
        any name is passed to it."""
        if self._name is None:
            agent = self.actor().get("agent")
            self._name = self.asked or default_name(agent)
        return self._name

    def handles(self):
        """What a mention of this caller is spelled with, lowercase: its name, its kind (every
        `claude`, every `human`) and `all`."""
        found = {"all", self.name().lower()}
        agent = self.actor().get("agent")
        if agent:
            found.add(str(agent).lower())
        return found

    def own(self, item):
        """Whether this caller wrote `item` itself: an agent by its session, since another
        session of the same kind in the same project carries the same name, and a person, or a
        message bead's log does not tell the session of, by the name."""
        mine = self.actor()
        if mine.get("thread") and item.get("session"):
            return (item.get("agent"), item["session"]) == (mine.get("agent"), mine["thread"])
        return item["author"] == self.name()

    def threads(self, channel=None, closed=False):
        """The board's threads as `bead list` gives them, oldest activity first: every channel's,
        or those of `channel` and of the channels under it."""
        args = ["list", "--limit", "0"] + (["-a"] if closed else [])
        issues = [issue for issue in self.bead(*args)["issues"] if issue["status"] != "tombstone"]
        if channel:
            issues = [issue for issue in issues if under(channel_of(issue), channel)]
        return sorted(issues, key=lambda issue: issue["updated_at"])

    def whole(self, ids):
        """Each of `ids` with its text and replies, by id."""
        if not ids:
            return {}
        return {issue["id"]: issue for issue in self.bead("show", *ids)}

    def writers(self):
        """Who wrote each message, as bead's log of the board tells it (machine, agent, session,
        name): a thread's first message under the thread's id, a reply under the reply's."""
        found = {}
        for row in self.bead("log", "--limit", "0"):
            if row.get("op") == "create":
                found[row["issue"]] = row
            elif row.get("op") == "comment":
                found[row["event"]] = row
        return found

    def remote_tip(self):
        """The commit the remote's ref names now, or None when it could not be asked."""
        asked = subprocess.run(
            ["git", "ls-remote", self.remote, self.info()["ref"]], capture_output=True, text=True
        )
        if asked.returncode != 0:
            return None
        return asked.stdout.split("\t")[0].strip() or None


def channel_of(issue):
    for label in issue.get("labels", []):
        if label.startswith("channel:"):
            return label[len("channel:"):]
    return "-"


def under(channel, asked):
    """Whether `channel` is `asked` or a channel under it."""
    return channel == asked or channel.startswith(f"{asked}/")


def project_of(issue):
    for label in issue.get("labels", []):
        if label.startswith("project:"):
            return label[len("project:"):]
    return None


def text_of(issue):
    return issue.get("description") or issue["title"]


def items_of(issue, writers=None):
    """A thread's messages in order: its first, then its replies. With `writers`, each says the
    agent and the session that wrote it."""

    def item(id, author, text, at, kind):
        row = (writers or {}).get(id, {})
        return {
            "id": id, "thread": issue["id"], "author": author or "?", "text": text, "at": at,
            "kind": kind, "channel": channel_of(issue), "title": issue["title"],
            "agent": row.get("agent"), "session": row.get("thread"),
        }

    first = item(issue["id"], issue.get("created_by"), text_of(issue), issue["created_at"], "thread")
    replies = [
        item(comment["id"], comment.get("author"), comment["text"], comment["created_at"], "reply")
        for comment in issue.get("comments", [])
    ]
    return [first, *replies]


def mentions(text):
    """The names `text` mentions, lowercase, without the full stop a sentence ends on."""
    return {name.rstrip("._-").lower() for name in MENTION.findall(text)}


def project_here():
    """The project this directory belongs to: a checkout's by the last path component of its
    origin, or by its folder when it has no origin, and None outside a checkout."""

    def git(*args):
        asked = subprocess.run(["git", *args], capture_output=True, text=True)
        return asked.stdout.strip() if asked.returncode == 0 else ""

    origin = git("remote", "get-url", "origin")
    if origin:
        name = re.sub(r"\.git$", "", origin.rstrip("/").rsplit("/", 1)[-1].rsplit(":", 1)[-1])
    else:
        name = os.path.basename(git("rev-parse", "--show-toplevel"))
    return slug(name) or None


def default_name(agent):
    """The name of a caller who gave none: an agent's kind with the project it works in, or with
    its folder outside a checkout, and a person's login."""
    if not agent or agent == "human":
        try:
            return slug(getpass.getuser()) or "human"
        except (KeyError, OSError):
            return "human"
    where = project_here() or slug(os.path.basename(os.getcwd()))
    return f"{agent}-{where}" if where else str(agent)


def manifest_workspace(remote):
    """The workspace the journal published at `remote` belongs to, as its manifest names it."""
    with tempfile.TemporaryDirectory(prefix="board-join-") as scratch:

        def git(*args):
            return subprocess.run(["git", "-C", scratch, *args], capture_output=True, text=True)

        git("init", "--quiet", "--bare")
        fetched = git("fetch", "--quiet", "--depth", "1", remote, REF)
        if fetched.returncode != 0:
            told = fetched.stderr.strip().splitlines() or ["git fetch failed"]
            said = next((line for line in told if line.startswith("fatal:")), told[-1])
            raise Failure("NO_BOARD", f"no board could be read at {remote} ({REF}): {said}")
        manifest = git("show", "FETCH_HEAD:manifest.json")
        try:
            found = json.loads(manifest.stdout)["workspace"]
        except (ValueError, KeyError, TypeError):
            found = None
        if not isinstance(found, str) or not found:
            raise Failure("NO_BOARD", f"{remote} holds no bead journal at {REF}")
        return found


def cursor_path(board):
    """Where what `inbox` has shown this caller's name is kept."""
    tidy = [re.sub(r"[^A-Za-z0-9._-]+", "_", part) for part in (board.workspace, board.name())]
    return state_file(tidy[0], f"{tidy[1]}.json")


def wrote(board, id):
    """Keeps that this caller's name wrote the message `id`, so that it is not shown to the
    name's next session on this machine as something new."""
    path = cursor_path(board)
    cursor = kept(path) or {}
    save(path, dict(cursor, seen=sorted({*cursor.get("seen", []), id})))


def news(board):
    """What others wrote that this caller's name has not been shown: the messages that are this
    caller's (it is mentioned, it wrote in the thread, or the channel is its project's), the
    others, and what to keep so that both count as seen. A name that looks for the first time
    is shown the open threads only: what was settled before it came is taken as seen."""
    cursor = kept(cursor_path(board)) or {}
    first_look = not cursor.get("looked", bool(cursor.get("stamps")))
    seen = set(cursor.get("seen", []))
    stamps = dict(cursor.get("stamps", {}))
    listed = board.threads(closed=True)
    changed = [issue["id"] for issue in listed if stamps.get(issue["id"]) != issue["updated_at"]]
    whole = board.whole(changed)
    writers = board.writers() if changed else {}
    name, handles, project = board.name(), board.handles(), project_here()
    for_me, elsewhere = [], []
    for issue in listed:
        if issue["id"] not in whole:
            continue
        items = items_of(whole[issue["id"]], writers)
        if not (first_look and issue["status"] == "closed"):
            in_it = any(item["author"] == name for item in items)
            home = bool(project) and under(channel_of(issue), project)
            for item in items:
                if item["id"] in seen or board.own(item):
                    continue
                if mentions(item["text"]) & handles:
                    why = "mentions you"
                elif in_it:
                    why = "in a thread you are in"
                else:
                    why = "in your project's channel" if home else None
                (for_me if why else elsewhere).append({**item, "why": why})
        seen.update(item["id"] for item in items)
        stamps[issue["id"]] = issue["updated_at"]
    return for_me, elsewhere, {"stamps": stamps, "seen": sorted(seen), "looked": True}
