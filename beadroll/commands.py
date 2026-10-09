"""The commands: argument shapes in, results out."""
from __future__ import annotations

import json
import os
import re
import sys
import time

from pathlib import Path

from .base import (
    BLOCKING, BeadError, DONE, EXPORT_FIELDS, FORMAT, ICON,
    IMPORT_FIELDS, MAX_NESTING, STATUSES, __version__, event_millis, event_time,
    git_text, hung_up, loads_strict, read_utf8, run_git, too_deep,
    uuid7, without_password,
)
from .machine import (dir_entries, 
    actor_label, find_workspace, home, machine_aliases, replica_id, segment_files,
    write_atomic,
)
from .model import (
    causal_order, conflicts, event_problem, is_done, load_journal, load_pending,
    payload_problem, remove_edge, replay, view, write_problem,
)
from .queries import (
    brief, descendants, detail, export_records, find_cycles, is_blocked, line,
    open_blockers, reaches,
)
from .sync import (coordinated, mint_key, publish_soon, synchronise, writing)


def need_tracker(st: State):
    if st.prefix is None:
        raise BeadError("NOT_INITIALIZED", "no tracker for this repository yet: run `bead init` or `bead sync`")


def parse_priority(value) -> int:
    match = re.fullmatch(r"[Pp]?([0-4])", str(value))
    if not match:
        raise BeadError("VALIDATION", f"priority must be 0-4, got {value!r}")
    return int(match.group(1))


def text_arg(args, name: str) -> str | None:
    path = getattr(args, f"{name}_file", None)
    if path:
        return read_utf8(path, name)
    return getattr(args, name, None)


def split_list(values) -> list[str]:
    return [item.strip() for value in values or [] for item in value.split(",") if item.strip()]


def field_args(args) -> dict:
    fields = {}
    for name in ("title", "description", "acceptance_criteria", "notes", "design", "assignee", "external_ref"):
        value = text_arg(args, name)
        if value is not None:
            fields[name] = value
    if getattr(args, "priority", None) is not None:
        fields["priority"] = parse_priority(args.priority)
    if getattr(args, "type", None):
        fields["issue_type"] = args.type
    if getattr(args, "estimate", None) is not None:
        fields["estimated_minutes"] = args.estimate
    if getattr(args, "status", None):
        if args.status not in STATUSES:
            raise BeadError("VALIDATION", f"unknown status {args.status!r}")
        fields["status"] = args.status
    return fields


def cmd_init(args, ws, actor):
    notes = []
    if args.remote:
        args.remote, had_password = without_password(args.remote)
        if (not os.path.isabs(args.remote) and os.path.exists(args.remote)
                and run_git(["remote", "get-url", args.remote], check=False).returncode):
            # a path typed relative to the cwd; a stored one is read from the top of the checkout
            real, top = os.path.realpath(args.remote), run_git(["rev-parse", "--show-toplevel"], check=False)
            top_dir = os.path.realpath(top.stdout.decode().strip()) if top.returncode == 0 else None
            args.remote = os.path.relpath(real, top_dir) if args.tracked and top_dir else real
        if had_password:
            notes.append("the password was dropped from the remote URL: bead never stores one; use a git credential helper")
    if args.tracked:
        top = git_text(["rev-parse", "--show-toplevel"])
        config = {**ws.config, "workspace": ws.config.get("workspace") or uuid7()}  # keep `where`, `ref` and keys we do not know
        if args.remote:
            config["remote"] = args.remote
        elif config.get("remote"):
            config["remote"] = without_password(config["remote"])[0]  # never write a password back, even a hand-set one
        write_atomic(Path(top, ".beadroll.json"), (json.dumps(config, indent=1) + "\n").encode(), private=False)
        ws = find_workspace()
    with writing(ws, actor) as tx:
        if tx.state.prefix is None:
            prefix = args.prefix or re.sub(r"[^a-z0-9]+", "-", (ws.name or "bead").lower()).strip("-") or "bead"
            tx.emit("init", data={"prefix": prefix, "name": ws.name or prefix})
        imported = import_records(tx, args.import_path) if args.import_path else 0
        st = tx.state
    # `init --tracked --remote` is the deliberate switch: record it, so the tracked remote is the one in use
    tracked_remote = ws.resolve_remote(args.remote) if args.tracked and args.remote else None
    ws.save_meta(name=ws.name, remote=tracked_remote if args.tracked else args.remote)
    if ws.wrote and not getattr(args, "offline", False):  # the tracker exists for the other machines now, not at a later sync
        warning = publish_soon(ws, args, actor)
        if warning:
            notes.append(warning)
    return ({"workspace": ws.id, "prefix": st.prefix, "path": str(ws.dir), "imported": imported,
             "tracked_config": bool(ws.config)},
            [f"Tracker {st.prefix!r} at {ws.dir}" + (f" ({imported} issues imported)" if imported else "")]
            + (["Wrote .beadroll.json: commit it so every checkout and machine uses this workspace"] if args.tracked else [])
            + notes)


PROBE_ID = "00000000-0000-7000-8000-000000000000"  # a would-be event, checked before it is minted


def import_records(tx: Tx, path: str) -> int:
    """Load a Beads JSONL export, keeping ids, timestamps, relationships and unknown fields."""
    try:
        # records end at "\n" only: U+2028, U+2029 and U+0085 are ordinary text inside a JSON string
        records = [loads_strict(row) for row in read_utf8(path, path).split("\n") if row.strip()]
    except ValueError as error:
        raise BeadError("VALIDATION", f"{path} is not valid JSONL: {error}") from None
    for number, rec in enumerate(records, 1):
        if not isinstance(rec, dict) or not isinstance(rec.get("id"), str) or not rec["id"]:
            raise BeadError("VALIDATION", f"{path}:{number}: record has no string 'id'")
        comments = rec.get("comments") or []
        if not isinstance(comments, list) or any(not isinstance(c, dict) for c in comments):
            raise BeadError("VALIDATION", f"{path}:{number}: record {rec['id']!r} has malformed comments")
        deps = rec.get("dependencies") or []
        if not isinstance(deps, list) or any(not isinstance(d, dict) or not isinstance(d.get("depends_on_id"), str)
                                             or not d["depends_on_id"] or not isinstance(d.get("type"), (str, type(None)))
                                             for d in deps):
            raise BeadError("VALIDATION", f"{path}:{number}: record {rec['id']!r} has a malformed dependency")
        if any(d["depends_on_id"] == rec["id"] for d in deps):  # as `dep add` and br refuse it
            raise BeadError("VALIDATION", f"{path}:{number}: record {rec['id']!r} depends on itself")
        if too_deep(rec, MAX_NESTING - 2):  # its create event holds it two levels down (data, then extra)
            raise BeadError("VALIDATION", f"{path}:{number}: record {rec['id']!r} nests more than {MAX_NESTING - 2} levels deep")
    first = {}
    for number, rec in enumerate(records, 1):  # one id is one issue; br refuses a file that repeats one too
        if first.setdefault(rec["id"], number) != number:
            raise BeadError("VALIDATION", f"{path}:{number}: record id {rec['id']!r} repeats record {first[rec['id']]}")
    known = dict(tx.state.by_key)
    fresh = [rec for rec in records if rec["id"] not in known]
    for rec in records:  # a known record that no longer lists a dependency we still hold by key takes it back
        if rec["id"] in known:
            listed = {(d["depends_on_id"], d.get("type") or "blocks") for d in rec.get("dependencies") or []}
            for dep in tx.state.issues[known[rec["id"]]]["dangling"]:
                if (dep["depends_on_id"], dep.get("type") or "blocks") not in listed:
                    tx.emit("dep_remove", known[rec["id"]], {"key": dep["depends_on_id"], "type": dep.get("type") or "blocks"})
    all_ids = set(known) | {rec["id"] for rec in fresh}
    created = {}
    for rec in fresh:
        extra = {k: v for k, v in rec.items()
                 if k not in IMPORT_FIELDS and k not in ("id", "labels", "dependencies", "comments", "source_repo_path")}
        dangling = [{**d, "type": d.get("type") or "blocks"} for d in rec.get("dependencies") or []
                    if isinstance(d, dict) and isinstance(d.get("depends_on_id"), str) and d["depends_on_id"] not in all_ids]
        comments = [{"legacy_id": c.get("id"), "author": c.get("author"), "text": c.get("text", ""),
                     "created_at": c.get("created_at")} for c in rec.get("comments") or []]
        data = {"key": rec["id"], "fields": {k: rec[k] for k in EXPORT_FIELDS if k in rec},
                "labels": rec.get("labels") or [], "comments": comments, "extra": extra, "imported": True,
                **({"dangling": dangling} if dangling else {})}
        probe = {"v": FORMAT, "id": PROBE_ID, "replica": tx.replica, "deps": [], "op": "create", "issue": PROBE_ID, "data": data}
        problem = event_problem(probe) or payload_problem(probe) or write_problem(probe)
        if problem:  # nothing has been appended yet: the import fails whole
            raise BeadError("VALIDATION", f"record {rec['id']!r} cannot be imported: {problem}")
        created[rec["id"]] = tx.emit("create", data=data)["id"]
    uid_of = {**known, **created}
    for rec in fresh:
        for dep in rec.get("dependencies") or []:
            if dep["depends_on_id"] in uid_of:
                meta = {k: dep[k] for k in ("created_at", "created_by", "metadata", "thread_id") if k in dep}
                tx.emit("dep_add", created[rec["id"]],
                        {"to": uid_of[dep["depends_on_id"]], "type": dep.get("type") or "blocks",
                         "meta": meta, "imported": True})
    return len(fresh)


def cmd_import(args, ws, actor):
    with writing(ws, actor) as tx:
        need_tracker(tx.state)
        count = import_records(tx, args.path)
    return {"imported": count}, [f"Imported {count} issues"]


def cmd_export(args, ws, actor):
    st = view(ws)
    text = "".join(json.dumps(rec, ensure_ascii=False, separators=(",", ":"), allow_nan=False) + "\n" for rec in export_records(st))
    if args.output:
        write_atomic(Path(args.output), text.encode(), private=False)
        return {"exported": len(st.issues), "path": args.output}, [f"Exported {len(st.issues)} issues to {args.output}"]
    try:
        sys.stdout.write(text)
    except OSError as error:
        raise hung_up(error, sys.stdout) from None
    return None, None


def cmd_create(args, ws, actor):
    title = args.title or args.title_pos
    if not title:
        raise BeadError("VALIDATION", "a title is required")
    fields = {**field_args(args), "title": title}
    if fields.get("status") == "in_progress":
        raise BeadError("VALIDATION", "create the issue, then claim it: in_progress is reached only through a claim")
    with writing(ws, actor) as tx:
        st = tx.state
        need_tracker(st)
        parent = st.resolve(args.parent) if args.parent else None
        deps = []
        for spec in split_list(args.deps):
            kind, _, target = spec.rpartition(":")
            deps.append((st.resolve(target)["uid"], kind or "blocks"))
        for target, kind in deps:
            if parent and kind in BLOCKING and (target == parent["uid"] or reaches(st, target, parent["uid"])):
                raise BeadError("CYCLE", f"depending on {st.key(target)} would make {parent['id']} wait for itself")
        uid = uuid7()
        data = {"key": mint_key(st, uid, parent), "fields": fields, "labels": split_list(args.labels)}
        tx.emit("create", data=data, event_id=uid)
        if parent:
            tx.emit("dep_add", uid, {"to": parent["uid"], "type": "parent-child"})
        for target, kind in deps:
            tx.emit("dep_add", uid, {"to": target, "type": kind})
        issue = tx.state.issues[uid]
        result = detail(tx.state, issue)
    if args.silent:
        return {"id": issue["id"]}, [issue["id"]]
    return result, [f"✓ Created {issue['id']}: {issue['title']}"]


def claim_builder(args, actor, keys, takeover=False):
    """The claim step of a coordinated transaction: online by default, settled by who updates
    the coordination ref first."""
    if takeover and getattr(args, "offline", False):
        raise BeadError("VALIDATION", "a takeover has to be coordinated online")
    if takeover and not args.reason:
        raise BeadError("VALIDATION", "a takeover needs --reason")

    def build(tx, mode):
        st = tx.state
        need_tracker(st)
        me = actor_label(actor, tx.replica)
        for key in keys:
            issue = st.resolve(key)
            held = issue["claim"]
            if is_done(st, issue):
                raise BeadError("VALIDATION", f"{issue['id']} is {issue['status']}")
            if "status" in conflicts(st, issue):
                raise BeadError("STATUS_CONFLICT", f"{issue['id']} has a disputed status; settle it before claiming")
            if held and held["owner"] == me:
                continue
            if held and not takeover:
                raise BeadError("ALREADY_CLAIMED", f"{issue['id']} is claimed by {held['owner']} "
                              f"(generation {held['generation']}, since {held['since']})")
            data = {"mode": mode, "generation": issue["claim_generation"] + 1}
            if takeover and held:
                data.update(takeover=held["id"], reason=args.reason)
            tx.emit("claim", issue["uid"], data)
        return mode

    return build


def claim_issues(args, ws, actor, keys, takeover=False) -> str:
    return coordinated(ws, actor, args, claim_builder(args, actor, keys, takeover))


CLAIM_NOTE = {"online": "confirmed", "local": "local tracker, no remote",
              "offline": "PROVISIONAL: not exclusive until claimed online"}


def still_held(st: State, keys, actor: dict):
    """A retry recognised from its receipt answers for a claim accepted earlier, which may have moved
    on since: say so rather than report a claim this actor no longer holds."""
    me = actor_label(actor, replica_id())
    for key in keys:
        issue = st.resolve(key)
        held = issue["claim"]
        if not held or held["owner"] != me:
            raise BeadError("ALREADY_CLAIMED" if held else "NOT_OWNER", f"{issue['id']}: your claim was accepted but "
                          + (f"is now held by {held['owner']}" if held else f"has since ended ({issue['status']})"))


def cmd_claim(args, ws, actor):
    mode = claim_issues(args, ws, actor, args.ids)
    st = view(ws)
    if mode != "offline":
        still_held(st, args.ids, actor)
    result = [detail(st, st.resolve(key)) for key in args.ids]
    return result, [f"Claimed {r['id']}: {r['title']} ({CLAIM_NOTE[mode]})" for r in result]


def cmd_takeover(args, ws, actor):
    claim_issues(args, ws, actor, args.ids, takeover=True)
    st = view(ws)
    still_held(st, args.ids, actor)
    result = [detail(st, st.resolve(key)) for key in args.ids]
    return result, [f"Took over {r['id']} (generation {r['claim']['generation']}): {args.reason}" for r in result]


def cmd_release(args, ws, actor):
    def build(tx, mode):
        st = tx.state
        need_tracker(st)
        me = actor_label(actor, tx.replica)
        for key in args.ids:
            issue = st.resolve(key)
            held = issue["claim"]
            if held and held["owner"] == me:
                tx.emit("release", issue["uid"], {"claim": held["id"], **({"reason": args.reason} if args.reason else {})})
            elif any(p["owner"] == me for p in issue["provisional_claims"]):
                tx.emit("release", issue["uid"], {})
            else:
                raise BeadError("NOT_OWNER", f"{issue['id']} is not claimed by {me}"
                              + (f"; it is held by {held['owner']}" if held else ""))
        return mode

    coordinated(ws, actor, args, build)
    st = view(ws)
    result = [brief(st, st.resolve(key)) for key in args.ids]
    return result, [f"Released {r['id']}: {r['title']}" for r in result]


def cmd_claims(args, ws, actor):
    st = view(ws)
    need_tracker(st)
    aliases, now, rows = machine_aliases(), time.time(), []
    for issue in sorted(st.issues.values(), key=lambda i: i["id"]):
        held = issue["claim"]
        if held:
            last = st.last_by_thread.get((held["machine"], held["thread"])) or held["id"]
            rows.append({"id": issue["id"], "title": issue["title"], "owner": held["owner"],
                         "machine": aliases.get(held["machine"], held["machine"]), "agent": held["agent"],
                         "thread": held["thread"], "generation": held["generation"], "since": held["since"],
                         "branch": (held.get("where") or {}).get("branch"),
                         "state": "confirmed", "last_activity": event_time(last),
                         "idle_minutes": int((now - event_millis(last) / 1000) / 60)})
        for attempt in issue["provisional_claims"]:
            rows.append({"id": issue["id"], "title": issue["title"], "owner": attempt["owner"],
                         "since": attempt["since"], "state": "provisional"})
        for pending in issue["unconfirmed"]:
            rows.append({"id": issue["id"], "title": issue["title"], "owner": pending["by"],
                         "since": pending["at"], "state": f"unconfirmed {pending['op']}"})
    synced = ws.meta().get("last_sync")
    lines = [f"{r['id']}  {r['state']:<11} {r['owner']}  since {r['since'][:16]}"
             + (f"  gen {r['generation']}  last activity {r['last_activity'][:16]}" if "generation" in r else "")
             + (f"  on {r['branch']}" if r.get("branch") else "")
             for r in rows] or ["Nothing is claimed"]
    lines.append(f"(as of this machine's last sync: {synced or 'never'})")
    return {"claims": rows, "last_sync": synced}, lines


def ended_by(issue: dict, me: str) -> bool:
    """`me` ended the issue's last claim by closing or deleting it. The run that did so may have died
    before it published that, so a retry of the command publishes as promptly as the first run."""
    last = issue["claim_history"][-1] if issue["claim_history"] else None
    return bool(last) and last["ended_by"] == me and last["how"] in ("close", "delete")


def cmd_update(args, ws, actor):
    fields = field_args(args)
    status = fields.pop("status", None)
    if status == "tombstone":
        raise BeadError("VALIDATION", "use `bead delete` to tombstone an issue; it ends the claim as well")
    wants_claim = args.claim or status == "in_progress"  # in_progress is reached only through a claim
    if status == "in_progress":
        status = None

    def edits(tx):
        """Validate every target first, then emit; nothing is written if any part is wrong."""
        st = tx.state
        need_tracker(st)
        me = actor_label(actor, tx.replica)
        targets = [st.resolve(key) for key in args.ids]
        new_parent = st.resolve(args.parent)["uid"] if args.parent else None
        for issue in targets:
            held = issue["claim"]
            if new_parent and (new_parent == issue["uid"] or reaches(st, issue["uid"], new_parent)):
                raise BeadError("CYCLE", f"{issue['id']} cannot become a child of {st.key(new_parent)}: it would wait for itself")
            if status == "closed" and (held or ended_by(issue, me)):
                ended.append(issue["id"])
            if status and held and held["owner"] != me and not args.force:
                raise BeadError("CLAIMED_BY_OTHER", f"{issue['id']} is claimed by {held['owner']}: "
                              "use `bead takeover` first, or --force")
            if status == "open" and held and held["owner"] == me:
                raise BeadError("VALIDATION", f"{issue['id']}: use `bead release` to give up your claim")
        for issue in targets:
            uid, held = issue["uid"], issue["claim"]
            if fields:
                tx.emit("set", uid, {"fields": fields})
            if status == "closed":  # the same transition `bead close` makes, ending exactly our claim
                tx.emit("close", uid, {"claim": held["id"]} if held and held["owner"] == me else {})
            elif status == "open" and issue["status"] in DONE:
                tx.emit("reopen", uid, {})
            elif status:
                tx.emit("set", uid, {"fields": {"status": status}})
            if args.add_label:
                tx.emit("label_add", uid, {"labels": split_list(args.add_label)})
            if args.remove_label:
                tx.emit("label_remove", uid, {"labels": split_list(args.remove_label)})
            if args.set_labels is not None:
                tx.emit("label_set", uid, {"labels": split_list([args.set_labels])})
            if new_parent:
                for (to, kind) in list(issue["deps"]):
                    if kind == "parent-child":
                        remove_edge(tx, st, issue, to, kind)
                tx.emit("dep_add", uid, {"to": new_parent, "type": "parent-child"})

    notes, ended = [], []
    if wants_claim:
        claim = claim_builder(args, actor, args.ids)

        def build(tx, mode):  # the claim and its edits are one attempt: both land, or neither
            mode = claim(tx, mode)
            tx.conditional = True
            edits(tx)
            return mode

        mode = coordinated(ws, actor, args, build, request={"fields": fields, "status": status})
        if mode == "offline":
            notes.append("claim is PROVISIONAL: not exclusive until claimed online")
        elif status != "closed":  # a claiming close ends the claim on purpose
            still_held(view(ws), args.ids, actor)
    else:
        with writing(ws, actor) as tx:
            edits(tx)
    warning = publish_soon(ws, args, actor) if ended else None  # the same promptness `bead close` gives
    st = view(ws)
    result = [detail(st, st.resolve(key)) for key in args.ids]
    if warning:
        notes.append(warning)
        for r in result:
            r["unpublished"] = warning  # --json is told too: the other machines do not know yet
    return result, [f"Updated {r['id']}: {r['title']}" for r in result] + notes


def transition(op: str, verb: str):
    def command(args, ws, actor):
        ended, named = False, {}
        with writing(ws, actor) as tx:
            need_tracker(tx.state)
            me = actor_label(actor, tx.replica)
            for key in args.ids:
                issue = tx.state.resolve(key)
                held = issue["claim"]
                if held and held["owner"] != me and not args.force:
                    raise BeadError("CLAIMED_BY_OTHER", f"{issue['id']} is claimed by {held['owner']}: "
                                  "use `bead takeover` first, or --force")
                data = {"reason": args.reason} if args.reason else {}
                if op in ("close", "delete") and held and held["owner"] == me:
                    data["claim"] = named[issue["uid"]] = held["id"]  # ends exactly this claim, never a successor's
                ended = ended or bool(held) or (op in ("close", "delete") and ended_by(issue, me))
                tx.emit(op, issue["uid"], data)
            st = tx.state
            result = [brief(st, st.resolve(key)) for key in args.ids]
        warning = publish_soon(ws, args, actor) if ended else None
        notes = [warning] if warning else []
        if warning:
            for r in result:
                r["unpublished"] = warning  # --json is told too: the other machines do not know yet
        elif named:  # published: answer with what the remote made of it
            st = view(ws)
            lost = {uid for uid, claim in named.items()
                    if any(s["op"] == op and s.get("claim") == claim for s in st.issues[uid]["stale"])}
            if lost:
                result = [brief(st, st.resolve(key)) for key in args.ids]
                for r in result:
                    if st.resolve(r["id"])["uid"] in lost:
                        r["superseded"] = f"another owner took over before this {op} reached the remote; it is ignored"
                        notes.append(f"! {r['id']}: {r['superseded']} (see `bead show`, under stale)")
        return result, [f"✓ {verb} {r['id']}: {r['title']}" for r in result] + notes
    return command


def cmd_show(args, ws, actor):
    st = view(ws)
    need_tracker(st)
    result = [detail(st, st.resolve(key), check_merge=True) for key in args.ids]
    lines = []
    for item in result:
        lines.append(f"{ICON.get(item['status'], '?')} {item['id']} · {item['title']}   [P{item['priority']} · {item['status'].upper()}]")
        if item.get("where", {}).get("branch"):
            lines.append(f"  last worked on branch {item['where']['branch']} ({item['where']['commit'][:12]})")
        if item.get("closed_on"):
            merged = item.get("merged")
            lines.append(f"  closed on {item['closed_on'].get('branch') or 'detached HEAD'} at {(item['closed_on'].get('commit') or '')[:12]}"
                         + (" · merged into the default branch" if merged is True else
                            " · NOT yet on the default branch" if merged is False else ""))
        lines.append(f"  type {item['issue_type']} · created {item['created_at'][:10]} by {item.get('created_by') or 'unknown'}"
                     + (f" · assignee {item['assignee']}" if item.get("assignee") else ""))
        if item.get("claim"):
            held = item["claim"]
            lines.append(f"  claimed by {held['owner']} · generation {held['generation']} · since {held['since'][:16]}"
                         + (f" · on branch {held['where']['branch']}" if held.get("where", {}).get("branch") else ""))
        for pending in item.get("unconfirmed", []):
            lines.append(f"  {pending['op']} by {pending['by']} awaiting the remote's acknowledgement")
        for attempt in item.get("provisional_claims", []):
            lines.append(f"  provisional claim by {attempt['owner']} since {attempt['since'][:16]} (not exclusive)")
        for name in ("description", "acceptance_criteria", "notes", "design", "close_reason"):
            if item.get(name):
                lines.append(f"  {name}: {item[name]}")
        if item.get("labels"):
            lines.append("  labels: " + ", ".join(item["labels"]))
        for dep in item["dependencies"]:
            lines.append(f"  depends on ({dep['dependency_type']}): {dep['id']} [{dep['status']}] {dep['title']}")
        for dep in item.get("unresolved_dependencies", []):
            lines.append(f"  depends on ({dep['dependency_type']}): {dep['id']} (not an issue yet)")
        for dep in item["dependents"]:
            lines.append(f"  needed by ({dep['dependency_type']}): {dep['id']} [{dep['status']}] {dep['title']}")
        for comment in item["comments"]:
            lines.append(f"  [{(comment.get('created_at') or '?')[:16]}] {comment.get('author') or '?'}: {comment['text']}")
        for field, options in item.get("conflicts", {}).items():
            lines.append(f"  ⚠ {field} was changed on two machines at once: "
                         + " | ".join(f"{o['value']!r} by {o['by']}" for o in options))
        for lost in item.get("lost_claims", []):
            lines.append(f"  ! claim by {lost['by']} lost to {lost['held_by']}")
        for old in item.get("stale", []):
            lines.append(f"  ! ignored {old['op']} by {old['by']}: it named a claim that is no longer current")
    return result, lines


def select(st: State, args) -> list[dict]:
    statuses = set(split_list(getattr(args, "status", None)))
    want_all = getattr(args, "all", False) or "all" in statuses
    # resolved before anything is filtered: a parent that names no issue is an error, never an empty answer
    parent = st.resolve(args.parent)["uid"] if getattr(args, "parent", None) is not None else None
    items = []
    for issue in st.issues.values():
        if statuses and not want_all:
            if issue["status"] not in statuses:
                continue
        elif not want_all:
            if issue["status"] in DONE or (issue["status"] == "deferred" and not getattr(args, "deferred", False)):
                continue
        if getattr(args, "type", None) and issue["issue_type"] not in split_list(args.type):
            continue
        if getattr(args, "label", None) and not set(split_list(args.label)) <= issue["labels"]:
            continue
        if getattr(args, "priority", None) is not None and issue["priority"] != parse_priority(args.priority):
            continue
        if getattr(args, "assignee", None) and issue.get("assignee") != args.assignee:
            continue
        if getattr(args, "title_contains", None) and args.title_contains.lower() not in issue["title"].lower():
            continue
        if getattr(args, "id", None) and issue["id"] not in split_list(args.id):
            continue
        if parent is not None and (parent, "parent-child") not in issue["deps"]:
            continue
        items.append(issue)
    return sorted(items, key=lambda i: (i["priority"], i["created_at"], i["id"]))


def cmd_list(args, ws, actor):
    st = view(ws)
    need_tracker(st)
    items = select(st, args)
    shown = items if args.limit == 0 else items[:args.limit]
    return ({"issues": [brief(st, i) for i in shown], "total": len(items), "limit": args.limit, "offset": 0,
             "has_more": len(shown) < len(items)}, [line(st, i) for i in shown] or ["No issues"])


def cmd_ready(args, ws, actor):
    st = view(ws)
    need_tracker(st)
    root = args.epic if args.root is None else args.root  # the parser takes one scope at most
    # The subtree decides membership only: readiness is judged on the whole tracker, so a blocker
    # outside it (on the root, an ancestor or a second parent) still holds an issue inside it.
    beneath = descendants(st, st.resolve(root)["uid"]) if root is not None else None
    args.status = ["open"]
    items = [i for i in select(st, args) if (beneath is None or i["uid"] in beneath)
             and not i["claim"] and not is_blocked(st, i) and "status" not in conflicts(st, i)]
    if args.limit:
        items = items[:args.limit]
    return [brief(st, i) for i in items], [line(st, i) for i in items] or ["Nothing is ready"]


def cmd_blocked(args, ws, actor):
    st = view(ws)
    need_tracker(st)
    items = [i for i in st.issues.values() if i["status"] not in DONE and is_blocked(st, i)]
    result = [{**brief(st, i), "blocked_by": [b["id"] for b in open_blockers(st, i)]}
              for i in sorted(items, key=lambda i: i["id"])]
    return result, [f"{line(st, st.resolve(r['id']))}  ⟵ {', '.join(r['blocked_by'])}" for r in result] or ["Nothing is blocked"]


def cmd_search(args, ws, actor):
    st = view(ws)
    need_tracker(st)
    needle = args.query.lower()
    hits = []
    for issue in st.issues.values():
        if issue["status"] in DONE and not args.all:
            continue
        haystack = [issue["id"], issue["title"]] + [str(issue.get(k) or "") for k in
                    ("description", "acceptance_criteria", "notes", "design", "external_ref")]
        haystack += [c["text"] for c in issue["comments"]]
        if any(needle in text.lower() for text in haystack):
            hits.append(issue)
    hits.sort(key=lambda i: (i["priority"], i["id"]))
    return [brief(st, i) for i in hits], [line(st, i) for i in hits] or ["No matches"]


def cmd_dep(args, ws, actor):
    words = list(args.words)
    action = words.pop(0) if words and words[0] in ("add", "remove", "rm", "list", "tree", "cycles") else "list"
    if action in ("add", "remove", "rm"):
        if len(words) != 2:
            raise BeadError("VALIDATION", f"usage: bead dep {action} ISSUE DEPENDS_ON")
        with writing(ws, actor) as tx:
            st = tx.state
            need_tracker(st)
            issue = st.resolve(words[0])
            try:
                target = st.resolve(words[1])
            except BeadError:
                if action == "add" or not any(d["depends_on_id"] in (words[1], f"{st.prefix}-{words[1]}") for d in issue["dangling"]):
                    raise
                target = None  # an imported dependency on a key that is not an issue yet: it can still be taken back
            if action == "add":
                kind = args.type or "blocks"
                loop = (reaches(st, target["uid"], issue["uid"]) if kind in BLOCKING
                        else reaches(st, issue["uid"], target["uid"]) if kind == "parent-child" else False)
                if issue["uid"] == target["uid"] or loop:
                    raise BeadError("CYCLE", f"{issue['id']} -> {target['id']} would create a dependency cycle")
                tx.emit("dep_add", issue["uid"], {"to": target["uid"], "type": kind})
            elif target is None:
                key = next(d["depends_on_id"] for d in issue["dangling"] if d["depends_on_id"] in (words[1], f"{st.prefix}-{words[1]}"))
                tx.emit("dep_remove", issue["uid"], {"key": key, "type": args.type})
            else:
                remove_edge(tx, st, issue, target["uid"], args.type)
        done = "now depends on" if action == "add" else "no longer depends on"
        shown = target["id"] if target else key
        return ({"issue_id": issue["id"], "depends_on_id": shown, "action": action},
                [f"{issue['id']} {done} {shown}"])
    st = view(ws)
    need_tracker(st)
    if action == "cycles":
        cycles = find_cycles(st, args.blocking_only)
        return {"cycles": cycles, "count": len(cycles)}, [" -> ".join(c + c[:1]) for c in cycles] or ["No cycles"]
    if not words:
        raise BeadError("VALIDATION", f"usage: bead dep {action} ISSUE")
    issue = st.resolve(words[0])
    if action == "tree":
        lines, seen, work = [], set(), [(issue, 0)]
        while work:
            node, depth = work.pop()
            lines.append("  " * depth + line(st, node))
            if node["uid"] in seen:
                continue
            seen.add(node["uid"])
            children = [st.issues[to] for (to, _kind) in sorted(node["deps"]) if to in st.issues]
            work.extend((child, depth + 1) for child in reversed(children))
        return {"tree": lines}, lines
    info = detail(st, issue)
    result = {"dependencies": info["dependencies"], "dependents": info["dependents"],
              **({"unresolved_dependencies": info["unresolved_dependencies"]} if "unresolved_dependencies" in info else {})}
    return result, ([f"depends on {d['id']} ({d['dependency_type']})" for d in info["dependencies"]]
                    + [f"depends on {d['id']} ({d['dependency_type']}, not an issue yet)" for d in info.get("unresolved_dependencies", [])]
                    + [f"needed by {d['id']} ({d['dependency_type']})" for d in info["dependents"]]) or ["No dependencies"]


def cmd_comments(args, ws, actor):
    words = list(args.words)
    action = words.pop(0) if words and words[0] in ("add", "list") else "list"
    if not words:
        raise BeadError("VALIDATION", "usage: bead comments [add] ISSUE [TEXT]")
    if action == "add":
        text = args.message or (read_utf8(args.file, "the comment file") if args.file else " ".join(words[1:]))
        if not text.strip():
            raise BeadError("VALIDATION", "comment text is required")
        with writing(ws, actor) as tx:
            need_tracker(tx.state)
            issue = tx.state.resolve(words[0])
            event = tx.emit("comment", issue["uid"], {"text": text, **({"author": args.author} if args.author else {})})
        return {"id": event["id"], "issue_id": issue["id"], "text": text}, [f"Comment added to {issue['id']}"]
    st = view(ws)
    need_tracker(st)
    comments = detail(st, st.resolve(words[0]))["comments"]
    return comments, [f"[{(c.get('created_at') or '?')[:16]}] {c.get('author') or '?'}: {c['text']}" for c in comments] or ["No comments"]


def cmd_lint(args, ws, actor):
    st = view(ws)
    need_tracker(st)
    missing = [i for i in st.issues.values() if i["status"] not in DONE
               and i["issue_type"] in ("task", "bug", "feature") and not i.get("acceptance_criteria")]
    result = [{"id": i["id"], "title": i["title"], "problem": "missing acceptance criteria"}
              for i in sorted(missing, key=lambda i: i["id"])]
    return {"findings": result, "count": len(result)}, [f"{r['id']}: {r['problem']}" for r in result] or ["Clean"]


def cmd_log(args, ws, actor):
    journal = load_journal(ws)
    st = replay(journal)
    events, _ = causal_order(journal)
    aliases = machine_aliases()
    wanted = st.resolve(args.key)["uid"] if args.key else None
    rows = [e for e in events if e["op"] != "init" and (wanted is None or e.get("issue") == wanted)]
    rows = rows[-args.limit:] if args.limit else rows
    result = [{"at": event_time(e["id"]), "machine": aliases.get(e["replica"], e["replica"]),
               "agent": (e.get("actor") or {}).get("agent"), "thread": (e.get("actor") or {}).get("thread"),
               "op": e["op"], "issue": st.key(e.get("issue")), "event": e["id"],
               "branch": (e.get("where") or {}).get("branch")} for e in rows]
    return result, [f"{r['at'][:19]}  {r['machine']:<10} {r['agent'] or '?':<6} {(r['thread'] or '-')[:8]:<8}  "
                    f"{r['op']:<12} {r['issue']}" + (f"  ({r['branch']})" if r.get("branch") else "") for r in result] or ["No events"]


def cmd_info(args, ws, actor):
    journal = load_journal(ws)
    st = view(ws)
    replica = replica_id(create=False)
    try:
        remote = ws.remote()
    except BeadError as error:  # info is where a damaged meta.json is diagnosed, not where it stops
        remote = f"unknown ({error.code}: {error})"
    source = ("BEAD_WORKSPACE" if os.environ.get("BEAD_WORKSPACE") else "tracked .beadroll.json" if ws.config.get("workspace")
              else "origin url" if ws.origin else "local path")
    result = {"version": __version__, "workspace": ws.id, "workspace_source": source, "name": st.name or ws.name, "prefix": st.prefix,
              "path": str(ws.dir), "remote": remote, "ref": ws.ref, "machine": replica,
              "actor": actor, "actor_label": actor_label(actor, replica or "?"),
              "issues": len(st.issues), "events": len(journal.events),
              "events_by_machine": {rep: len(ids) for rep, ids in sorted(journal.order.items())},
              "segments": len(segment_files(ws)), "frontier": journal.heads(),
              "pending_ownership": [e["id"] for attempt in load_pending(ws) for e in attempt],
              "waiting_events": len(st.pending), "integrity": st.integrity,
              "renamed_keys": st.renames, "last_sync": ws.meta().get("last_sync")}
    damaged = sorted(p.name for p in dir_entries(ws.dir) if p.name.startswith("sync.git.damaged-"))
    if damaged:  # set aside by a rebuild; nothing in them is the only copy of anything, delete at leisure
        result["damaged_sync_repos"] = damaged
    return result, [f"{k}: {v}" for k, v in result.items()]


def cmd_machines(args, ws, actor):
    if args.name:
        ident, _, alias = args.name.partition("=")
        path = home() / "aliases.json"
        try:
            aliases = loads_strict(path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            aliases = {}
        except (OSError, ValueError) as error:  # never replace names we could not read
            raise BeadError("IO", f"{path} cannot be read ({error}); repair it before adding a name") from None
        if not isinstance(aliases, dict):
            raise BeadError("IO", f"{path} does not hold a JSON object; repair it before adding a name")
        aliases[ident] = alias
        write_atomic(path, json.dumps(aliases, indent=1).encode())
    journal, aliases = load_journal(ws), machine_aliases()
    rows = [{"machine": replica, "alias": aliases.get(replica), "events": len(ids), "last_event": event_time(max(ids))}
            for replica, ids in sorted(journal.order.items())]
    return rows, [f"{r['machine']}  {r.get('alias') or '(unnamed)':<14} {r['events']:>6} events  last {r['last_event'][:19]}"
                  for r in rows] or ["No machines yet"]


def cmd_sync(args, ws, actor):
    remote = ws.remote(args.remote)
    if not remote:
        raise BeadError("NO_REMOTE", "no coordination remote known for this tracker: pass --remote")
    result = synchronise(ws, remote, actor)
    lines = [f"Synced with {remote}: pulled {result['pulled']}, pushed {result['pushed']} event(s)"]
    lines += [f"  renamed {r['from']} -> {r['to']} (created on machine {r['replica']})" for r in result["renamed"]]
    lines += [f"  ! {path} differed from the published copy; ours was kept beside it and the published copy used"
              for path in result["forked_journals"]]
    if result["integrity"]:
        lines.append(f"  ! {result['integrity']} integrity problem(s); see `bead info`")
    return result, lines
