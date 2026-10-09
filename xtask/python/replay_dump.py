"""Replay journals with bead's Python implementation and print the report the Rust core must equal
(`cargo xtask diff replay`, docs/RUST-PORT.md §4.2): one case per line on stdin, one report per line
on stdout, in the same order.

A case is `{"name": ..., "ops": [...]}`, its operations those that build a journal:
  ["segment", REPLICA, ORIGIN, BYTES]   read_segment, then each event added to REPLICA's journal
  ["attempt", REPLICA, ORIGIN, BYTES]   read_attempt, each event added unsequenced
  ["add", EVENT, REPLICA, ORIGIN, SEQUENCED]
  ["problem", {"event", "origin", "problem"}]   a reader's problem, appended to journal.invalid
  ["settle"]
  ["import", REPLICA, PATH, TEXT]   import_records of TEXT (the file PATH) by REPLICA, through a Tx
                                   that emits as Tx.emit does, with ids from a counter; all or nothing

BYTES is a string whose code points are the bytes (latin-1). Run it with PYTHONINTMAXSTRDIGITS=4300:
it refuses to run under any other integer limit.
"""
import argparse
import copy
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

import beadroll as bead  # noqa: E402

LIMIT = 4300


class ImportTx:
    """What import_records needs of a Tx: its state, and emit, which checks every event as Tx.emit
    does and appends it to this journal (a copy, kept only if the import succeeds)."""

    def __init__(self, journal, replica, ids):
        self.journal, self.replica, self.ids, self._state = journal, replica, ids, None

    @property
    def state(self):
        if self._state is None:
            self._state = bead.replay(self.journal)
        return self._state

    def emit(self, op, issue=None, data=None):
        event_id = f"019b0000-0000-7000-8000-{next(self.ids):012x}"
        event = {"v": bead.FORMAT, "id": event_id, "replica": self.replica, "deps": self.journal.heads(),
                 "actor": {"agent": "import", "thread": "t"}, "op": op, "issue": issue or event_id, "data": data or {}}
        problem = bead.event_problem(event) or bead.payload_problem(event) or bead.write_problem(event)
        if problem:
            raise bead.BeadError("VALIDATION", problem)
        if bead.too_deep(event):
            raise bead.BeadError("VALIDATION", f"data nests more than {bead.MAX_NESTING} levels deep")
        try:
            bead.encode_events([event])
        except UnicodeEncodeError:
            raise bead.BeadError("VALIDATION", "text is not valid UTF-8") from None
        self.journal.add(event, self.replica)
        self._state = None
        return event


def import_into(journal, replica, path, text, ids, imports):
    """The journal after importing TEXT, or as it was if the import fails; the outcome is recorded."""
    trial = copy.deepcopy(journal)
    original = bead.commands.read_utf8
    bead.commands.read_utf8 = lambda name, what: text
    try:
        count = bead.import_records(ImportTx(trial, replica, ids), path)
    except bead.BeadError as error:
        imports.append({"error": {"code": error.code, "message": str(error)}})
        return journal
    finally:
        bead.commands.read_utf8 = original
    imports.append({"imported": count})
    return trial


def journal_of(ops, imports):
    journal = bead.Journal()
    ids = iter(range(1, 1 << 40))
    for op in ops:
        kind = op[0]
        if kind == "segment":
            _, replica, origin, data = op
            for event in bead.read_segment(data.encode("latin-1"), origin, journal.invalid):
                journal.add(event, replica, origin)
        elif kind == "attempt":
            _, replica, origin, data = op
            for event in bead.read_attempt(origin, journal.invalid, data=data.encode("latin-1")):
                journal.add(event, replica, origin, sequenced=False)
        elif kind == "add":
            _, event, replica, origin, sequenced = op
            journal.add(event, replica, origin, sequenced)
        elif kind == "problem":
            journal.invalid.append(dict(op[1]))
        elif kind == "settle":
            journal.settle()
        elif kind == "import":
            _, replica, path, text = op
            journal = import_into(journal, replica, path, text, ids, imports)
        else:
            raise ValueError(f"unknown operation {kind!r}")
    return journal


def issue_dump(issue):
    """Every member of an issue; sets sorted, tuple keys as lists, members by name."""
    out = {}
    for name, value in issue.items():
        if name == "reg":
            value = {field: dict(sorted(register.items())) for field, register in value.items()}
        elif name == "label_tags":
            value = {label: sorted(tags) for label, tags in sorted(value.items())}
        elif name == "dep_tags":
            value = [[to, kind, dict(sorted(tags.items()))] for (to, kind), tags in value.items()]
        elif name == "labels":
            value = sorted(value)
        elif name == "deps":
            value = [[to, kind, meta] for (to, kind), meta in value.items()]
        elif name == "retracted":
            rows = sorted(value.items(), key=lambda row: (row[0][0], row[0][1] is not None, row[0][1] or ""))
            value = [[to, kind, sorted(ids)] for (to, kind), ids in rows]
        out[name] = value
    return dict(sorted(out.items()))


def report(journal, imports):
    try:
        st = bead.replay(journal)
    except bead.BeadError as error:
        return {"error": {"code": error.code, "message": str(error)}, "imports": imports}
    pending = {event["id"] for event in st.pending}
    applied = [eid for eid in journal.events if eid not in pending]
    out = {
        "events": [[eid, event] for eid, event in journal.events.items()],
        "order": journal.order,
        "quarantined": journal.quarantined,
        "invalid": [{"event": e["event"], "origin": e["origin"], "problem": e["problem"]} for e in journal.invalid],
        "heads": journal.heads(),
        "prefix": st.prefix,
        "name": st.name,
        "integrity": st.integrity,
        "pending": [event["id"] for event in st.pending],
        "renames": st.renames,
        "chain_breaks": st.chain_breaks,
        "orphans": st.orphans,
        "comment_count": st.comment_count,
        "last_by_thread": [[replica, thread, eid] for (replica, thread), eid in st.last_by_thread.items()],
        "by_key": [[key, uid] for key, uid in st.by_key.items()],
        "issues": [issue_dump(issue) for issue in st.issues.values()],
        "views": [{
            "brief": bead.brief(st, issue),
            "detail": bead.detail(st, issue),
            "line": bead.line(st, issue),
            "done": bead.is_done(st, issue),
            "blocked": bead.is_blocked(st, issue),
            "blockers": [b["id"] for b in bead.open_blockers(st, issue)],
            "parents": [p["id"] for p in bead.parents_of(st, issue)],
            "children": st.children(issue["uid"]),
            "waits_for": st.waits_for(issue["uid"]),
            "descendants": sorted(bead.descendants(st, issue["uid"])),
            "ready_beneath": [i["id"] for i in ready(st, issue["uid"])],
        } for issue in st.issues.values()],
        "ready": [issue["id"] for issue in ready(st, None)],
        "cycles": bead.find_cycles(st),
        "cycles_blocking": bead.find_cycles(st, blocking_only=True),
        "export": bead.export_records(st),
        "imports": imports,
    }
    if len(applied) <= 25:
        out["saw"] = [[a, b] for a in applied for b in applied if a != b and st.saw(a, b)]
    if len(st.issues) <= 12:
        out["reaches"] = [[a, b] for a in st.issues for b in st.issues if bead.reaches(st, a, b)]
    return out


def ready(st, root):
    """`bead ready`'s rule (cmd_ready, without the CLI): open issues in scope that nobody holds,
    nothing blocks and whose status is not disputed, in select()'s order."""
    beneath = bead.descendants(st, root) if root is not None else None
    return [i for i in bead.select(st, argparse.Namespace(status=["open"]))
            if (beneath is None or i["uid"] in beneath) and not i["claim"] and not bead.is_blocked(st, i)
            and "status" not in bead.conflicts(st, i)]


def plain(value):
    """The report as JSON can hold it: an integer of more digits than Python prints by default (a
    claim count, counted one past 4,300 nines) is written {"big_int": DIGITS}."""
    if isinstance(value, bool) or value is None or isinstance(value, (str, float)):
        return value
    if isinstance(value, int):
        if -10 ** LIMIT < value < 10 ** LIMIT:
            return value
        sys.set_int_max_str_digits(0)
        try:
            return {"big_int": str(value)}
        finally:
            sys.set_int_max_str_digits(LIMIT)
    if isinstance(value, dict):
        return {key: plain(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [plain(item) for item in value]
    raise TypeError(f"cannot report {type(value).__name__}")


def main():
    if sys.get_int_max_str_digits() != LIMIT:
        sys.exit(f"replay_dump.py: run with PYTHONINTMAXSTRDIGITS={LIMIT}, not {sys.get_int_max_str_digits()}")
    sys.setrecursionlimit(10_000)  # deep values in a report are dumped recursively
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    out = sys.stdout
    for line in sys.stdin:
        case, imports = json.loads(line), []
        result = plain(report(journal_of(case["ops"], imports), imports))
        out.write(json.dumps(result, ensure_ascii=False, separators=(",", ":")) + "\n")
        out.flush()


if __name__ == "__main__":
    main()
