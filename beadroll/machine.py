"""This machine: the store and its files, the workspace a checkout resolves to, identity."""
from __future__ import annotations

import contextlib
import hashlib
import json
import os
import re
import socket
import subprocess
import time
import uuid

from pathlib import Path

from .base import (
    BpError, DEFAULT_REF, ROTATE_BYTES, is_local_path, loads_line, loads_strict,
    normalize_remote, run_git, shown, stamp, suspect, without_password,
)


def home() -> Path:
    value = os.environ.get("BEAD_HOME")
    if not value:
        return Path.home() / ".local" / "state" / "beadroll"
    path = Path(os.path.expanduser(value))  # settings files and service definitions pass `~` unexpanded
    if not path.is_absolute():
        raise BpError("CONFIG", f"BEAD_HOME must be an absolute path, not {value!r}: the store never lives in a checkout")
    return path


class Workspace:
    def __init__(self, wid: str, name: str | None, origin: str | None, config: dict):
        if not isinstance(wid, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,99}", wid):
            raise BpError("CONFIG", f"workspace id must be an opaque token, not a path: {wid!r}")
        self.id, self.name, self.origin, self.config = wid, name, origin, config
        self.derived = None  # the URL-derived id, when that is where this id came from (adopt_published)
        self.holding = False  # this process holds the store lock (see `locked`)
        self.dir = home() / wid
        self.events_dir = self.dir / "events"

    def meta(self, strict: bool = False) -> dict:
        """What this machine last used. A missing file is empty. For remote() (`strict`) the file may be
        the only record of the coordination remote: one that cannot be read or parsed, or whose remote
        is not a string, is an error there, never a silent switch to origin."""
        path = self.dir / "meta.json"
        try:
            meta = loads_strict(path.read_text(encoding="utf-8"))
            if not isinstance(meta, dict) or not isinstance(meta.get("remote", ""), str):
                raise ValueError("not an object with a string remote")
        except FileNotFoundError:
            return {}
        except (OSError, ValueError) as error:
            if strict:
                raise BpError("IO", f"{path} cannot be used ({error}): it records the coordination remote last "
                              "used here; repair it, or name the remote with --remote, which rewrites it") from None
            return {}
        return meta

    def save_meta(self, **values):
        try:
            meta = self.meta(strict=True)
        except BpError:
            if not values.get("remote"):
                return  # keep a damaged file for remote() to report: it may be the only record of the remote
            meta = {}  # the remote in use came from elsewhere: rewriting the file with it repairs it
        meta = {**meta, **{k: v for k, v in values.items() if v}}
        if self.is_origin(meta.get("remote")):
            del meta["remote"]  # origin is looked up each time: remembering its URL would freeze it
        write_atomic(self.dir / "meta.json", json.dumps(meta, indent=1).encode())

    def note_sync(self, remote: str, **values):
        """Bookkeeping once the remote has accepted. It never turns a command that took effect into
        a failure: a full volume or a damaged meta.json only leaves `last_sync` behind."""
        with contextlib.suppress(OSError):
            self.save_meta(**values, remote=remote, last_sync=stamp())

    def is_origin(self, url) -> bool:
        return isinstance(url, str) and bool(url and self.origin) and normalize_remote(url) == normalize_remote(self.origin)

    def remembered_remote(self):
        """The remote last named here. Origin itself is never remembered (older versions saved it too):
        it is looked up each time, so `git remote set-url origin` is followed."""
        remembered = self.meta(strict=True).get("remote")
        return None if self.is_origin(remembered) else remembered

    @property
    def ref(self) -> str:
        ref = self.config.get("ref") or DEFAULT_REF
        bad = not ref.startswith("refs/") or ref.startswith(("refs/heads/", "refs/tags/", "refs/remotes/"))
        if bad or run_git(["check-ref-format", ref], check=False).returncode:
            raise BpError("CONFIG", f"coordination ref {ref!r} must be a valid ref outside branches and tags")
        return ref

    @staticmethod
    def resolve_remote(value: str, typed: bool = False) -> str:
        """One remote spelling (a configured remote's name, a URL, a path) reduced to the one bead uses,
        as git would read it: a configured remote wins over a same-named path, a stored relative path is
        read from the top of the checkout whatever the cwd, a path typed just now from the cwd."""
        if re.fullmatch(r"[A-Za-z0-9._-]+", value):
            named = run_git(["remote", "get-url", value], check=False)
            if named.returncode == 0:
                value, typed = named.stdout.strip().decode("utf-8", "surrogateescape"), False
        value = without_password(value)[0]
        if not typed and is_local_path(value) and not os.path.isabs(value):
            top = run_git(["rev-parse", "--show-toplevel"], check=False)
            if top.returncode == 0:
                value = os.path.join(top.stdout.decode().strip(), value)
        return os.path.realpath(value) if os.path.exists(value) else value

    def remote(self, override: str | None = None) -> str | None:
        """The one coordination endpoint: flag, environment, tracked config, last named, then origin.
        A tracked config seeds a new store's remote, but never silently moves an existing store's."""
        typed = override or os.environ.get("BEAD_REMOTE")
        if typed:
            return self.resolve_remote(typed, typed=True)
        tracked = self.config.get("remote")
        try:
            used = self.remembered_remote()
        except BpError:  # a damaged meta.json: only fatal when it is the sole record of the remote
            if not tracked:
                raise
            used = None
        if tracked and used and self.resolve_remote(tracked) != used:
            raise BpError("CONFIG", f".beadroll.json names coordination remote {without_password(tracked)[0]}, but "
                          f"this tracker already coordinates through {used}; change it deliberately with "
                          "`bead sync --remote` or `bead init --tracked --remote`")
        value = typed or tracked or used or self.origin
        if not value:
            return None
        return self.resolve_remote(value, typed=bool(typed))


CONFIG_KEYS = {"workspace": str, "ref": str, "remote": str, "where": bool}


def checked_config(config, path) -> dict:
    """The tracked config, or a CONFIG error naming what is wrong. Unknown keys are left for newer
    versions to understand."""
    if not isinstance(config, dict):
        raise BpError("CONFIG", f"{path} must hold a JSON object")
    for key, kind in CONFIG_KEYS.items():
        if key in config and (not isinstance(config[key], kind) or isinstance(config[key], bool) and kind is not bool):
            raise BpError("CONFIG", f"{path}: {key!r} must be a {kind.__name__}")
        if kind is str and config.get(key) == "":  # an empty value would silently mean "not set"
            raise BpError("CONFIG", f"{path}: {key!r} must not be empty")
    return config


def committed_config() -> dict:
    """The committed .beadroll.json, for a checkout whose tree has none: a worktree on a branch cut before
    it was committed, a bisect, a bare clone. HEAD's copy first, then the default branch's. One git call."""
    refs = ("HEAD", "refs/remotes/origin/HEAD")
    proc = run_git(["cat-file", "--batch"], data="".join(f"{ref}:.beadroll.json\n" for ref in refs).encode(), check=False)
    out = proc.stdout if proc.returncode == 0 else b""
    for ref in refs:
        header, _, out = out.partition(b"\n")
        fields = header.split()
        if len(fields) != 3 or not fields[2].isdigit():
            continue  # "<name> missing": not in that commit, or no such ref
        body, out = out[:int(fields[2])], out[int(fields[2]) + 1:]
        if fields[1] == b"blob":
            try:
                config = loads_strict(body.decode("utf-8"))
            except ValueError:
                raise BpError("CONFIG", f"{ref}:.beadroll.json is not valid JSON") from None
            return checked_config(config, f"{ref}:.beadroll.json")
    return {}


def moved_workspaces() -> dict:
    """URL-derived ids whose repository's tracker was found under another id (adopt_published)."""
    try:
        moved = loads_strict((home() / "moved.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return {k: v for k, v in moved.items() if isinstance(v, str)} if isinstance(moved, dict) else {}


def find_workspace() -> Workspace:
    """Identify the tracker from the repository, never from the checkout path.

    A tracked `.beadroll.json` names the workspace explicitly. Without one, the id is
    derived from the origin URL, so every worktree and clone still agrees.
    """
    top = run_git(["rev-parse", "--show-toplevel"], check=False)
    root = Path(os.fsdecode(top.stdout.strip())) if top.returncode == 0 else None
    if root and (root / ".beadroll.json").is_symlink() and not (root / ".beadroll.json").exists():
        raise BpError("CONFIG", f"{root / '.beadroll.json'} is a symbolic link that does not resolve to a file")  # broken, not absent
    config = {}
    if root and (root / ".beadroll.json").exists():
        try:
            config = loads_strict((root / ".beadroll.json").read_text(encoding="utf-8"))  # UTF-8 whatever the locale
        except (OSError, ValueError):
            raise BpError("CONFIG", f"{root / '.beadroll.json'} is not valid JSON") from None
        config = checked_config(config, root / ".beadroll.json")
    else:
        config = committed_config()
    # origin means where `git push origin` goes: its first pushurl, else its first url (which is also the one
    # git fetches from; `config --get` would give the last). As configured: insteadOf stays git's, not the id's.
    proc = run_git(["config", "--get-regexp", r"^remote\.origin\.(push)?url$"], check=False)
    found = [line.partition(" ")[::2] for line in proc.stdout.decode("utf-8", "surrogateescape").splitlines()] if proc.returncode == 0 else []
    origin = next((url.strip() for key in ("remote.origin.pushurl", "remote.origin.url")
                   for k, url in found if k == key and url.strip()), "")
    if origin and root and is_local_path(origin) and not os.path.isabs(origin):
        origin = str(root / origin)  # git reads a relative path from the top of the worktree
    name = shown(normalize_remote(origin).rsplit("/", 1)[-1]) if origin else (root.name if root else None)
    derived = hashlib.sha256(normalize_remote(origin).encode("utf-8", "surrogateescape")).hexdigest()[:16] if origin else None
    tracked_wid = config.get("workspace")
    if not os.environ.get("BEAD_WORKSPACE") and tracked_wid and tracked_wid != derived \
            and re.fullmatch(r"[0-9a-f]{16}|local-[0-9a-f]{12}", tracked_wid):
        raise BpError("CONFIG", f".beadroll.json names workspace {tracked_wid}, an id another repository derives from "
                      "its own origin; a tracked workspace id is one `bead init --tracked` mints")
    wid = os.environ.get("BEAD_WORKSPACE") or tracked_wid
    if not wid and derived:
        wid = moved_workspaces().get(derived, derived)  # this repository's tracker may have been found under another id
    if not wid:
        common = run_git(["rev-parse", "--path-format=absolute", "--git-common-dir"], check=False)
        if common.returncode:
            # git's own reason, in the user's language: no repository here, or one git refuses to use
            # (dubious ownership, with the safe.directory command that fixes it). One line, so nothing is escaped.
            said = " ".join(common.stderr.decode(errors="replace").split())
            raise BpError("NO_WORKSPACE", f"no usable git repository here: {said} (outside a repository, set BEAD_WORKSPACE)"
                          if said else "not inside a git repository (or set BEAD_WORKSPACE)")
        wid = "local-" + hashlib.sha256(os.fsencode(os.path.realpath(os.fsdecode(common.stdout.strip())))).hexdigest()[:12]
    ws = Workspace(wid, name, origin or None, config)
    ws.derived = derived if not os.environ.get("BEAD_WORKSPACE") and not tracked_wid else None
    return ws


def private_dirs(path: Path):
    """Create `path` and any missing parents as 0700, whatever the umask: nothing in the store follows it."""
    missing = []
    while not path.is_dir() and path != path.parent:
        missing.append(path)
        path = path.parent
    for folder in reversed(missing):
        folder.mkdir(mode=0o700, exist_ok=True)  # a race loser is fine; a file in the way still raises


def private_dir(path: Path):
    """A directory only its owner can enter: the store and everything beneath it."""
    private_dirs(path)
    os.chmod(path, 0o700)


def write_atomic(path: Path, data: bytes, private: bool = True):
    """Written whole, then renamed into place. Store files are 0600; a file written into the
    caller's checkout (`private=False`) follows the caller's umask like any other file there.
    A write that fails leaves nothing behind."""
    if private:
        private_dirs(path.parent)
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600 if private else 0o666)
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(data)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
    except BaseException:
        with contextlib.suppress(OSError):
            tmp.unlink()  # a failed write leaves nothing behind, in the store or in a checkout
        raise


def replica_id(create: bool = True) -> str | None:
    """Opaque id of this machine. The hostname stays in a local alias, never in events."""
    path = home() / "replica.json"
    if not path.exists():
        if not create:
            return None
        private_dir(home())
        tmp = path.with_name(f".replica.{os.getpid()}.tmp")
        with os.fdopen(os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as fh:  # whole, then linked: never seen half done
            json.dump({"id": uuid.uuid4().hex[:8], "alias": socket.gethostname().split(".")[0]}, fh)
            fh.flush()
            os.fsync(fh.fileno())
        try:
            os.link(tmp, path)
        except FileExistsError:
            pass  # another process won; its identity is the machine's
        finally:
            tmp.unlink(missing_ok=True)
    try:
        raw = path.read_bytes()
    except OSError as error:  # the identity may be fine, its file cannot be reached: never advise replacing it
        raise BpError("IO", f"{error.strerror or error}: {path} (fix its access; do not replace it: the claims this "
                      "machine holds belong to that identity)") from None
    try:
        ident = loads_strict(raw.decode("utf-8"))["id"]
        if not isinstance(ident, str) or not re.fullmatch(r"[0-9a-f]{8}", ident):
            raise ValueError("id")
        return ident
    except (ValueError, KeyError, TypeError):
        raise BpError("IDENTITY", f"{path} is not a valid machine identity; restore it, or move it aside to mint a "
                      "new one (claims held under the old identity then need `bead takeover`)") from None


def machine_aliases() -> dict:
    out = {}
    with contextlib.suppress(OSError, ValueError, KeyError, TypeError):
        own = loads_strict((home() / "replica.json").read_text(encoding="utf-8"))
        if isinstance(own, dict) and isinstance(own.get("id"), str):
            out[own["id"]] = own.get("alias") if isinstance(own.get("alias"), str) else own["id"]
    with contextlib.suppress(OSError, ValueError):
        aliases = loads_strict((home() / "aliases.json").read_text(encoding="utf-8"))
        if isinstance(aliases, dict):
            out.update({k: v for k, v in aliases.items() if isinstance(k, str) and isinstance(v, str)})
    return out


def nearest_harness() -> str | None:
    """Walk up the process tree to the closest coding agent. Settles nested harnesses, where
    an agent launched from another agent's session inherits both sets of variables."""
    pid = os.getppid()
    for _ in range(12):
        try:
            proc = subprocess.run(["ps", "-o", "ppid=,args=", "-p", str(pid)], capture_output=True, text=True,
                                  errors="replace")
        except OSError:
            return None
        if proc.returncode or not proc.stdout.strip():
            return None
        parent, _, args = proc.stdout.strip().partition(" ")
        command = args.split()[0].rsplit("/", 1)[-1] if args.split() else ""
        if command.startswith("codex") or "/codex" in args.split()[0]:
            return "codex"
        if command == "claude" or (command == "node" and "claude" in args):
            return "claude"
        if not parent.strip().isdigit() or int(parent) <= 1:
            return None
        pid = int(parent)
    return None


def detect_actor(name: str | None) -> dict:
    """Agent type and thread id of the caller, taken from the harness environment."""
    env = os.environ
    claude_thread = env.get("CLAUDE_CODE_SESSION_ID")
    codex_thread = env.get("CODEX_THREAD_ID") or env.get("CODEX_SESSION_ID")
    if env.get("BEAD_AGENT"):
        agent, thread = env["BEAD_AGENT"], env.get("BEAD_THREAD")
    elif claude_thread and codex_thread:  # one agent was started from inside the other's session
        agent = nearest_harness() or "codex"
        thread = codex_thread if agent == "codex" else claude_thread
    elif claude_thread or env.get("CLAUDECODE"):
        agent, thread = "claude", claude_thread
    elif codex_thread:
        agent, thread = "codex", codex_thread
    else:
        agent, thread = "human", None
    thread = env.get("BEAD_THREAD") or thread
    actor = {"agent": agent}
    if thread:
        actor["thread"] = thread
    if name or env.get("BEAD_ACTOR"):
        actor["name"] = name or env["BEAD_ACTOR"]
    return actor


def actor_label(actor: dict, replica: str) -> str:
    if actor.get("name"):
        return actor["name"]
    thread = actor.get("thread")
    return f"{actor.get('agent', 'human')}:{thread}@{replica}" if thread else f"{actor.get('agent', 'human')}@{replica}"


def read_segment(data: bytes, origin: str = "", problems: list | None = None) -> list:
    """Parse one segment. A torn tail is skipped; any other unreadable line is reported and skipped."""
    events, lines, checked = [], data.split(b"\n"), suspect(data)
    for number, line in enumerate(lines):
        if not line.strip():
            continue
        try:
            events.append(loads_line(line, origin, number, problems) if checked else loads_strict(line))
        except ValueError:
            if number == len(lines) - 1:
                break  # torn tail: another process is mid-append
            if problems is not None:
                problems.append({"event": None, "origin": f"{origin}:{number + 1}", "problem": "line is not valid JSON"})
    return events


def encode_events(events) -> bytes:
    return b"".join(json.dumps(e, ensure_ascii=False, separators=(",", ":")).encode() + b"\n" for e in events)


def complete_prefix(data: bytes) -> bytes:
    """The bytes up to the last complete record. A final record that only lacks its newline is kept."""
    if not data or data.endswith(b"\n"):
        return data
    cut = data.rfind(b"\n") + 1
    try:
        json.loads(data[cut:])
        return data + b"\n"
    except (ValueError, RecursionError):
        return data[:cut]


def repair_tail(path: Path):
    """Drop an interrupted final write (kept beside the segment for diagnosis) so later appends stay readable."""
    data = path.read_bytes()
    fixed = complete_prefix(data)
    if fixed == data:
        return
    if not fixed.startswith(data):  # an incomplete fragment, not just a missing newline
        write_atomic(path.with_name(f"{path.name}.torn-{time.time_ns()}"), data[len(fixed):])
    write_atomic(path, fixed)


UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
OWNERSHIP = {"claim", "release"}  # events settled with the remote; they live as one file each


def dir_entries(folder: Path) -> list[Path]:
    """A directory's entries, sorted. Absent is empty; one that cannot be read, or is not a directory,
    is an IO error, never an empty journal (is_dir() and glob() would both swallow it)."""
    try:
        return sorted(folder.iterdir())
    except FileNotFoundError:
        return []


def segment_files(ws: Workspace) -> list[Path]:
    return [p for d in dir_entries(ws.events_dir) if d.is_dir() for p in dir_entries(d) if p.suffix == ".jsonl"]


def claim_files(ws: Workspace) -> list[Path]:
    return [p for d in dir_entries(ws.events_dir) if d.is_dir() for p in dir_entries(d / "claims") if p.suffix == ".json"]


def pending_files(ws: Workspace) -> list[Path]:
    """Our own ownership attempts the remote has not confirmed. Never read as events."""
    folder = ws.dir / "pending"
    return sorted(folder.glob("*.json")) if folder.is_dir() else []


def attempt_path(ws: Workspace, events: list[dict]) -> Path:
    """Where an attempt lives once it counts: one file, named after its first event, holding every
    event that has to land with it. Written whole, so a reader sees all of it or none of it."""
    return ws.events_dir / events[0]["replica"] / "claims" / f"{events[0]['id']}.json"


def active_segment(ws: Workspace, replica: str) -> Path:
    """Where this machine appends next. A full segment is never touched again."""
    folder = ws.events_dir / replica
    segments = sorted(folder.glob("*.jsonl")) if folder.is_dir() else []
    if not segments:
        return folder / "000001.jsonl"
    last = segments[-1]
    return last if last.stat().st_size < ROTATE_BYTES else folder / f"{int(last.stem) + 1:06d}.jsonl"


def append_events(ws: Workspace, replica: str, events: list[dict]):
    """Append sequential events to this machine's journal. Called with the store lock held."""
    folder = ws.events_dir / replica
    for segment in sorted(folder.glob("*.jsonl")) if folder.is_dir() else []:
        repair_tail(segment)
    path = active_segment(ws, replica)
    private_dirs(path.parent)
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600), "ab") as segment:
        segment.write(encode_events(events))
        segment.flush()
        os.fsync(segment.fileno())


def write_attempt_file(path: Path, events: list[dict]):
    write_atomic(path, encode_events(events))


def capture_where(ws: Workspace) -> dict | None:
    """The branch and commit the caller is working on, when the command runs inside a checkout."""
    if ws.config.get("where") is False:
        return None
    head = run_git(["rev-parse", "HEAD"], check=False)
    if head.returncode:
        return None
    try:
        branch = run_git(["rev-parse", "--abbrev-ref", "HEAD"], check=False).stdout.decode().strip()
    except UnicodeDecodeError:
        branch = ""  # a branch name that is not UTF-8 is not recorded (events must encode)
    return {"branch": None if branch in ("", "HEAD") else branch, "commit": head.stdout.decode().strip()}


def merged_into_default(commit: str) -> bool | None:
    """Whether `commit` is reachable from the default branch as this checkout knows it: True when either
    the remote-tracking ref or the local branch contains it (a local branch in a worktree workflow is often
    stale); None when the commit or the branch is unknown here."""
    if not commit:
        return None
    names = []
    head = run_git(["symbolic-ref", "-q", "refs/remotes/origin/HEAD"], check=False)
    if head.returncode == 0:
        names.append(os.fsdecode(head.stdout.strip()).removeprefix("refs/remotes/origin/"))
    names += ["main", "master"]
    for name in dict.fromkeys(names):
        refs = [ref for ref in (f"refs/remotes/origin/{name}", f"refs/heads/{name}")
                if run_git(["rev-parse", "--verify", "-q", ref], check=False).returncode == 0]
        if not refs:
            continue
        codes = [run_git(["merge-base", "--is-ancestor", commit, ref], check=False).returncode for ref in refs]
        if 0 in codes:
            return True
        return False if 1 in codes else None  # None: the commit is not known here
    return None


TRANSPORT_KEYS = r"^(core\.sshcommand|ssh\.variant|http\..+|credential\..+|url\..+\.(insteadof|pushinsteadof))$"
RESETTABLE = re.compile(r"^(credential\.(.+\.)?helper|http\.(.+\.)?extraheader)$")  # an empty value clears the list


def transport_config() -> list[tuple[str, str]]:
    """How git reaches remotes from here: the caller's repository config, its includeIf includes and the
    global ones, which sync.git (elsewhere, with no remotes) does not all see. Given at command-line
    precedence in this order, these settings make sync.git's git reach the remote as the caller's does:
    lists that can be reset are emptied first, so nothing is sent twice."""
    proc = run_git(["config", "--null", "--get-regexp", TRANSPORT_KEYS], check=False)
    rows = []
    for item in proc.stdout.decode(errors="replace").split("\0") if proc.returncode == 0 else []:
        if item:
            key, newline, value = item.partition("\n")
            rows.append((key, value if newline else "true"))  # a key without a value means true
    return [(key, "") for key in dict.fromkeys(key for key, _ in rows) if RESETTABLE.match(key)] + rows


def object_format() -> str | None:
    """The object format (sha1 or sha256) of the repository bead runs in; None outside one."""
    probe = run_git(["rev-parse", "--show-object-format"], check=False)
    found = probe.stdout.decode().strip() if probe.returncode == 0 else ""
    return found if found in ("sha1", "sha256") else None
