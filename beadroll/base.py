"""Errors, strict JSON and text handling, the git runner, remote URLs: what every other module needs."""
from __future__ import annotations

import contextlib
import errno
import json
import os
import re
import secrets
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
import uuid

from datetime import datetime, timezone
from pathlib import Path


__version__ = "0.1.0"
FORMAT = 1
DEFAULT_REF = "refs/beads/journal"
REMOTE_REF = "refs/beads/remote"
ROTATE_BYTES = int(os.environ.get("BEAD_ROTATE_BYTES") or 256 * 1024)
NET_TIMEOUT = float(os.environ.get("BEAD_NET_TIMEOUT") or 45)  # seconds for one fetch or push
BLOCKING = {"blocks"}
DONE = {"closed", "tombstone"}
STATUSES = ["open", "in_progress", "blocked", "deferred", "closed", "tombstone"]  # what bead's commands set
STATUS_SHAPE = re.compile(r"[a-z][a-z0-9_-]{0,39}")  # any other status Beads tools write is kept as it is
REGISTERS = ["title", "description", "design", "acceptance_criteria", "notes", "priority", "issue_type",
             "assignee", "estimated_minutes", "external_ref"]
B36 = "0123456789abcdefghijklmnopqrstuvwxyz"
ICON = {"open": "○", "in_progress": "◐", "blocked": "●", "deferred": "❄", "closed": "✓", "tombstone": "✗"}


# Export order follows the Beads JSONL records br writes.
EXPORT_FIELDS = ["title", "description", "design", "acceptance_criteria", "notes", "status", "priority",
                 "issue_type", "assignee", "estimated_minutes", "created_at", "created_by", "updated_at",
                 "closed_at", "close_reason", "external_ref"]
IMPORT_FIELDS = set(EXPORT_FIELDS)


class BpError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


def _reject_constant(value: str):
    raise ValueError(f"{value} is not a JSON number")


def loads_strict(text):
    """JSON as the standard defines it: NaN and Infinity are refused, not smuggled into exports.
    Nesting deeper than this Python can parse is unreadable JSON like any other, not a crash."""
    try:
        return json.loads(text, parse_constant=_reject_constant)
    except RecursionError:
        raise ValueError("nested deeper than this Python can parse") from None


MAX_NESTING = 100  # arrays and objects in one event; json on Python 3.12/3.13 parses only about 10,000


def too_deep(value, limit: int = MAX_NESTING) -> bool:
    """Whether a JSON value nests arrays or objects more than `limit` deep. Iterative: safe on any input."""
    stack = [(value, 1)]
    while stack:
        item, depth = stack.pop()
        if isinstance(item, (dict, list)):
            if depth > limit:
                return True
            stack.extend((child, depth + 1) for child in (item.values() if isinstance(item, dict) else item))
    return False


def run_git(args, *, env=None, data=None, check=True, timeout=None, reap=True):
    """Run git. On a timeout, `reap` kills git together with the helpers it started (a fetch has
    nothing in flight worth keeping). A push passes `reap=False`: only the client is killed, so
    whatever the receiving side is doing finishes or fails on its own; which one is the uncertain
    outcome the ownership protocol then settles.

    git's answer is its exit status, so bead waits for git, not for the end of its output: that goes
    to unlinked temporary files, because a helper git started (a hook's or a transport's background
    job) may keep git's stderr open long after git has exited."""
    with contextlib.ExitStack() as files:
        out, err = files.enter_context(tempfile.TemporaryFile()), files.enter_context(tempfile.TemporaryFile())
        stdin = subprocess.DEVNULL
        if data is not None:
            stdin = files.enter_context(tempfile.TemporaryFile())
            stdin.write(data)
            stdin.seek(0)
        child = subprocess.Popen(["git", *args], env=env, stdin=stdin, stdout=out, stderr=err, start_new_session=True)
        waiter = threading.Thread(target=child.wait, daemon=True)  # a blocking wait, not a loop of sleeps
        waiter.start()
        waiter.join(timeout)
        if waiter.is_alive():
            with contextlib.suppress(ProcessLookupError):
                if reap:
                    os.killpg(child.pid, signal.SIGKILL)
                else:
                    child.kill()
            waiter.join()
            raise BpError("REMOTE_UNREACHABLE", f"git {args[0]} gave no answer within {timeout:.0f}s") from None
        # Read without moving the file offset, which git's children share: one still running writes on after it.
        captured = [os.pread(f.fileno(), os.fstat(f.fileno()).st_size, 0) for f in (out, err)]
        proc = subprocess.CompletedProcess(child.args, child.returncode, *captured)
    if check and proc.returncode:
        said = proc.stderr.decode(errors="replace").strip()
        if proc.returncode < 0:  # killed, so git said nothing: a file-size limit (SIGXFSZ), a timeout, an operator
            try:
                name = signal.Signals(-proc.returncode).name
            except ValueError:
                name = f"signal {-proc.returncode}"
            said = f"killed by {name}" + (f": {said}" if said else "")
        raise BpError("GIT_ERROR", f"git {' '.join(args[:2])}: {said}")
    return proc


def git_text(args, **kwargs) -> str:
    return os.fsdecode(run_git(args, **kwargs).stdout.strip())


def shown(text: str) -> str:
    """Text that came from the OS as bytes, made safe to store and print: undecodable bytes become U+FFFD."""
    return text.encode("utf-8", "surrogateescape").decode("utf-8", "replace")


def without_password(url: str) -> tuple[str, bool]:
    """A remote URL with any password removed from its user-info part, and whether one was there.
    bead never stores or uses an embedded password: git's credential helpers are the place for it."""
    # git's `<transport>::<address>` form hands the address, password included, to the remote helper
    match = re.match(r"(?i)^((?:[a-z][a-z0-9+.-]*::)?([a-z][a-z0-9+.-]*)://)(.*)$", url, re.S)
    if not match:
        return url, False
    scheme, inner, rest = match.group(1), match.group(2).lower(), match.group(3)
    end = len(re.match(r"[^/?#]*", rest).group())  # as git reads a URL, user-info is only in the authority
    if "@" in rest[:end]:
        authority, _, host = rest[:end].rpartition("@")
        tail = host + rest[end:]
    elif "@" in rest[end:] and not re.fullmatch(r"(\[[^\]]*\]|[^:]*)(:[0-9]*)?", rest[:end]):
        authority, _, tail = rest.partition("@")  # no host[:port] git could use: `user:pass/word@host`
    else:
        return url, False  # an '@' in the path, query or fragment is not user-info
    if inner in ("http", "https"):  # over http(s) the user part may be the token: TOKEN@, TOKEN:x-oauth-basic@
        return f"{scheme}{tail}", True
    if ":" in authority:  # user:password, whatever the password contains
        user = authority.split(":", 1)[0]
        return (f"{scheme}{user}@{tail}" if user else f"{scheme}{tail}"), True
    return url, False  # ssh://user@host: a user, not a secret


def normalize_remote(url: str) -> str:
    """Reduce the spellings of one remote (ssh, scp-like, https, .git) to one string."""
    u = url.strip()
    if os.path.exists(u):
        return re.sub(r"\.git/?$", "", os.path.realpath(u)).lower()
    u = re.sub(r"(?i)^[a-z][a-z0-9+.-]*://", "", u)
    u = re.sub(r"^[^@/]+@", "", u)
    if re.match(r"^[^/:]+:[^/0-9]", u):
        u = u.replace(":", "/", 1)
    return re.sub(r"\.git/?$", "", u).rstrip("/").lower()


def is_local_path(url: str) -> bool:
    """A repository path rather than a URL, told apart as git does: no `scheme://` or `transport::`
    prefix, and no ':' before the first '/' (`host:path` is scp-like ssh, with or without `user@`)."""
    if re.match(r"^[A-Za-z][A-Za-z0-9+.-]*(://|::)", url):
        return False
    colon, slash = url.find(":"), url.find("/")
    return colon < 0 or 0 <= slash < colon


def uuid7() -> str:
    """Time-ordered event id. The clock part orders displays only; causality comes from `deps`."""
    millis = time.time_ns() // 1_000_000
    value = (millis & (2**48 - 1)) << 80 | 0x7 << 76 | secrets.randbits(12) << 64 | 0b10 << 62 | secrets.randbits(62)
    return str(uuid.UUID(int=value))


MAX_MILLIS = 4_102_444_800_000  # year 2100: anything later is not a clock reading


def event_millis(event_id: str) -> int:
    return int(event_id.replace("-", "")[:12], 16)


def event_time(event_id: str) -> str:
    millis = min(event_millis(event_id), MAX_MILLIS)
    stamp = datetime.fromtimestamp(millis / 1000, timezone.utc)
    return stamp.strftime("%Y-%m-%dT%H:%M:%S.") + f"{stamp.microsecond // 1000:03d}Z"


def canonical(event: dict) -> str:
    return json.dumps(event, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


LONE_SURROGATE = re.compile("[\ud800-\udfff]")
SURROGATE_ESCAPE = re.compile(rb"\\u[dD][89a-fA-F]")


def suspect(data: bytes) -> bool:
    """Whether bytes could hold a lone surrogate: only a raw 0xED byte or a \\uD800-\\uDFFF escape can carry one."""
    return b"\xed" in data or SURROGATE_ESCAPE.search(data) is not None


def loads_line(line: bytes, origin: str, number: int, problems: list | None):
    """One journal or attempt line. Text that is not valid UTF-8 (a lone surrogate, escaped or as raw
    bytes; bead refuses to write one) can be neither printed nor exported: it reads as U+FFFD, reported."""
    event = loads_strict(line)
    if suspect(line):
        text = json.dumps(event, ensure_ascii=False)
        if LONE_SURROGATE.search(text):
            event = json.loads(LONE_SURROGATE.sub("\ufffd", text))
            if problems is not None:
                eid = event.get("id") if isinstance(event, dict) else None
                problems.append({"event": eid if isinstance(eid, str) else None, "origin": f"{origin}:{number + 1}",
                                 "problem": "text is not valid UTF-8; it reads as U+FFFD"})
    return event


def read_utf8(path: str, what: str) -> str:
    """A text argument from a file or stdin, decoded as UTF-8 whatever the locale says."""
    if path == "-" and sys.stdin is None:  # fd 0 was closed when bead started
        raise BpError("IO", "standard input is closed, so - cannot be read")
    try:
        raw = sys.stdin.buffer.read() if path == "-" else Path(path).read_bytes()
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        raise BpError("VALIDATION", f"{what} is not valid UTF-8") from None


def pause(seconds: float):
    """The wait between attempts against a moving remote (tests replace this, not time.sleep)."""
    time.sleep(seconds)


def stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


CONTROLS = re.compile("[\x00-\x1f\x7f-\x9f\u061c\u2028\u2029\u200e\u200f\u202a-\u202e\u2066-\u2069]")


def visible(text: str) -> str:
    """Human output shows control characters as escapes. Titles, comments and names come from
    journals and imports, and the terminal must never be asked to interpret them."""
    return CONTROLS.sub(lambda m: f"\\x{ord(m.group()):02x}" if ord(m.group()) < 0x100 else f"\\u{ord(m.group()):04x}", text)


def emit(text: str | None, stream=None):
    """Print one block (or only flush, for None) to stdout or `stream`, the terminal's hang-up included."""
    stream = stream or sys.stdout
    try:
        if text is not None:
            print(text, file=stream)
        stream.flush()
    except OSError as error:
        raise hung_up(error, stream) from None


def hung_up(error: OSError, stream) -> OSError:
    """A terminal that hung up fails with EIO where a closed pipe fails with EPIPE: both mean nobody is
    reading, and both become BrokenPipeError. EIO from a file is a failed write and stays one."""
    with contextlib.suppress(OSError, ValueError):
        if error.errno == errno.EIO and stat.S_ISCHR(os.fstat(stream.fileno()).st_mode):
            return BrokenPipeError(errno.EPIPE, "the terminal hung up")
    return error


def silence(stream):
    """Point a broken stream at /dev/null so that neither we nor the interpreter's shutdown try it again."""
    with contextlib.suppress(OSError, ValueError):
        os.dup2(os.open(os.devnull, os.O_WRONLY), stream.fileno())
