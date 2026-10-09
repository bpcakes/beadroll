"""Local transactions and the ownership protocol: writes, the link to the coordination remote, claims."""
from __future__ import annotations

import contextlib
import fcntl
import hashlib
import json
import os
import random
import re
import secrets
import shutil
import time
import uuid

from pathlib import Path

from .base import (
    BeadError, FORMAT, MAX_NESTING, NET_TIMEOUT, REMOTE_REF, git_text,
    loads_strict, pause, run_git, too_deep, uuid7, without_password,
)
from .machine import (dir_entries, 
    OWNERSHIP, Workspace, append_events, attempt_path, capture_where, claim_files,
    complete_prefix, encode_events, home, moved_workspaces, object_format, pending_files,
    private_dir, private_dirs, repair_tail, replica_id, segment_files, transport_config,
    write_atomic, write_attempt_file,
)
from .model import (
    b36, event_problem, load_journal, load_pending, payload_problem, read_attempt,
    replay, view, write_problem,
)


class Tx:
    """One locked read-modify-append on this machine's own journal.

    Sequential events go to the journal. Ownership events (`claim`, `release`) form an attempt
    that is settled with the remote as one unit, and so does anything emitted after them with
    `conditional` set: an edit made together with a claim lands with the claim or not at all.
    """

    def __init__(self, ws: Workspace, actor: dict):
        self.ws, self.actor, self.replica = ws, actor, replica_id()
        self.journal = load_journal(ws)
        self.where = capture_where(ws)
        self.pending: list[dict] = []
        self.ownership: list[dict] = []
        self.conditional = False
        self._state: State | None = None

    @property
    def state(self) -> State:
        if self._state is None:
            self._state = replay(self.journal)
        return self._state

    def emit(self, op: str, issue: str | None = None, data: dict | None = None, event_id: str | None = None) -> dict:
        event_id = event_id or uuid7()
        event = {"v": FORMAT, "id": event_id, "replica": self.replica, "deps": self.journal.heads(),
                 "actor": self.actor, "op": op, "issue": issue or event_id, "data": data or {}}
        if self.where:
            event["where"] = self.where
        problem = event_problem(event) or payload_problem(event) or write_problem(event)
        if problem:  # the same rules replay applies, before the event is minted
            raise BeadError("VALIDATION", "the clock is beyond the year 2100; fix it before writing"
                          if "timestamp" in problem else problem)
        if too_deep(event):  # every supported Python must be able to read back what any machine writes
            raise BeadError("VALIDATION", f"data nests more than {MAX_NESTING} levels deep")
        try:
            encode_events([event])
        except UnicodeEncodeError:
            raise BeadError("VALIDATION", "text is not valid UTF-8") from None
        if op in OWNERSHIP or self.conditional:
            self.ownership.append(event)
            self.journal.add(event, self.replica, sequenced=False)  # later emits build on it
        else:
            self.journal.add(event, self.replica)
            self.pending.append(event)
        self._state = None
        return event


@contextlib.contextmanager
def locked(ws: Workspace):
    private_dir(home())
    private_dir(ws.dir)
    private_dirs(ws.events_dir)
    with open(ws.dir / "lock", "a+", opener=lambda name, flags: os.open(name, flags, 0o600)) as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        ws.holding = True
        try:
            yield
        finally:
            ws.holding = False


def confirm_ownership(ws: Workspace, events: list[dict]):
    """This attempt counts: write it, as one file, where every machine will read it."""
    if events:
        write_attempt_file(attempt_path(ws, events), events)


@contextlib.contextmanager
def writing(ws: Workspace, actor: dict):
    """A local write. Ownership events made here (offline or with no remote) need no confirmation."""
    with locked(ws):
        tx = Tx(ws, actor)
        if tx.replica in tx.state.chain_breaks:  # everything written now would be inert
            raise BeadError("JOURNAL_FORK", "this machine's journal no longer forms one chain: the same machine "
                          "identity has written from two places. Give one of them a new identity.")
        yield tx
        if tx.pending:
            append_events(ws, tx.replica, tx.pending)
        if tx.ownership:
            confirm_ownership(ws, tx.ownership)
        if tx.pending or tx.ownership:  # this command went through: a repeat of an earlier one is a new command
            forget_receipts(ws, actor)
            ws.wrote = True  # the remote does not have this yet (cleared by a publish that went through)


def mint_key(st: State, seed: str, parent: dict | None) -> str:
    """A key no issue has and no imported dependency is waiting for."""
    taken = st.claimed_keys()
    if parent:
        pattern = re.compile(re.escape(parent["id"]) + r"\.(\d+)")
        used = [int(m.group(1)) for k in taken if (m := pattern.fullmatch(k))]
        return f"{parent['id']}.{max(used, default=0) + 1}"
    digest = b36(hashlib.sha256(seed.encode()).digest())
    for size in range(4, len(digest)):
        if f"{st.prefix}-{digest[:size]}" not in taken:
            return f"{st.prefix}-{digest[:size]}"
    raise BeadError("KEY_SPACE", "cannot mint a key")


# Where the caller's repository is (`git rev-parse --local-env-vars`, less the `-c` settings, which stay the
# caller's): none of it may reach sync.git
REPO_LOCATION_ENV = ("GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_OBJECT_DIRECTORY", "GIT_DIR", "GIT_WORK_TREE",
                     "GIT_IMPLICIT_WORK_TREE", "GIT_GRAFT_FILE", "GIT_INDEX_FILE", "GIT_NO_REPLACE_OBJECTS",
                     "GIT_REPLACE_REF_BASE", "GIT_PREFIX", "GIT_SHALLOW_FILE", "GIT_COMMON_DIR", "GIT_NAMESPACE",
                     "GIT_QUARANTINE_PATH")


LOCAL_DAMAGE = re.compile(r"object file .* is empty|corrupt or missing|cannot read existing object info|"
                          r"invalid index-pack output|unable to mmap|object .* is corrupt|bad object header|"
                          r"unable to read [0-9a-f]{40,64}|missing object [0-9a-f]{40,64} for refs/|"
                          r"packfile .* (?:cannot be accessed|is corrupt)|bad packed object|corrupted pack|"
                          r"index file .* is corrupt")
SYNC_REPO_CONFIG = [("gc.auto", "0"), ("gc.autoDetach", "false"), ("maintenance.auto", "false"),
                    ("fetch.unpackLimit", "1")]
REPACK_LOOSE = int(os.environ.get("BEAD_REPACK_LOOSE") or 1000)  # loose objects in sync.git before bead repacks it
REPACK_PACKS = int(os.environ.get("BEAD_REPACK_PACKS") or 40)  # packs (one per fetch that brought something) before it does


def local_damage(said: str) -> bool:
    """Whether git's complaint is about sync.git's own objects: its own read errors, never a line the remote
    relayed (those are the remote's objects, REMOTE_DAMAGED)."""
    local = "\n".join(line for line in said.splitlines() if not line.startswith("remote:") and "upload-pack:" not in line)
    return bool(LOCAL_DAMAGE.search(local))


class Link:
    """The coordination ref on one remote, reached through a private bare repository.

    The pushed history contains the manifest and the journals and nothing else:
    no code commits are its ancestors and no code objects are reachable from it.
    """

    def __init__(self, ws: Workspace, remote: str):
        self.ws, self.url, self.ref = ws, remote, ws.ref
        private_dir(ws.dir)
        self.git_dir = ws.dir / "sync.git"
        env = {k: v for k, v in os.environ.items() if k not in REPO_LOCATION_ENV}
        env.update(GIT_DIR=str(self.git_dir), GIT_TERMINAL_PROMPT="0", LC_ALL="C", GIT_AUTHOR_NAME="bead",
                   GIT_AUTHOR_EMAIL="bead@localhost", GIT_COMMITTER_NAME="bead", GIT_COMMITTER_EMAIL="bead@localhost")
        carried = transport_config()  # the checkout's transport settings, so sync.git reaches the remote as git does
        count = int(env["GIT_CONFIG_COUNT"]) if env.get("GIT_CONFIG_COUNT", "").isdigit() else 0
        for index, (key, value) in enumerate(carried, count):
            env[f"GIT_CONFIG_KEY_{index}"], env[f"GIT_CONFIG_VALUE_{index}"] = key, value
        if carried:
            env["GIT_CONFIG_COUNT"] = str(count + len(carried))
        if "GIT_SSH_COMMAND" not in env and "GIT_SSH" not in env and all(key != "core.sshcommand" for key, _ in carried):
            env["GIT_SSH_COMMAND"] = "ssh -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2"  # a dead remote must fail, not hang
        # sync.git is bead's own: no detached git maintenance racing the next command (bead repacks it itself,
        # under the store lock), and what a fetch brings stays in the pack index-pack verified and wrote whole,
        # never exploded into loose files. Last, so that these win over anything the checkout carries.
        count = int(env.get("GIT_CONFIG_COUNT") or 0)
        for index, (key, value) in enumerate(SYNC_REPO_CONFIG, count):
            env[f"GIT_CONFIG_KEY_{index}"], env[f"GIT_CONFIG_VALUE_{index}"] = key, value
        env["GIT_CONFIG_COUNT"] = str(count + len(SYNC_REPO_CONFIG))
        self.env = env
        self.object_format: str | None = None  # the project's, once fetch has looked
        self.last_error = ""  # what git said about the last push that did not go through
        self.remote_counts: dict[str, int] = {}
        self.forks: list[str] = []
        self.confirmed: list[str] = []
        self.chain_breaks: dict[str, list[int]] = {}
        self.rebuilt: str | None = None  # where a damaged sync.git was set aside, once per command

    def git(self, args, **kwargs) -> str:
        try:
            return git_text(args, env=self.env, **kwargs)
        except BeadError as error:
            if error.code == "GIT_ERROR" and self.damaged(str(error)):
                raise self.damage(str(error)) from None
            raise

    def damaged(self, said: str) -> bool:
        return local_damage(said)

    def damage(self, said: str) -> BeadError:
        return BeadError("SYNC_REPO_DAMAGED", f"this machine's sync repository {self.git_dir} is damaged; it holds nothing "
                       f"that is not also in events/ or on the remote, so bead makes it again (the damaged one is kept "
                       f"beside it as sync.git.damaged-<time>); if this persists, delete it and run the command again. "
                       f"git said: {said}")

    def rebuild(self):
        """sync.git is a cache of what the remote holds and of events/: a damaged one is set aside, kept for a look,
        and made again by the next fetch. Lock held."""
        aside = self.git_dir.with_name(f"sync.git.damaged-{time.time_ns()}")
        with contextlib.suppress(FileNotFoundError):  # another process may have set it aside already
            os.rename(self.git_dir, aside)
        self.rebuilt, self.object_format, self.remote_counts = str(aside), None, {}

    def maintain(self) -> bool:
        """Repack sync.git when loose objects or packs have piled up: one pack, loose copies of packed objects
        gone, unreachable loose objects older than an hour gone (an in-flight fetch's are younger). Lock held,
        so no bead process of this store is writing; git's own maintenance is off (SYNC_REPO_CONFIG)."""
        objects = self.git_dir / "objects"
        loose = sum(len(dir_entries(objects / f"{n:02x}")) for n in range(256))
        packs = sum(1 for p in dir_entries(objects / "pack") if p.suffix == ".pack")
        if loose < REPACK_LOOSE and packs < REPACK_PACKS:
            return False
        run_git(["repack", "-a", "-d", "-q"], env=self.env, check=False)
        run_git(["prune", "--expire=1.hour.ago"], env=self.env, check=False)
        return True

    def fetch(self) -> str | None:
        """Fetch exactly the coordination ref. Returns its head, or None if nobody has published."""
        if self.object_format is None:  # sync.git hashes like the project, not like this machine's default
            self.object_format = object_format() or ""
            have = run_git(["rev-parse", "--show-object-format"], env=self.env, check=False)
            if self.object_format and have.returncode == 0 and have.stdout.decode().strip() != self.object_format:
                # made in another format it can never exchange objects with the remote; it holds nothing that
                # is not also in events/ or on the remote, so it is made again
                stale = self.git_dir.with_name(f".sync.{uuid.uuid4().hex}.stale")
                with contextlib.suppress(FileNotFoundError):
                    os.rename(self.git_dir, stale)
                shutil.rmtree(stale, ignore_errors=True)
        if not all((self.git_dir / part).exists() for part in ("HEAD", "objects", "refs")):
            # missing, or left unfinished by a failed or interrupted init: `git init` completes it in place. No
            # template: the user's hooks are for their repositories. Two processes may race here; whichever
            # loses finds the directory in place.
            created = run_git(["init", "--quiet", "--bare", "--template=", "--shared=0600",  # 0700/0600 whatever the umask
                               *([f"--object-format={self.object_format}"] if self.object_format else []),
                               str(self.git_dir)], env=self.env, check=False)
            if created.returncode and not (self.git_dir / "HEAD").exists():
                raise BeadError("GIT_ERROR", created.stderr.decode(errors="replace").strip())
        # Into a ref of this process's own: git fetch updates its destination only if it still holds the
        # value it read first, and every bead process of this store fetches at its own time.
        mine = f"refs/beads/fetch/{os.getpid()}-{secrets.token_hex(4)}"
        proc = run_git(["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--", self.url, f"+{self.ref}:{mine}"],
                       env=self.env, check=False, timeout=NET_TIMEOUT)
        if proc.returncode:
            if b"find remote ref" in proc.stderr:
                return None
            said = proc.stderr.decode(errors="replace").strip()
            relayed = "\n".join(line for line in said.splitlines() if line.startswith("remote:") or "upload-pack:" in line)
            if re.search(r"not our ref|bad (?:tree |blob |commit )?object|unable to read [0-9a-f]{40}|"
                         r"does not point to a valid object", relayed):  # the remote answered: it lacks objects
                raise BeadError("REMOTE_DAMAGED", f"{self.ref} on {self.url} names objects that remote no longer has; "
                              "retrying will not help. Every machine keeps its own journal: delete the ref there "
                              f"(`git update-ref -d {self.ref}` in that repository), then run `bead sync` on every "
                              f"machine to publish again. git said: {said}")
            if self.damaged(said):
                raise self.damage(said)
            raise BeadError("REMOTE_UNREACHABLE", f"cannot reach the coordination remote {self.url}: " + said)
        tip = self.git(["rev-parse", mine])
        run_git(["update-ref", REMOTE_REF, tip], env=self.env, check=False)  # keeps what was fetched reachable
        run_git(["update-ref", "-d", mine], env=self.env, check=False)
        manifest = run_git(["cat-file", "blob", f"{tip}:manifest.json"], env=self.env, check=False)
        if manifest.returncode:
            raise BeadError("MANIFEST_MISSING", f"{self.ref} on {self.url} has no manifest; refusing to adopt it")
        try:
            parsed = json.loads(manifest.stdout)
            found = parsed.get("workspace")
        except (ValueError, AttributeError, RecursionError):
            raise BeadError("MANIFEST_MISSING", f"{self.ref} on {self.url} has an unreadable manifest") from None
        if found != self.ws.id:
            error = BeadError("WORKSPACE_MISMATCH", f"{self.ref} on {self.url} belongs to workspace {found}, not {self.ws.id}; "
                            f"if that is this repository's tracker (renamed, or its URL spelled differently here), set "
                            f"BEAD_WORKSPACE={found} or commit a .beadroll.json holding {{\"workspace\": \"{found}\"}}")
            error.found = found  # for adopt_published
            raise error
        if parsed.get("format") != FORMAT:
            raise BeadError("FORMAT_UNSUPPORTED", f"{self.ref} on {self.url} is in format {parsed.get('format')!r}; "
                          f"this version of bead reads format {FORMAT} only and will not rewrite it")
        return tip

    def local_blobs(self) -> dict[str, str]:
        files = segment_files(self.ws) + claim_files(self.ws)
        if not files:
            return {}
        shas = self.git(["hash-object", "-w", "--no-filters", "--stdin-paths"],
                        data="".join(f"{p}\n" for p in files).encode()).split()
        return {str(p.relative_to(self.ws.dir)): sha for p, sha in zip(files, shas)}

    def merge(self, tip: str | None) -> int:
        """Bring remote journals in. Each segment has one writer, so one side is always a prefix.

        A foreign segment that is not a prefix either way was damaged here (we never write other
        machines' journals): the published copy replaces it and ours is kept beside it. Our own
        segment in that state, or our own journal no longer forming one chain, means this machine's
        identity is in use somewhere else. Ownership files are immutable: a remote file we lack is
        copied, and one of our own pending attempts found on the remote is thereby confirmed.
        Called with the store lock held.
        """
        self.remote_counts, self.forks, self.confirmed, pulled = {}, [], [], 0
        for path in segment_files(self.ws):
            repair_tail(path)
        own = replica_id(create=False)
        pending = {p.stem: p for p in pending_files(self.ws)}
        self.confirmed_events: list[dict] = []
        if tip:
            local = self.local_blobs()
            for row in run_git(["ls-tree", "-r", "-z", tip], env=self.env).stdout.split(b"\0"):
                try:  # -z: the same names whatever core.quotePath says
                    meta, path = row.decode().split("\t", 1)
                except (UnicodeDecodeError, ValueError):
                    continue  # the empty tail, or a name no bead writes
                sha = meta.split()[2]
                if re.fullmatch(r"events/[^/]+/claims/[0-9a-f-]+\.json", path):
                    self.remote_counts[path] = 1
                    attempt_id = Path(path).stem
                    target = self.ws.dir / path
                    if local.get(path) == sha and attempt_id not in pending:
                        continue
                    theirs = run_git(["cat-file", "blob", sha], env=self.env).stdout
                    if attempt_id in pending:  # the remote has our attempt: it counts, whole
                        if pending[attempt_id].read_bytes() != theirs:
                            if target.exists() and target.read_bytes() == theirs:  # a leftover twin of a confirmed attempt
                                pending[attempt_id].unlink(missing_ok=True)  # its receipt stays: the attempt was accepted
                                continue
                            raise BeadError("ATTEMPT_FORK", f"{path} on the remote is not the attempt this machine has "
                                          "pending under that id: the machine identity is in use somewhere else")
                        if local.get(path) != sha:
                            write_atomic(target, theirs)
                        pending[attempt_id].unlink(missing_ok=True)
                        self.confirmed.append(attempt_id)
                        self.confirmed_events.extend(read_attempt(target))
                        continue
                    if target.exists():  # an immutable file that differs from the accepted copy was damaged here
                        write_atomic(target.with_name(f"{target.name}.fork-{time.time_ns()}"), target.read_bytes())
                        self.forks.append(path)
                    else:
                        pulled += 1
                    write_atomic(target, theirs)
                    continue
                if not re.fullmatch(r"events/[^/]+/\d+\.jsonl", path):
                    continue
                replica, target = Path(path).parent.name, self.ws.dir / path
                ours = target.read_bytes() if target.exists() else b""
                if local.get(path) == sha:
                    theirs = ours
                else:
                    theirs = complete_prefix(run_git(["cat-file", "blob", sha], env=self.env).stdout)
                self.remote_counts[path] = theirs.count(b"\n")
                if theirs == ours or ours.startswith(theirs):
                    continue
                if theirs.startswith(ours):
                    pulled += theirs.count(b"\n") - ours.count(b"\n")
                    write_atomic(target, theirs)
                elif replica == own:
                    raise BeadError("JOURNAL_FORK", "this machine's journal differs from the published one: the same "
                                  "machine identity is being used from two places. Give one of them a new identity.")
                else:
                    write_atomic(target.with_name(f"{target.name}.fork-{time.time_ns()}"), ours)
                    write_atomic(target, theirs)
                    self.forks.append(path)
        self.chain_breaks = replay(load_journal(self.ws)).chain_breaks
        if own and own in self.chain_breaks:
            raise BeadError("JOURNAL_FORK", "this machine's journal no longer forms one chain: the same machine "
                          "identity has written from two places. Give one of them a new identity.")
        return pulled

    def unpublished(self) -> int:
        count = sum(p.read_bytes().count(b"\n") - self.remote_counts.get(str(p.relative_to(self.ws.dir)), 0)
                    for p in segment_files(self.ws))
        return count + sum(1 for p in claim_files(self.ws) if str(p.relative_to(self.ws.dir)) not in self.remote_counts)

    def tree(self, pending: list[dict] = ()) -> str:
        for path in segment_files(self.ws):
            repair_tail(path)  # an interrupted write is never published
        blobs = self.local_blobs()
        if pending:  # the attempt rides along at the place it will live if accepted
            blobs[str(attempt_path(self.ws, pending).relative_to(self.ws.dir))] = self.git(
                ["hash-object", "-w", "--stdin"], data=encode_events(pending))
        folders: dict[str, dict[str, list[str]]] = {}
        for path, sha in sorted(blobs.items()):
            parts = path.split("/")
            folders.setdefault(parts[1], {}).setdefault(parts[2] if len(parts) == 4 else "", []).append(
                f"100644 blob {sha}\t{parts[-1]}\n")
        rows = []
        for replica, groups in sorted(folders.items()):
            entries = list(groups.get("", []))
            if "claims" in groups:
                entries.append(f"040000 tree {self.git(['mktree'], data=''.join(groups['claims']).encode())}\tclaims\n")
            rows.append(f"040000 tree {self.git(['mktree'], data=''.join(entries).encode())}\t{replica}\n")
        events = self.git(["mktree"], data="".join(rows).encode())
        manifest = json.dumps({"format": FORMAT, "workspace": self.ws.id}, sort_keys=True).encode() + b"\n"
        manifest_sha = self.git(["hash-object", "-w", "--stdin"], data=manifest)
        return self.git(["mktree"], data=f"040000 tree {events}\tevents\n100644 blob {manifest_sha}\tmanifest.json\n".encode())

    def publish(self, tip: str | None, note: str = "sync", pending: list[dict] = (), fence: bool = False) -> str:
        """Conditional update: the new commit's parent is the head we fetched, and the push is a
        plain fast-forward, so the remote refuses it if anyone else got there first.

        Returns "ok", "rejected" (the remote definitely did not apply it: someone else got there
        first), "refused" (the remote definitely did not apply it and will not on a retry: a hook, a
        rule, missing objects) or "uncertain" (the outcome is unknown: the update may still be
        applied). `last_error` keeps what git said. With `fence`, a commit is pushed even when
        nothing changed, so that any earlier in-flight update on the same head dies.
        """
        tree = self.tree(pending)
        if tip and not fence and self.git(["rev-parse", f"{tip}^{{tree}}"]) == tree:
            self.last_commit = tip
            return "ok"
        commit = self.git(["commit-tree", tree, *(["-p", tip] if tip else []), "-m", f"bead {note} {uuid.uuid4().hex[:12]}"])
        try:
            # --no-verify: the user's client pre-push hooks are written for their code pushes; the remote's own
            # hooks still apply. `--`: a repository-controlled remote string is never a git option.
            push = run_git(["push", "--porcelain", "--no-verify", "--", self.url, f"{commit}:{self.ref}"], env=self.env,
                           check=False, timeout=NET_TIMEOUT, reap=False)
        except BeadError as error:
            self.last_error = str(error)
            return "uncertain"
        # The outcome is git's own status line for this ref ("<flag>\t<from>:<to>\t<summary>"), never text on
        # stderr: a server hook can print anything there, also after the ref was updated.
        status = next((line.split("\t") for line in push.stdout.decode(errors="replace").splitlines()
                       if line.split("\t")[1:2] == [f"{commit}:{self.ref}"]), None)
        if push.returncode == 0 or (status and status[0] in (" ", "+", "*", "=")):
            self.git(["update-ref", REMOTE_REF, commit])  # the remote holds it, whatever went wrong afterwards
            self.last_commit = commit
            return "ok"
        said = push.stderr.decode(errors="replace")
        summary = status[2] if status and len(status) > 2 else ""
        self.last_error = " ".join([summary] + [line.strip() for line in said.splitlines() if line.strip()
                                                and not line.startswith(("To ", "hint:"))]).strip() or f"git push exited {push.returncode}"
        if status and status[0] == "!":
            if summary.startswith("[rejected]") or re.search(r"cannot lock ref|failed to update ref|failed to lock|"
                                                          r"incorrect old value|non-fast-forward|fetch first|stale info", summary):
                return "rejected"  # someone else got there first: decide again against the new head
            if summary.startswith("[remote rejected]"):
                # The remote's own verdict on this ref (a hook, a rule) is final: a retry gets the same answer. A
                # remote that could not take the pack (maintenance running there, or our sync repository pruned
                # under us: "unpacker error", "missing necessary objects") answers differently after a fresh fetch.
                if re.search(r"declined|prohibited|not allowed|denied|deny|checked out|funny|protected", summary):
                    return "refused"
                return "rejected"
        return "uncertain"


def drop_pending(ws: Workspace, attempts: list[list[dict]]):
    for events in attempts:
        if events:
            (ws.dir / "pending" / f"{events[0]['id']}.json").unlink(missing_ok=True)
            (ws.dir / "pending" / f"{events[0]['id']}.command").unlink(missing_ok=True)


def forget_receipts(ws: Workspace, actor: dict):
    """A later command of this actor went through. Attempts still pending stay for sync to settle;
    only the receipts that would answer a repeat of an earlier command go."""
    folder = ws.dir / "pending"
    for receipt in sorted(folder.glob("*.command")) if folder.is_dir() else []:
        try:
            recorded = loads_strict(receipt.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if isinstance(recorded, dict) and recorded.get("actor") == actor:
            receipt.unlink(missing_ok=True)


def forget(receipts: list[Path]):
    for receipt in receipts:
        receipt.unlink(missing_ok=True)


def command_signature(args, actor: dict, request=None) -> str:
    """What a command asked for, independent of how it was typed: the same command run again by
    the same actor after a crash is the same request, not a second one. `request` carries what
    the command resolved from files or stdin, so changed contents are a new request."""
    asked = {k: v for k, v in vars(args).items() if k not in ("func", "json") and not callable(v)}
    return json.dumps({"actor": actor, "args": asked, "request": request}, sort_keys=True, default=str)


def settle_commands(ws: Workspace, replica: str, actor: dict, signature: str) -> tuple[bool, list[Path]]:
    """Receipts of this actor's earlier attempts. One whose attempt the remote accepted and that asked
    for exactly this command means the command is already done. Any other settled receipt of this
    actor is stale once this command has gone through (the actor has moved on); it is returned so
    the caller drops it only then. A receipt for an attempt still pending stays. A receipt older than
    an accepted attempt of this actor is dropped now: that later command went through."""
    done, stale, settled = False, [], []
    for receipt in sorted((ws.dir / "pending").glob("*.command")) if (ws.dir / "pending").is_dir() else []:
        try:
            recorded = loads_strict(receipt.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            recorded = None
        if not isinstance(recorded, dict):  # unreadable, or not a receipt bead wrote: it can answer nothing
            receipt.unlink(missing_ok=True)
            continue
        if recorded.get("actor") != actor:
            continue
        if receipt.with_suffix(".json").exists():
            continue  # still pending: nothing to conclude yet
        settled.append(receipt)
    accepted = [r for r in settled if (ws.events_dir / replica / "claims" / r.with_suffix(".json").name).exists()]
    for receipt in settled:  # named by attempt id (UUIDv7), so in time order
        if accepted and receipt.name < accepted[-1].name:
            receipt.unlink(missing_ok=True)  # a later command of this actor was accepted: it has moved on
        elif receipt in accepted and receipt.read_text(encoding="utf-8") == signature:
            done = True
            receipt.unlink(missing_ok=True)
        else:
            stale.append(receipt)
    return done, stale


def settle_pending(ws: Workspace, link: Link, tip: str | None) -> str:
    """Our ownership attempts whose outcome is unknown, after a merge against `tip` has confirmed
    any that the remote holds. Whatever is still pending was absent at `tip`; a fence commit on
    that head makes sure an attempt still in flight can no longer land, and only then are they
    dropped. Returns "clear", "moved" (the head changed; fetch again) or "unresolved". Lock held."""
    pending = load_pending(ws)
    if not pending:
        return "clear"
    outcome = link.publish(tip, note="fence", fence=True)
    if outcome == "ok":
        drop_pending(ws, pending)
        return "clear"
    if outcome == "refused":  # the attempts stay pending: the fence that would settle them did not land
        raise refused(link)
    return "moved" if outcome == "rejected" else "unresolved"


def conflict_detail(link: Link) -> str:
    """What the remote last answered, when it was not the plain race the message names."""
    said = link.last_error or ""
    return f" (the remote last answered: {said})" if "[remote rejected]" in said else ""


def refused(link: Link) -> BeadError:
    return BeadError("REMOTE_REFUSED", f"the remote refuses bead's update of {link.ref}; nothing was lost, and "
                   f"retrying will not help until the remote allows it: {link.last_error}")


def adopt_published(ws: Workspace, remote: str, error: BeadError) -> Workspace | None:
    """A URL-derived id that finds this checkout's own origin already holding a tracker under another id
    (the repository was renamed, or its URL is spelled differently here) adopts that tracker, once, while
    nothing has been written under the derived id. A remote named any other way is never adopted."""
    found = getattr(error, "found", None)
    if error.code != "WORKSPACE_MISMATCH" or not ws.derived or ws.id != ws.derived or not isinstance(found, str):
        return None
    origin = without_password(ws.origin or "")[0]
    if not origin or remote != (os.path.realpath(origin) if os.path.exists(origin) else origin):
        return None
    if segment_files(ws) or claim_files(ws) or pending_files(ws):
        return None
    try:
        adopted = Workspace(found, ws.name, ws.origin, ws.config)  # validates the id
    except BeadError:
        return None
    adopted.derived = ws.derived
    write_atomic(home() / "moved.json", json.dumps({**moved_workspaces(), ws.derived: found}, indent=1).encode())
    return adopted


def synchronise(ws: Workspace, remote: str, actor: dict) -> dict:
    link = Link(ws, remote)
    known_renames = len(view(ws).renames)
    pulled = pushed = 0
    repacked = False
    for attempt in range(1, 13):
        try:
            tip = link.fetch()
            with locked(ws):
                if load_pending(ws):  # also sweeps pending files that are not attempts
                    tip = link.fetch()  # settling our own attempts needs the current head, not a snapshot
                pulled += link.merge(tip)
                if not tip and not segment_files(ws):
                    raise BeadError("NOT_INITIALIZED", "no tracker here and none published on the remote: run `bead init`")
                pushed = link.unpublished()
                outcome = link.publish(tip, fence=bool(load_pending(ws)))
                if outcome == "ok":
                    drop_pending(ws, load_pending(ws))  # absent at a head we just advanced: they can never land
                    repacked = link.maintain()
                    break
                if outcome == "refused":
                    raise refused(link)
                if outcome == "uncertain":
                    raise BeadError("REMOTE_UNREACHABLE", "the push did not complete; nothing was lost, run sync again "
                                  f"once the cause is fixed: {link.last_error}")
        except BeadError as error:
            if error.code == "SYNC_REPO_DAMAGED" and not link.rebuilt:
                with locked(ws):
                    link.rebuild()
                continue
            adopted = adopt_published(ws, remote, error) if attempt == 1 else None
            if adopted is None:
                raise
            return synchronise(adopted, remote, actor)
        pause(random.uniform(0.1, 0.4) * attempt)  # another machine pushed first: merge again
    else:
        raise BeadError("SYNC_CONFLICT", "the remote journal kept moving; run sync again" + conflict_detail(link))
    st = view(ws)
    ws.note_sync(remote, name=ws.name, published=published_counts(ws))
    ws.wrote = False
    return {"pulled": pulled, "pushed": pushed, "attempts": attempt, "tip": link.last_commit, "remote": remote,
            **({"sync_repo_rebuilt": link.rebuilt} if link.rebuilt else {}), **({"repacked": True} if repacked else {}),
            "issues": len(st.issues), "renamed": st.renames[known_renames:], "forked_journals": link.forks,
            "confirmed": link.confirmed, "waiting_events": len(st.pending), "integrity": len(st.integrity)}


def coordinated(ws: Workspace, actor: dict, args, build, request=None):
    """Run an ownership change against the head as it is now and publish it as one conditional update.

    The attempt (ownership events and any edits made with them) is written as pending files first,
    pushed along with our journal, and counts for nothing until the remote has it. A push the remote definitely refused is dropped. A push whose
    outcome is unknown stays pending; the next round (or the next `sync`) either finds it on the
    remote, which confirms it, or fences it off with a commit on the same head and drops it.
    Nothing sleeps while the store lock is held.
    """
    remote = ws.remote(getattr(args, "remote", None))
    if getattr(args, "offline", False) or not remote:
        with writing(ws, actor) as tx:
            return build(tx, "offline" if remote else "local")
    link = Link(ws, remote)
    replica, signature = replica_id(), command_signature(args, actor, request)
    receipt = None  # (events, result) of a push of ours that got no answer; it may still have landed
    stale: list[Path] = []  # this actor's settled receipts: dropped only once this command has gone through
    for attempt in range(1, 13):
        try:
            with locked(ws):
                tip = link.fetch()  # under the lock: ownership is decided against the head as it is now
                link.merge(tip)
                if receipt and attempt_path(ws, receipt[0]).exists():  # it had: the work is done, not to be redone
                    drop_pending(ws, [receipt[0]])
                    forget(stale)
                    ws.note_sync(remote, published=published_counts(ws)); ws.wrote = False
                    return receipt[1]
                done, stale = settle_commands(ws, replica, actor, signature)
                if done:  # an earlier run of this command was accepted
                    ws.note_sync(remote, published=published_counts(ws)); ws.wrote = False
                    return "online"
                settled = settle_pending(ws, link, tip)
                if settled == "unresolved":
                    raise BeadError("UNRESOLVED", "an ownership attempt is unsettled and the push that would settle it did "
                                  f"not complete ({link.last_error}); it counts for nothing until `bead sync` can settle it")
                if settled == "clear":
                    receipt = None  # whatever this command had in flight is settled one way or the other
                    tx = Tx(ws, actor)
                    result = build(tx, "online")
                    if tx.pending:
                        append_events(ws, tx.replica, tx.pending)
                    if not tx.ownership:
                        forget(stale)  # nothing to settle: this command went through
                        return result
                    folder = ws.dir / "pending"
                    private_dirs(folder)
                    write_atomic(folder / f"{tx.ownership[0]['id']}.command", signature.encode())  # the receipt first
                    write_attempt_file(folder / f"{tx.ownership[0]['id']}.json", tx.ownership)
                    if os.environ.get("BEAD_TEST_CRASH_BEFORE_PUSH"):
                        os._exit(98)
                    try:
                        outcome = link.publish(tip, note="claim", pending=tx.ownership)
                    except BeadError as error:
                        if error.code != "REMOTE_UNREACHABLE":
                            raise
                        outcome = "uncertain"
                    if outcome == "ok":
                        if os.environ.get("BEAD_TEST_CRASH_AFTER_PUSH"):
                            os._exit(99)  # test hook: the remote accepted, the acknowledgement was lost
                        confirm_ownership(ws, tx.ownership)
                        drop_pending(ws, [tx.ownership])
                        forget(stale)  # this command went through: the actor has moved on from those
                        ws.note_sync(remote, published=published_counts(ws)); ws.wrote = False
                        return result
                    if outcome == "refused":  # it definitely did not land, and will not on a retry
                        drop_pending(ws, [tx.ownership])
                        raise refused(link)
                    if outcome == "rejected":
                        drop_pending(ws, [tx.ownership])  # someone else got there first; decide again against the new head
                    else:  # uncertain: the attempt stays pending; keep its result in case the push did land
                        receipt = (tx.ownership, result)
        except BeadError as error:
            if error.code == "SYNC_REPO_DAMAGED" and not link.rebuilt:  # once: set it aside and decide again
                with locked(ws):
                    link.rebuild()
                continue
            raise
        pause(random.uniform(0.1, 0.4) * attempt)
    raise BeadError("SYNC_CONFLICT", "the remote journal kept moving; try again" + conflict_detail(link))


def published_counts(ws: Workspace) -> dict[str, int]:
    """What the remote holds of this store the moment a publish went through: lines per journal segment and
    one per claim file, by path. Kept in meta.json so that `unpublished_count` needs no network."""
    counts = {str(p.relative_to(ws.dir)): p.read_bytes().count(b"\n") for p in segment_files(ws)}
    counts.update({str(p.relative_to(ws.dir)): 1 for p in claim_files(ws)})
    return counts


def unpublished_count(ws: Workspace) -> int | None:
    """Events of this store the remote does not have, as of the last publish that went through; None when
    there is no remote to publish to. Everything is unpublished before the first publish."""
    try:
        if not ws.remote(None):
            return None
    except BeadError:
        return None
    meta = ws.meta()
    published = meta.get("published")
    if not isinstance(published, dict):
        if meta.get("last_sync"):  # published by a build that kept no counts: unknown until the next publish
            return None
        published = {}  # never published: everything is behind
    count = sum(max(0, p.read_bytes().count(b"\n") - int(published.get(str(p.relative_to(ws.dir)), 0) or 0))
                for p in segment_files(ws))
    return count + sum(1 for p in claim_files(ws) if str(p.relative_to(ws.dir)) not in published)


def publish_soon(ws: Workspace, args, actor: dict) -> str | None:
    """Ownership ended locally: tell the other machines now rather than at the next routine sync."""
    if getattr(args, "offline", False):
        return None
    try:
        remote = ws.remote(getattr(args, "remote", None))
        if not remote:
            return None
        synchronise(ws, remote, actor)
        return None
    except BeadError as error:
        return f"recorded locally, not yet published ({error.code})"
    except OSError as error:  # a full volume while pulling, say: the local write stands all the same
        return f"recorded locally, not yet published (IO: {error.strerror or error})"
