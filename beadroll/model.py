"""The data: event validation, journals, and their replay into state."""
from __future__ import annotations

import bisect
import hashlib
import heapq
import json
import re

from .base import (
    B36, BLOCKING, BeadError, DONE, FORMAT, MAX_MILLIS,
    REGISTERS, STATUSES, STATUS_SHAPE, canonical, event_millis, event_time,
    loads_line,
)
from .machine import (UUID_RE, actor_label, claim_files, pending_files, read_segment, segment_files)


def read_attempt(path: Path, problems: list | None = None, data: bytes | None = None) -> list[dict]:
    """The events of one attempt file (one JSON record per line), or of `data` when the caller has
    read it already. An unreadable file is reported."""
    try:
        data = path.read_bytes() if data is None else data
    except OSError:
        if problems is not None:
            problems.append({"event": None, "origin": str(path), "problem": "attempt file is unreadable"})
        return []
    events = []
    for number, line in enumerate(data.split(b"\n")):
        if not line.strip():
            continue
        try:
            event = loads_line(line, str(path), number, problems)
            problem = None if isinstance(event, dict) else "attempt line is not an event"
        except ValueError:
            event, problem = None, "line is not valid JSON"
        problem = problem or event_problem(event) or payload_problem(event)
        if problem:
            if problems is not None:
                problems.append({"event": None, "origin": f"{path}:{number + 1}", "problem": f"{problem}; the whole attempt is ignored"})
            return []  # an attempt counts whole or not at all
        events.append(event)
    return events


def event_problem(event) -> str | None:
    """Why an event's envelope cannot be used, or None."""
    if not isinstance(event, dict):
        return "event is not an object"
    if not isinstance(event.get("id"), str) or not UUID_RE.match(event["id"]):
        return "id is not a UUID"
    if event_millis(event["id"]) > MAX_MILLIS:
        return "id carries an impossible timestamp"
    if not isinstance(event.get("op"), str) or not isinstance(event.get("replica"), str):
        return "op or replica missing"
    deps = event.get("deps", [])
    if not isinstance(deps, list) or not all(isinstance(d, str) and UUID_RE.match(d) for d in deps):
        return "deps is not a list of event ids"
    if event.get("data") is not None and not isinstance(event["data"], dict):
        return "data is not an object"
    if not isinstance(event.get("v", 1), int):
        return "v is not an integer"
    if event.get("issue") is not None and not isinstance(event["issue"], str):
        return "issue is not an id"
    actor = event.get("actor")
    if actor is not None and (not isinstance(actor, dict)
                              or any(not isinstance(actor.get(k), (str, type(None))) for k in ("agent", "thread", "name"))):
        return "actor is malformed"
    where = event.get("where")
    if where is not None and (not isinstance(where, dict) or not isinstance(where.get("branch"), (str, type(None)))
                              or not isinstance(where.get("commit"), (str, type(None)))):
        return "where is malformed"
    return None


KNOWN_OPS = {"init", "create", "set", "claim", "release", "close", "reopen", "delete", "comment", "dep_add",
             "dep_remove", "label_add", "label_remove", "label_set"}
REQUIRED_FIELDS = ("title", "priority", "issue_type", "status")  # every issue shows these
FIELD_TYPES = {"title": str, "description": str, "design": str, "acceptance_criteria": str, "notes": str,
               "priority": int, "issue_type": str, "assignee": str, "estimated_minutes": int, "external_ref": str,
               "status": str, "created_at": str, "updated_at": str, "closed_at": str, "close_reason": str,
               "created_by": str}


def fields_problem(fields, allow_bookkeeping: bool) -> str | None:
    if not isinstance(fields, dict):
        return "fields is not an object"
    for name, value in fields.items():
        if name not in FIELD_TYPES or (not allow_bookkeeping and name in ("created_at", "updated_at", "created_by")):
            return f"field {name!r} is not writable"
        if value is None and name in REQUIRED_FIELDS:
            return f"field {name!r} cannot be cleared"
        if value is not None and not isinstance(value, FIELD_TYPES[name]):
            return f"field {name!r} has the wrong type"
        if name == "priority" and value is not None and not 0 <= value <= 4:
            return "priority is out of range"
        if name == "status" and value not in STATUSES and not STATUS_SHAPE.fullmatch(value):
            return f"unknown status {value!r}"
    return None


def write_problem(event: dict) -> str | None:
    """Stricter than replay, for events this bead mints: Beads readers take priority and
    estimated_minutes as non-negative 32-bit integers, and a JSON boolean is not one. Replay keeps
    the looser rule so a history written before this check means the same on every machine."""
    fields = (event.get("data") or {}).get("fields") if event["op"] in ("create", "set") else None
    for name in ("priority", "estimated_minutes"):
        value = fields.get(name) if isinstance(fields, dict) else None
        if isinstance(value, bool):
            return f"field {name!r} has the wrong type"
        if name == "estimated_minutes" and value is not None and not 0 <= value <= 2**31 - 1:
            return "estimated_minutes must be between 0 and 2147483647"
    return None


def payload_problem(event: dict) -> str | None:
    """Why an event's content cannot be applied, or None. Checked whole, before anything is mutated."""
    op, data = event["op"], event.get("data") or {}
    if op not in KNOWN_OPS:
        return f"unknown operation {op!r}"
    strings = lambda key: isinstance(data.get(key, []), list) and all(isinstance(x, str) for x in data.get(key, []))
    if op == "create":
        if event.get("issue") != event["id"]:
            return "create event does not name itself as the issue"
        if not isinstance(data.get("key"), str) or not data["key"]:
            return "create event has no valid 'key'"
        problem = fields_problem(data.get("fields", {}), allow_bookkeeping=True)
        if problem:
            return problem
        if not strings("labels") or not isinstance(data.get("extra", {}), dict) or not isinstance(data.get("comments", []), list):
            return "create event has malformed labels, extra or comments"
        if not isinstance(data.get("dangling", []), list) or any(
                not isinstance(d, dict) or not isinstance(d.get("depends_on_id"), str) or not d["depends_on_id"]
                or not isinstance(d.get("type", "blocks"), (str, type(None))) for d in data.get("dangling", [])):
            return "create event has malformed dangling dependencies"
        for c in data.get("comments", []):
            if (not isinstance(c, dict) or not isinstance(c.get("text"), str)
                    or any(k not in c or not isinstance(c[k], (str, type(None))) for k in ("author", "created_at"))
                    or not isinstance(c.get("legacy_id"), (int, str, type(None))) or isinstance(c.get("legacy_id"), bool)):
                return "create event has a malformed comment"
    elif op == "set":
        return fields_problem(data.get("fields"), allow_bookkeeping=False)
    elif op in ("dep_add", "dep_remove"):
        target = data.get("to") if "to" in data or op == "dep_add" else data.get("key")
        if not isinstance(target, str) or not target or not isinstance(data.get("type", "blocks"), (str, type(None))):
            return f"{op} event has no valid target"
    elif op in ("label_add", "label_remove", "label_set"):
        if not strings("labels"):
            return f"{op} event has malformed labels"
    elif op == "comment":
        if not isinstance(data.get("text", ""), str) or not isinstance(data.get("author"), (str, type(None))):
            return "comment event is malformed"
    elif op in ("claim", "release", "close", "reopen", "delete"):
        for key in ("mode", "takeover", "reason", "claim"):
            if not isinstance(data.get(key), (str, type(None))):
                return f"{op} event has a malformed {key!r}"
        if not isinstance(data.get("generation", 0), int):
            return "claim event has a malformed generation"
    return None


class Journal:
    """Every event this machine holds, with each machine's own append order.

    Raw records are collected first and settled together, so a duplicated or
    malformed record is judged the same way whatever order the files were read in.
    """

    def __init__(self):
        self.events: dict[str, dict] = {}
        self.order: dict[str, list[str]] = {}
        self.quarantined: dict[str, list[dict]] = {}
        self.invalid: list[dict] = []
        self._raw: dict[str, list[tuple]] = {}
        self._unidentified: list[dict] = []
        self._counter = 0
        self._settled = True
        self._referenced: set[str] = set()

    def add(self, event, replica: str, origin: str = "", sequenced: bool = True):
        eid = event.get("id") if isinstance(event, dict) else None
        if not (isinstance(eid, str) and UUID_RE.match(eid)):
            entry = {"event": None, "origin": origin, "problem": event_problem(event) or "id is not a UUID"}
            self._unidentified.append(entry)
            self.invalid.append(entry)
            return
        self._counter += 1
        self._raw.setdefault(eid, []).append((event, replica, origin, sequenced, self._counter))
        if self._settled and eid not in self.events and len(self._raw[eid]) == 1 and event_problem(event) is None \
                and event["replica"] == replica:
            self._install(eid, event, replica, sequenced)  # the common case: a fresh, well-formed event
        else:
            self._settled = False

    def _install(self, eid: str, event: dict, replica: str, sequenced: bool):
        self.events[eid] = event
        if sequenced:
            self.order.setdefault(replica, []).append(eid)
        self._referenced.update(event.get("deps") or ())

    def settle(self):
        """Rebuild from the raw records: one content per id is an event; several contents are a
        quarantined id; a malformed record keeps its place as an inert placeholder; a copy found
        only in another machine's journal is ignored."""
        if self._settled:
            return
        self.events, self.order, self.quarantined, self._referenced = {}, {}, {}, set()
        self.invalid = [entry for entry in self.invalid if entry["event"] is None]  # unreadable lines and files stay reported
        placed = []
        for eid, records in self._raw.items():
            own = [r for r in records if event_problem(r[0]) is None and r[0]["replica"] == r[1]]
            placed.append((eid, records, own, own[0] if own else records[0]))
        placed.sort(key=lambda row: row[3][4])  # each journal keeps its own order, whatever else was read first
        for eid, records, own, placement in placed:
            distinct = {}
            for event, *_ in records:
                distinct.setdefault(canonical(event), event)
            event, replica, origin, sequenced, _ = placement
            if len(distinct) > 1:
                self.quarantined[eid] = list(distinct.values())
                trusted = [d for d in (event.get("deps") or []) if isinstance(d, str) and UUID_RE.match(d)] \
                    if own and isinstance(event.get("deps"), list) else []
                self._install(eid, {"id": eid, "op": "quarantined", "replica": replica, "deps": trusted, "data": {},
                                    "issue": event.get("issue") if isinstance(event, dict) else None}, replica, sequenced)
                continue
            problem = event_problem(event)
            if problem is not None:
                self.invalid.append({"event": eid, "origin": origin, "problem": problem})
                deps = [d for d in event.get("deps") or [] if isinstance(d, str) and UUID_RE.match(d)] \
                    if isinstance(event.get("deps"), list) else []
                self._install(eid, {"id": eid, "op": "invalid", "replica": replica, "deps": deps, "data": {},
                                    "problem": problem}, replica, sequenced)
                continue
            if not own:
                self.invalid.append({"event": eid, "origin": origin, "problem":
                                     f"written by machine {event['replica']} but found only in machine {replica}'s journal"})
                continue
            self._install(eid, event, replica, sequenced)
        self._settled = True

    def heads(self) -> list[str]:
        """The frontier: events nothing else here has built on yet."""
        self.settle()
        return sorted(set(self.events) - self._referenced)


def load_journal(ws: Workspace) -> Journal:
    journal = Journal()
    for path in segment_files(ws):
        for event in read_segment(path.read_bytes(), str(path), journal.invalid):
            journal.add(event, path.parent.name, str(path))
    for path in claim_files(ws):
        for event in read_attempt(path, journal.invalid):
            journal.add(event, path.parent.parent.name, str(path), sequenced=False)
    journal.settle()
    return journal


def load_pending(ws: Workspace) -> list[list[dict]]:
    """Our unconfirmed attempts, each a list of events. Only the holder of the store lock sweeps: a
    pending file it has read and found not to be an attempt is nothing to settle, and is removed
    along with its receipt. A file that cannot be read is not junk (it may be an accepted attempt
    whose receipt a retry needs): under the lock that is an IO error, and a reader skips it, as it
    skips a file that vanished (settled meanwhile by the lock holder, which keeps the receipt)."""
    out = []
    for path in pending_files(ws):
        try:
            data = path.read_bytes()
        except FileNotFoundError:
            continue
        except OSError:
            if ws.holding:
                raise
            continue
        events = read_attempt(path, data=data)
        if events:
            out.append(events)
        elif ws.holding:
            path.unlink(missing_ok=True)
            path.with_suffix(".command").unlink(missing_ok=True)
    return out


def effective_deps(journal: Journal, event: dict) -> list[str]:
    """What an event waits for. A quarantined id is inert anyway, so it waits only for causes that
    exist: those its own journal's copy names (that copy is as trustworthy as the journal's other
    events), or, lacking one, those every variant agrees on, so one bad variant cannot hold
    everything after it."""
    if event["id"] in journal.quarantined:
        if event.get("deps"):
            return [d for d in event["deps"] if d in journal.events]
        common = None
        for variant in journal.quarantined[event["id"]]:
            listed = variant.get("deps") if isinstance(variant, dict) and isinstance(variant.get("deps"), list) else []
            deps = {d for d in listed if isinstance(d, str) and d in journal.events}
            common = deps if common is None else common & deps
        return sorted(common or ())
    return event.get("deps") or []


def causal_order(journal: Journal) -> tuple[list[dict], list[dict]]:
    """Events with all their causes first; ties broken by id. The rest wait for missing causes.
    A quarantined id whose remaining causes loop back onto it is released: it applies nothing."""
    journal.settle()
    waiting, dependents, ready = {}, {}, []
    for event_id, event in journal.events.items():
        deps = effective_deps(journal, event)
        waiting[event_id] = len(deps)
        for dep in deps:
            dependents.setdefault(dep, []).append(event_id)
        if not deps:
            heapq.heappush(ready, event_id)
    ordered, applied = [], set()
    while True:
        while ready:
            event_id = heapq.heappop(ready)
            ordered.append(journal.events[event_id])
            applied.add(event_id)
            for child in dependents.get(event_id, ()):
                waiting[child] -= 1
                if waiting[child] == 0:
                    heapq.heappush(ready, child)
        stuck = [i for i in journal.quarantined if i not in applied and waiting[i] > 0]
        if not stuck:
            break
        for event_id in stuck:
            waiting[event_id] = 0
            heapq.heappush(ready, event_id)
    return ordered, [e for i, e in journal.events.items() if i not in applied]


class State:
    def __init__(self, journal: Journal):
        journal.settle()
        self.journal = journal
        self.prefix = self.name = None
        self.issues: dict[str, dict] = {}
        self.by_key: dict[str, str] = {}
        self.renames: list[dict] = []
        self.integrity: list[dict] = []
        self.pending: list[dict] = []
        self.orphans = 0
        self.seq = {eid: (rep, n) for rep, ids in journal.order.items() for n, eid in enumerate(ids)}
        self.seen: dict[str, dict] = {}
        self.own_anc: dict[str, int] = {}   # per event: a bitset over the unsequenced events in its past
        self._bit: dict[str, int] = {}      # unsequenced event id -> its bit
        self._shared: dict[int, int] = {}   # equal bitsets stored once
        self.chain_breaks: dict[str, list[int]] = {}
        self.comment_count = 0
        self.last_by_thread: dict[tuple, str] = {}
        self._children: dict[str, list[str]] | None = None

    def saw(self, event_id: str, other_id: str) -> bool:
        """True when the writer of `event_id` had already seen `other_id`."""
        if other_id in (self.journal.events[event_id].get("deps") or ()):
            return True
        if other_id in self.seq:
            replica, number = self.seq[other_id]
            return self.seen[event_id].get(replica, -1) >= number
        bit = self._bit.get(other_id)  # an unsequenced event has no position: it is an ancestor or not
        return bit is not None and (self.own_anc.get(event_id, 0) >> bit) & 1 == 1

    def children(self, uid: str) -> list[str]:
        if self._children is None:
            self._children = {}
            for child in self.issues.values():
                for (to, kind) in child["deps"]:
                    if kind == "parent-child":
                        self._children.setdefault(to, []).append(child["uid"])
        return self._children.get(uid, [])

    def waits_for(self, uid: str) -> list[str]:
        """Edges used for cycle checks: `blocks` targets, plus parent -> child as in br."""
        issue = self.issues[uid]
        return sorted({to for (to, kind) in issue["deps"] if kind in BLOCKING and to in self.issues}
                      | set(self.children(uid)))

    def claimed_keys(self) -> set:
        """Keys a new or renamed issue must not take: every issue's, and every key an imported dependency
        is still waiting for (the record it names may be imported later)."""
        return set(self.by_key) | {d["depends_on_id"] for issue in self.issues.values() for d in issue["dangling"]}

    def free_key(self, key: str, uid: str) -> str:
        taken = self.claimed_keys()
        match = re.fullmatch(r"(.+)\.(\d+)", key)
        if match:
            number = int(match.group(2))
            while f"{match.group(1)}.{number}" in taken:
                number += 1
            return f"{match.group(1)}.{number}"
        digest = b36(hashlib.sha256(uid.encode()).digest())
        for size in range(1, len(digest)):
            if key + digest[:size] not in taken:
                return key + digest[:size]
        raise BeadError("KEY_SPACE", f"cannot derive a free key from {key}")

    def resolve(self, key: str) -> dict:
        uid = self.by_key.get(key) or self.by_key.get(f"{self.prefix}-{key}")
        if not uid:
            raise BeadError("ISSUE_NOT_FOUND", f"Issue not found: {key}")
        return self.issues[uid]

    def key(self, uid: str) -> str:
        return self.issues[uid]["id"] if uid in self.issues else uid

    def who(self, event_id: str) -> str:
        event = self.journal.events[event_id]
        return actor_label(event.get("actor") or {}, event["replica"])


def b36(raw: bytes) -> str:
    number, out = int.from_bytes(raw, "big"), ""
    while number:
        number, digit = divmod(number, 36)
        out += B36[digit]
    return out


def write_register(st: State, issue: dict, field: str, event_id: str, value, supersede=()):
    """Keep every value its writer had not seen replaced; a write retires only what it observed
    (plus anything it names explicitly, such as the claim a close ends)."""
    register = issue["reg"].setdefault(field, {})
    for old in [o for o in register if o in supersede or st.saw(event_id, o)]:
        del register[old]
    register[event_id] = value
    shown = register[max(register)]  # deterministic display; alternatives stay visible as a conflict
    if field == "status":
        issue["status"] = shown["status"]
        for extra in ("closed_at", "close_reason"):
            if extra in shown:
                issue[extra] = shown[extra]
            else:
                issue.pop(extra, None)
    elif shown is None:
        issue.pop(field, None)
    else:
        issue[field] = shown


def write_status(st: State, issue: dict, event_id: str, status: str, when: str, reason=None, supersede=()):
    value = {"status": status}
    if status in DONE:
        value["closed_at"] = when
        value["close_reason"] = reason or ""
    write_register(st, issue, "status", event_id, value, supersede)


def materialise_dangling(st: State, issue: dict):
    """An imported dependency on a key that did not exist yet becomes a real edge the moment the key
    does, tagged with the create that carried it, so every machine derives the same edge and a
    later removal that saw that create can take it away."""
    kept = []
    for dep in issue["dangling"]:
        target = st.by_key.get(dep["depends_on_id"])
        if target is None or target == issue["uid"]:
            kept.append(dep)
            continue
        edge = (target, dep.get("type") or "blocks")
        if any(st.saw(removal, issue["uid"]) for key in (edge, (target, None))
               for removal in issue["retracted"].get(key, ())):
            continue  # taken back before the target existed, by someone who had seen this import
        meta = {"created_at": event_time(issue["uid"]), "created_by": st.who(issue["uid"]),  # as dep_add stamps it
                **{k: dep[k] for k in ("created_at", "created_by", "metadata", "thread_id") if k in dep}}
        issue["dep_tags"].setdefault(edge, {}).setdefault(issue["uid"], meta)
    issue["dangling"] = kept


def end_claim(issue: dict, how: str, who: str, when: str, reason=None):
    claim = issue["claim"]
    if claim:
        issue["claim_history"].append({**claim, "ended": when, "ended_by": who, "how": how,
                                       **({"reason": reason} if reason else {})})
        issue["claim"] = None


def replay(journal: Journal) -> State:
    """Fold the events into current state. Any machine holding the same events gets the same result."""
    st = State(journal)
    ordered, st.pending = causal_order(journal)
    for entry in journal.invalid:
        st.integrity.append({"event": entry["event"], "issue": None, "problem": entry["problem"], "origin": entry["origin"]})
    # Phase 1: what every event had seen, and which events cannot be applied at all.
    inert: dict[str, str] = {}
    applicable = {rep: [n for n, i in enumerate(ids) if journal.events[i]["op"] not in ("quarantined", "invalid")]
                  for rep, ids in journal.order.items()}

    def predecessor(replica: str, number: int) -> int:
        """The last position before `number` in this journal that holds an applicable event, or -1."""
        positions = applicable[replica]
        at = bisect.bisect_left(positions, number)
        return positions[at - 1] if at else -1

    for event in ordered:
        eid = event["id"]
        seen, anc = {}, 0
        for dep in effective_deps(journal, event):
            if dep not in st.seen:
                continue  # a released quarantined id may name a cause that never applied
            for replica, number in st.seen[dep].items():
                if number > seen.get(replica, -1):
                    seen[replica] = number
            anc |= st.own_anc.get(dep, 0)
            if dep not in st.seq:
                anc |= 1 << st._bit.setdefault(dep, len(st._bit))
        st.own_anc[eid] = st._shared.setdefault(anc, anc)
        if eid in st.seq:
            replica, number = st.seq[eid]
            if seen.get(replica, -1) < predecessor(replica, number):
                # Not a single writer's log: the event does not build on its predecessor. Report it and
                # do not pretend it saw what came before (a copied machine identity looks like this).
                # A placeholder that fails this applies nothing anyway, but it must not lend its
                # position to what comes after it.
                st.chain_breaks.setdefault(replica, []).append(number)
                if event["op"] not in ("quarantined", "invalid"):
                    inert[eid] = f"journal chain broken at position {number} of machine {replica} (copied machine identity?)"
                st.seen[eid] = seen
                continue
            seen[replica] = max(seen.get(replica, -1), number)
        st.seen[eid] = seen
        if eid in journal.quarantined:
            inert[eid] = "one event id with different contents"
        elif event["op"] == "invalid":
            inert[eid] = ""  # already reported when it was loaded
        elif event.get("v", 1) != FORMAT:
            inert[eid] = f"event format v{event['v']} is not one this version of bead reads"
        elif (problem := payload_problem(event)):
            inert[eid] = problem
    # Phase 2: ownership bookkeeping, over applicable events only. A takeover is settled online, so
    # it outranks anything its predecessor published later, whichever of the two sorts first here.
    superseded = set()
    for event in ordered:
        if event["id"] in inert or event["op"] != "claim" or not (event.get("data") or {}).get("takeover"):
            continue
        previous = journal.events.get(event["data"]["takeover"])
        if previous is None or previous["op"] != "claim" or previous.get("issue") != event.get("issue"):
            inert[event["id"]] = "takeover names a claim that is not a claim on this issue"
        elif not st.saw(event["id"], previous["id"]):
            inert[event["id"]] = "takeover of a claim its writer had not seen"
        elif event["data"].get("mode") != "offline" and previous["id"] not in inert:
            superseded.add(previous["id"])
    # Phase 3: apply.
    for event in ordered:
        eid, op, data, uid = event["id"], event["op"], event.get("data") or {}, event.get("issue")
        reason = inert.get(eid)
        if reason is not None:
            if reason:
                st.integrity.append({"event": eid, "issue": st.key(uid), "problem": reason})
            continue
        actor = event.get("actor") or {}
        who, when = actor_label(actor, event["replica"]), event_time(eid)
        st.last_by_thread[(event["replica"], actor.get("thread"))] = eid
        try:
            applied = apply_event(st, event, eid, op, data, uid, actor, who, when, superseded)
        except (KeyError, TypeError, AttributeError, ValueError) as error:  # last resort: never take the tracker down
            st.integrity.append({"event": eid, "issue": st.key(uid), "problem": f"event could not be applied: {error!r}"})
            continue
        if applied:
            issue = st.issues[uid]
            if not data.get("imported"):
                issue["updated_at"] = when
            if event.get("where"):
                issue["where"] = event["where"]
    for issue in st.issues.values():
        issue["labels"] = {label for label, tags in issue["label_tags"].items() if tags}
        issue["deps"] = {edge: tags[min(tags)] for edge, tags in issue["dep_tags"].items() if tags}
    return st


def apply_event(st: State, event: dict, eid: str, op: str, data: dict, uid: str,
                actor: dict, who: str, when: str, superseded: set) -> bool:
    """Apply one event. Returns True when it touched an existing issue."""
    if op == "init":
        if st.prefix is None:
            st.prefix, st.name = data.get("prefix"), data.get("name")
        return False
    if op == "create":
        key = data["key"]
        if key in st.by_key:  # two machines minted the same key before they synced
            taken, key = key, st.free_key(key, uid)
            st.renames.append({"from": taken, "to": key, "replica": event["replica"], "uid": uid})
        fields = data.get("fields") or {}
        issue = {"uid": uid, "id": key, "title": "", "status": "open", "priority": 2, "issue_type": "task",
                 "created_at": when, "created_by": who, "updated_at": when, "reg": {},
                 "label_tags": {label: {eid} for label in data.get("labels") or []}, "dep_tags": {},
                 "labels": set(), "deps": {}, "comments": [], "extra": data.get("extra") or {},
                 "dangling": list(data.get("dangling") or []), "retracted": {},
                 "claim": None, "claim_generation": 0, "claim_history": [], "provisional_claims": [],
                 "lost_claims": [], "stale": [], "unconfirmed": [],
                 "origin": {"machine": event["replica"], **actor, **({"where": event["where"]} if event.get("where") else {})}}
        issue.update(fields)
        if data.get("imported") and "created_by" not in fields:
            issue["created_by"] = None  # the record's author is unknown, not the importer
        for name in REGISTERS:
            if name in fields:
                issue["reg"][name] = {eid: fields[name]}
        issue["reg"]["status"] = {eid: {"status": issue["status"],
                                        **{k: fields[k] for k in ("closed_at", "close_reason") if k in fields}}}
        for comment in data.get("comments") or []:
            issue["comments"].append({"uid": None, **comment})
        st.issues[uid], st.by_key[key] = issue, uid
        if event.get("where"):
            issue["where"] = event["where"]
        materialise_dangling(st, issue)
        for other in st.issues.values():  # an import may have named this key before it existed
            if other is not issue and other["dangling"]:
                materialise_dangling(st, other)
        return False
    issue = st.issues.get(uid)
    if issue is None:
        st.orphans += 1
        return False
    claim = issue["claim"]
    if op == "set":
        for name, value in (data.get("fields") or {}).items():
            if name == "status":
                write_status(st, issue, eid, value, when)
            else:
                write_register(st, issue, name, eid, value)
    elif op == "claim":
        if data.get("mode") == "offline":  # provisional: never displaces a confirmed owner
            if not (claim and claim["owner"] == who):
                issue["provisional_claims"] = [p for p in issue["provisional_claims"] if p["owner"] != who]
                issue["provisional_claims"].append({"id": eid, "owner": who, "since": when,
                                                    **({"where": event["where"]} if event.get("where") else {})})
                if not claim:
                    write_status(st, issue, eid, "in_progress", when)
        elif claim is None or (data.get("takeover") and claim["id"] == data["takeover"]):
            end_claim(issue, "takeover", who, when, data.get("reason"))
            issue["claim_generation"] = max(issue["claim_generation"] + 1, data.get("generation") or 0)
            issue["claim"] = {"id": eid, "owner": who, "machine": event["replica"],
                              "agent": actor.get("agent"), "thread": actor.get("thread"),
                              "generation": issue["claim_generation"], "since": when,
                              "mode": data.get("mode") or "online",
                              **({"where": event["where"]} if event.get("where") else {})}
            issue["provisional_claims"] = [p for p in issue["provisional_claims"] if p["owner"] != who]
            write_status(st, issue, eid, "in_progress", when)
            write_register(st, issue, "assignee", eid, who)
        elif claim["owner"] != who:
            issue["lost_claims"].append({"by": who, "at": when, "held_by": claim["owner"], "event": eid})
            return False
    elif op == "release":
        if data.get("claim") in superseded:
            issue["stale"].append({"op": op, "by": who, "at": when, "claim": data.get("claim")})
            return False
        if claim and claim["id"] == data.get("claim"):
            end_claim(issue, "release", who, when, data.get("reason"))
            write_status(st, issue, eid, "open", when, supersede={claim["id"]})
            write_register(st, issue, "assignee", eid, None, supersede={claim["id"]})
        elif any(p["owner"] == who for p in issue["provisional_claims"]):
            issue["provisional_claims"] = [p for p in issue["provisional_claims"] if p["owner"] != who]
            if not claim and not issue["provisional_claims"]:
                write_status(st, issue, eid, "open", when)
        else:  # a delayed release from an owner that has since been replaced
            issue["stale"].append({"op": op, "by": who, "at": when, "claim": data.get("claim")})
            return False
    elif op == "close":
        named = data.get("claim")
        if named and (named in superseded or not (claim and claim["id"] == named)):
            issue["stale"].append({"op": op, "by": who, "at": when, "claim": named})
            return False
        # A close that names its claim knows about it, even when the claim's bytes arrived later.
        write_status(st, issue, eid, "closed", when, data.get("reason"), supersede={named} if named else ())
        if claim and (named or st.saw(eid, claim["id"])):
            if named:
                write_register(st, issue, "assignee", eid, issue.get("assignee"), supersede={named})
            end_claim(issue, "close", who, when)
        issue["provisional_claims"] = [p for p in issue["provisional_claims"] if not st.saw(eid, p["id"])]
        if event.get("where"):
            issue["closed_on"] = event["where"]
    elif op == "reopen":
        write_status(st, issue, eid, "open", when)
        issue.pop("closed_on", None)
        if claim and st.saw(eid, claim["id"]):
            end_claim(issue, "reopen", who, when)
    elif op == "delete":
        named = data.get("claim")
        if named and (named in superseded or not (claim and claim["id"] == named)):
            issue["stale"].append({"op": op, "by": who, "at": when, "claim": named})  # an owner replaced by a takeover
            return False
        write_status(st, issue, eid, "tombstone", when, data.get("reason"), supersede={named} if named else ())
        issue["extra"] = {**issue["extra"], "deleted_at": when, "deleted_by": who,
                          "delete_reason": data.get("reason") or ""}
        if claim and (named or st.saw(eid, claim["id"])):
            end_claim(issue, "delete", who, when)
    elif op == "comment":
        st.comment_count += 1
        issue["comments"].append({"uid": eid, "seq": st.comment_count, "author": data.get("author") or who,
                                  "text": data.get("text", ""), "created_at": when,
                                  **({"where": event["where"]} if event.get("where") else {})})
    elif op == "dep_add":
        edge = (data["to"], data.get("type") or "blocks")
        issue["dep_tags"].setdefault(edge, {})[eid] = {"created_at": when, "created_by": who, **(data.get("meta") or {})}
    elif op == "dep_remove":  # removes only the additions its writer had seen
        if data.get("key"):  # an imported dependency on a key that was not an issue for its writer
            wanted = data.get("type")
            issue["dangling"] = [d for d in issue["dangling"] if not (
                d["depends_on_id"] == data["key"] and wanted in (None, d.get("type") or "blocks"))]
            bound = st.by_key.get(data["key"])  # a concurrent create of the key that sorted first bound it already
            if bound is not None and st.saw(eid, uid):
                for (to, kind), tags in issue["dep_tags"].items():
                    if to == bound and wanted in (None, kind):
                        tags.pop(uid, None)  # the import's own tag only: an explicit `dep add` keeps its edge
        else:
            for edge, tags in issue["dep_tags"].items():
                if edge[0] == data["to"] and data.get("type") in (None, edge[1]):
                    for tag in [t for t in tags if st.saw(eid, t)]:
                        del tags[tag]
            issue["retracted"].setdefault((data["to"], data.get("type")), set()).add(eid)
    elif op == "label_add":
        for label in data.get("labels") or []:
            issue["label_tags"].setdefault(label, set()).add(eid)
    elif op in ("label_remove", "label_set"):
        targets = list(issue["label_tags"]) if op == "label_set" else (data.get("labels") or [])
        for label in targets:
            tags = issue["label_tags"].get(label, set())
            tags -= {t for t in tags if st.saw(eid, t)}
        if op == "label_set":
            for label in data.get("labels") or []:
                issue["label_tags"].setdefault(label, set()).add(eid)
    return True


def conflicts(st: State, issue: dict) -> dict:
    """Fields that two writers changed without seeing each other, with every alternative."""
    out = {}
    for field, register in issue["reg"].items():
        if len(register) < 2:
            continue
        values = {json.dumps(v["status"] if field == "status" else v, sort_keys=True) for v in register.values()}
        if len(values) > 1:
            out[field] = [{"value": (v["status"] if field == "status" else v), "by": st.who(e), "at": event_time(e),
                           "event": e} for e, v in sorted(register.items())]
    return out


def is_done(st: State, issue: dict) -> bool:
    """Finished for the purposes of unblocking others. A disputed status does not count."""
    return issue["status"] in DONE and "status" not in conflicts(st, issue)


def view(ws: Workspace) -> State:
    """Current state as this machine should see it: confirmed events, with our own unconfirmed
    ownership attempts listed on their issues but counting for nothing."""
    st = replay(load_journal(ws))
    for event in (e for attempt in load_pending(ws) for e in attempt):
        issue = st.issues.get(event.get("issue"))
        if issue is not None:
            issue["unconfirmed"].append({"event": event["id"], "op": event["op"],
                                         "by": actor_label(event.get("actor") or {}, event["replica"]),
                                         "at": event_time(event["id"]), **({"where": event["where"]} if event.get("where") else {})})
    return st


def remove_edge(tx, st: State, issue: dict, to: str, kind):
    """Take back the edges to `to` this writer sees. When one of them is an imported dependency on a key
    (tagged with the issue's own create), take the key back as well: a concurrent import of the same key
    that sorts first keeps the key, and after the merge the imported dependency binds to that copy."""
    imported = any(issue["uid"] in tags for (t, k), tags in issue["dep_tags"].items() if t == to and kind in (None, k))
    tx.emit("dep_remove", issue["uid"], {"to": to, "type": kind})
    if imported:
        tx.emit("dep_remove", issue["uid"], {"key": st.key(to), "type": kind})
