"""Reading the state: the dependency graph, readiness, presentation and Beads export."""
from __future__ import annotations

import hashlib

from .base import (BLOCKING, EXPORT_FIELDS, ICON)
from .machine import (merged_into_default)
from .model import (conflicts, is_done)


def reaches(st: State, start: str, goal: str) -> bool:
    """True when `start` already waits, directly or not, for `goal`."""
    stack, seen = [start], set()
    while stack:
        uid = stack.pop()
        if uid == goal:
            return True
        if uid in seen or uid not in st.issues:
            continue
        seen.add(uid)
        stack.extend(st.waits_for(uid))
    return False


def parents_of(st: State, issue: dict) -> list[dict]:
    return [st.issues[to] for (to, kind) in sorted(issue["deps"]) if kind == "parent-child" and to in st.issues]


def parent_of(st: State, issue: dict) -> dict | None:
    parents = parents_of(st, issue)
    return parents[0] if parents else None


def descendants(st: State, uid: str) -> set[str]:
    """Every issue beneath `uid`: its children, theirs, and so on, through every parent link and
    whatever the issues in between are. Only parent-child edges count (a key that merely looks like
    a child's does not), and `uid` itself is never among them, even when a merged cycle leads back."""
    found, stack = set(), list(st.children(uid))
    while stack:
        child = stack.pop()
        if child != uid and child not in found:
            found.add(child)
            stack.extend(st.children(child))
    return found


def direct_blockers(st: State, issue: dict) -> list[dict]:
    return [st.issues[to] for (to, kind) in sorted(issue["deps"])
            if kind in BLOCKING and to in st.issues and not is_done(st, st.issues[to])]


def open_blockers(st: State, issue: dict) -> list[dict]:
    """br's readiness rule, checked against br on real trackers.

    Prerequisites block directly; an epic (only an epic) waits for its open
    children; and an ancestor held by a prerequisite holds its descendants.
    """
    found = {b["uid"]: b for b in direct_blockers(st, issue)}
    if issue["issue_type"] == "epic":
        found.update({uid: st.issues[uid] for uid in st.children(issue["uid"])
                      if not is_done(st, st.issues[uid])})
    seen, queue = {issue["uid"]}, list(parents_of(st, issue))
    while queue:
        parent = queue.pop()
        if parent["uid"] in seen or is_done(st, parent):
            continue
        seen.add(parent["uid"])
        found.update({b["uid"]: b for b in direct_blockers(st, parent)})
        queue.extend(parents_of(st, parent))
    return sorted(found.values(), key=lambda i: i["id"])


def is_blocked(st: State, issue: dict) -> bool:
    return bool(open_blockers(st, issue))


MAX_CYCLES = 200


def strongly_connected(nodes: list[str], edges) -> list[list[str]]:
    """Tarjan's components, without recursion. Only components that can hold a cycle are returned."""
    index, low, on_stack, stack, out = {}, {}, set(), [], []
    for root in nodes:
        if root in index:
            continue
        work = [(root, iter(edges(root)))]
        index[root] = low[root] = len(index)
        stack.append(root)
        on_stack.add(root)
        while work:
            node, children = work[-1]
            child = next(children, None)
            if child is not None:
                if child not in index:
                    index[child] = low[child] = len(index)
                    stack.append(child)
                    on_stack.add(child)
                    work.append((child, iter(edges(child))))
                elif child in on_stack:
                    low[node] = min(low[node], index[child])
                continue
            work.pop()
            if work:
                low[work[-1][0]] = min(low[work[-1][0]], low[node])
            if low[node] == index[node]:
                component = []
                while True:
                    member = stack.pop()
                    on_stack.discard(member)
                    component.append(member)
                    if member == node:
                        break
                if len(component) > 1 or node in edges(node):
                    out.append(sorted(component))
    return out


def find_cycles(st: State, blocking_only: bool = False) -> list[list[str]]:
    """Every elementary cycle of the waits-for graph (Johnson's algorithm, without recursion), each
    written from its smallest member, at most MAX_CYCLES of them."""
    def edges(uid):
        if blocking_only:
            return [to for (to, kind) in sorted(st.issues[uid]["deps"]) if kind in BLOCKING and to in st.issues]
        return st.waits_for(uid)

    cycles = []
    components = strongly_connected(sorted(st.issues), edges)
    while components and len(cycles) < MAX_CYCLES:
        component = set(components.pop())
        start = min(component)
        inside = lambda uid: [to for to in edges(uid) if to in component]
        path, blocked, closed, unblock_later = [start], {start}, set(), {}
        work = [(start, iter(inside(start)))]
        while work and len(cycles) < MAX_CYCLES:
            node, children = work[-1]
            child = next(children, None)
            if child is not None:
                if child == start:
                    cycles.append(list(path))
                    closed.update(path)
                elif child not in blocked:
                    path.append(child)
                    blocked.add(child)
                    closed.discard(child)
                    work.append((child, iter(inside(child))))
                continue
            work.pop()
            path.pop()
            if node in closed:  # a cycle went through here: everything waiting on it may try again
                queue = [node]
                while queue:
                    unblocked = queue.pop()
                    if unblocked in blocked:
                        blocked.discard(unblocked)
                        queue.extend(unblock_later.pop(unblocked, ()))
            else:
                for to in inside(node):
                    unblock_later.setdefault(to, set()).add(node)
        component.discard(start)  # cycles through `start` are all found; the rest live in what remains
        components.extend(strongly_connected(sorted(component), lambda uid: [to for to in edges(uid) if to in component]))
    return [[st.key(uid) for uid in cycle] for cycle in cycles]


def brief(st: State, issue: dict) -> dict:
    out = {"id": issue["id"], "title": issue["title"], "status": issue["status"],
           "priority": issue["priority"], "issue_type": issue["issue_type"]}
    if issue.get("assignee"):
        out["assignee"] = issue["assignee"]
    if issue["labels"]:
        out["labels"] = sorted(issue["labels"])
    if issue.get("where", {}).get("branch"):
        out["branch"] = issue["where"]["branch"]
    disputed = sorted(conflicts(st, issue))
    if disputed:
        out["conflicted"] = disputed
    out["created_at"], out["updated_at"] = issue["created_at"], issue["updated_at"]
    return out


def detail(st: State, issue: dict, check_merge: bool = False) -> dict:
    out = {"id": issue["id"]}
    for name in EXPORT_FIELDS:
        if issue.get(name) not in (None, ""):
            out[name] = issue[name]
    if issue["labels"]:
        out["labels"] = sorted(issue["labels"])
    parents = parents_of(st, issue)
    if parents:
        out["parent"] = parents[0]["id"]
        if len(parents) > 1:
            out["parents"] = [p["id"] for p in parents]
    out["dependencies"] = [{"id": st.key(to), "title": st.issues[to]["title"] if to in st.issues else "",
                            "status": st.issues[to]["status"] if to in st.issues else "unknown",
                            "dependency_type": kind} for (to, kind) in sorted(issue["deps"])]
    if issue["dangling"]:  # imported dependencies on keys that are not issues yet: they bind when the key arrives
        out["unresolved_dependencies"] = [{"id": key, "dependency_type": kind} for (key, kind) in sorted(
            {(d["depends_on_id"], d.get("type") or "blocks") for d in issue["dangling"]})]
    out["dependents"] = [{"id": other["id"], "title": other["title"], "status": other["status"],
                          "dependency_type": kind}
                         for other in st.issues.values() for (to, kind) in sorted(other["deps"])
                         if to == issue["uid"]]
    out["comments"] = [{"id": c["legacy_id"] if c.get("legacy_id") is not None else c["uid"], "author": c.get("author"),
                        "text": c["text"], "created_at": c.get("created_at"), **({"branch": c["where"]["branch"]} if c.get("where", {}).get("branch") else {})}
                       for c in issue["comments"]]
    out["blocked"] = is_blocked(st, issue)
    out["origin"] = issue["origin"]
    if issue.get("where"):
        out["where"] = issue["where"]
    if issue.get("closed_on"):
        out["closed_on"] = issue["closed_on"]
        if check_merge and issue["status"] == "closed":
            merged = merged_into_default(issue["closed_on"].get("commit"))
            out["merged"] = "unknown" if merged is None else merged
    if issue["claim"]:
        out["claim"] = issue["claim"]
    for name in ("provisional_claims", "lost_claims", "stale", "unconfirmed"):
        if issue[name]:
            out[name] = issue[name]
    disputed = conflicts(st, issue)
    if disputed:
        out["conflicts"] = disputed
    return out


def line(st: State, issue: dict) -> str:
    who = f" @{issue['assignee']}" if issue.get("assignee") else ""
    disputed = sorted(conflicts(st, issue))
    flag = " ⚠ conflict: " + ", ".join(disputed) if disputed else ""
    branch = f" ({issue['where']['branch']})" if issue.get("where", {}).get("branch") else ""
    return (f"{ICON.get(issue['status'], '?')} {issue['id']} [P{issue['priority']}] [{issue['issue_type']}] - "
            f"{issue['title']}{who}{branch}{flag}")


COMMENT_ID_BASE = 2 ** 47
RESERVED_EXPORT = set(EXPORT_FIELDS) | {"id", "labels", "dependencies", "comments", "source_repo_path"}


def exported_legacy_id(comment: dict, taken: set):
    """An imported comment keeps its id, unless it collides with a derived one of ours."""
    number = comment.get("legacy_id")
    if isinstance(number, int) and not isinstance(number, bool) and number in taken:
        while number in taken:
            number += 1
    return number


def export_records(st: State) -> list[dict]:
    """Beads-compatible JSONL records, so `bv` and other Beads readers keep working."""
    # A comment made here exports an integer derived from its event id, so every machine and every
    # later export agree on it. The range (2^47 upwards, 48 bits) keeps clear of the small ids Beads
    # hands out and makes two comments sharing a number a one-in-10^14 event; should it happen, the
    # tie is broken by the ids alone, never by arrival order. An imported id is kept as it came
    # unless it collides with one of these, which only an export of ours re-imported can do.
    new_ids, taken = {}, set()
    for comment in sorted((c for i in st.issues.values() for c in i["comments"] if c["uid"]), key=lambda c: c["uid"]):
        number = COMMENT_ID_BASE + int.from_bytes(hashlib.sha256(comment["uid"].encode()).digest()[:6], "big") % COMMENT_ID_BASE
        while number in taken:
            number += 1
        taken.add(number)
        new_ids[comment["uid"]] = number
    records = []
    for issue in sorted(st.issues.values(), key=lambda i: i["id"]):
        rec = {k: v for k, v in issue["extra"].items() if k not in RESERVED_EXPORT}  # extras first, never over the record
        rec["id"] = issue["id"]
        for name in EXPORT_FIELDS:
            if issue.get(name) is not None:
                rec[name] = issue[name]
        # still unknown targets travel on as they were imported, under this record's (possibly renamed) id
        dangling = [{**d, "issue_id": issue["id"]} for d in issue["dangling"]]
        if issue["labels"]:
            rec["labels"] = sorted(issue["labels"])
        deps = [{"issue_id": issue["id"], "depends_on_id": st.key(to), "type": kind,
                 "created_at": meta.get("created_at"), "created_by": meta.get("created_by"),
                 "metadata": meta.get("metadata", "{}"), "thread_id": meta.get("thread_id", "")}
                for (to, kind), meta in issue["deps"].items()]
        if deps or dangling:
            rec["dependencies"] = sorted(deps + dangling, key=lambda d: (d["depends_on_id"], d.get("type") or ""))
        if issue["comments"]:
            rec["comments"] = [{"id": new_ids[c["uid"]] if c["uid"] else exported_legacy_id(c, taken), "issue_id": issue["id"],
                                "author": c.get("author"), "text": c["text"], "created_at": c.get("created_at")}
                               for c in issue["comments"]]
        records.append(rec)
    return records
