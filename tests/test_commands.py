"""Commands end to end: every verb, flags, JSON envelope, identity detection, branch tracking.

Gathered from: first independent audit (AUDIT), 2026-10-06; the author's suite; third independent review (REVIEW3), 2026-10-06. Docstrings and names that cite
finding ids (A.., R.., N.., C.., K.., P.., D..) refer to FINDINGS.md."""
import argparse
import collections
import concurrent.futures
import copy
import importlib.machinery
import importlib.util
import itertools
import json
import os
import random
import resource
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from pathlib import Path
from unittest import mock
from unittest.mock import patch

import stat
import textwrap
import tests.support as support
from tests.support import *  # noqa: F401,F403  (BEAD, Machine, Sandbox, load_module, git, code)


# ---- from test_poc.py: the author's suite
class BpTest(unittest.TestCase):
    def setUp(self):
        self.sandbox = Sandbox()
        self.addCleanup(self.sandbox.cleanup)
        self.a = Machine(self.sandbox, "a")

    def two_machines(self):
        """Machine a owns an initialised tracker with issues X and Y; machine b has synced it."""
        a = self.a
        a.bead("init", "--prefix", "ex")
        x = a.bead("create", "Issue X", "-d", "original")["id"]
        y = a.bead("create", "Issue Y")["id"]
        a.bead("sync")
        b = Machine(self.sandbox, "b")
        b.bead("sync")
        return a, b, x, y

    def settle(self, *machines):
        for machine in (*machines, *machines):
            machine.bead("sync")
        exports = [machine.export() for machine in machines]
        self.assertTrue(all(text == exports[0] for text in exports), "machines did not converge")
        views = [json.dumps(machine.bead("show", *[r["id"] for r in machine.bead("list", "--all", "--limit", "0")["issues"]]),
                            sort_keys=True) for machine in machines]
        self.assertTrue(all(view == views[0] for view in views), "machines disagree on claims or conflicts")

    # -- one machine, many checkouts ------------------------------------------------

    def test_worktrees_and_clones_share_one_tracker_and_checkouts_stay_clean(self):
        a = self.a
        primary = a.checkout
        wt1, wt2 = self.sandbox.root / "wt1", self.sandbox.root / "wt2"
        git("worktree", "add", "-q", str(wt1), "-b", "feat-1", cwd=primary)
        git("worktree", "add", "-q", str(wt2), "-b", "feat-2", cwd=primary)
        clone2 = self.sandbox.clone("a-second-clone")
        a.bead("init", "--prefix", "ex")
        for index, cwd in enumerate((primary, wt1, wt2, clone2)):
            a.bead("create", f"from checkout {index}", cwd=cwd)
        self.assertEqual(a.bead("list", cwd=wt2)["total"], 4)
        self.assertEqual(a.bead("list", cwd=clone2)["total"], 4)
        for cwd in (primary, wt1, wt2, clone2):
            self.assertEqual(git("status", "--porcelain", cwd=cwd), "")
            self.assertFalse((cwd / ".beads").exists())

    def test_parallel_writers_lose_nothing_and_claim_is_exclusive(self):
        a = self.a
        wt = self.sandbox.root / "wt"
        git("worktree", "add", "-q", str(wt), "-b", "feat", cwd=a.checkout)
        a.bead("init", "--prefix", "ex")
        jobs = [(cwd, f"job {n}") for n in range(15) for cwd in (a.checkout, wt)]
        with concurrent.futures.ThreadPoolExecutor(8) as pool:
            results = list(pool.map(lambda job: a.run("create", job[1], cwd=job[0]).returncode, jobs))
        self.assertEqual(results, [0] * 30)
        listed = a.bead("list", "--limit", "0")
        self.assertEqual(listed["total"], 30)
        self.assertEqual(len({i["id"] for i in listed["issues"]}), 30)
        target = listed["issues"][0]["id"]
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            claims = list(pool.map(lambda t: a.run("update", target, "--claim", "--json", thread=t),
                                   ["thread-one-0001", "thread-two-0002"]))
        self.assertEqual(sorted(c.returncode for c in claims), [0, 1])
        self.assertIn("ALREADY_CLAIMED", "".join(c.stderr for c in claims))

    def test_readiness_follows_the_rule_br_uses(self):
        a = self.a
        a.bead("init", "--prefix", "ex")
        gate = a.bead("create", "prerequisite")["id"]
        epic = a.bead("create", "epic", "-t", "epic")["id"]
        feature = a.bead("create", "feature with a subtask", "-t", "feature", "--parent", epic, "--deps", gate)["id"]
        subtask = a.bead("create", "subtask", "--parent", feature)["id"]
        plain = a.bead("create", "plain parent", "-t", "feature")["id"]
        a.bead("create", "its child", "--parent", plain)
        self.assertNotIn(epic, a.ready())      # an epic waits for its open children
        self.assertIn(plain, a.ready())        # any other parent does not
        self.assertNotIn(feature, a.ready())   # held by its prerequisite
        self.assertNotIn(subtask, a.ready())   # and so is everything beneath it
        a.bead("close", gate, "--reason", "done")
        self.assertLessEqual({feature, subtask}, a.ready())
        self.assertEqual(code(a.run("dep", "add", subtask, epic, "--json")), "CYCLE")  # epic already waits for it

    def test_reads_work_when_the_store_is_read_only(self):
        a = self.a
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "readable")["id"]
        subprocess.run(["chmod", "-R", "a-w", str(a.home)], check=True)
        try:
            self.assertEqual(a.show(key)["title"], "readable")
            self.assertEqual(a.bead("list")["total"], 1)
            self.assertEqual(len(a.bead("ready")), 1)
            self.assertNotEqual(a.run("create", "nope").returncode, 0)
        finally:
            subprocess.run(["chmod", "-R", "u+w", str(a.home)], check=True)

    # -- several machines: ordinary edits -------------------------------------------

    def test_two_machines_converge_and_flag_what_both_changed(self):
        a, b, x, y = self.two_machines()
        self.assertEqual(b.show(x)["description"], "original")
        # Both work offline on the same issues.
        a.bead("update", x, "--title", "title from a", "--priority", "0", "--add-label", "from-a")
        a.bead("comments", "add", x, "note from a")
        b.bead("update", x, "--description", "body from b", "--priority", "3", "--add-label", "from-b")
        b.bead("comments", "add", x, "note from b")
        b.bead("close", y, "--reason", "done on b")
        a.bead("comments", "add", y, "late note from a")
        self.settle(a, b)
        for machine in (a, b):
            issue = machine.show(x)
            self.assertEqual(issue["title"], "title from a")        # only a touched it
            self.assertEqual(issue["description"], "body from b")   # only b touched it
            self.assertEqual(issue["priority"], 3)                  # shown: the newer of the two
            self.assertEqual(set(issue["conflicts"]), {"priority"})  # and flagged, with both values
            self.assertEqual([o["value"] for o in issue["conflicts"]["priority"]], [0, 3])
            self.assertEqual(issue["labels"], ["from-a", "from-b"])
            self.assertEqual([c["text"] for c in issue["comments"]], ["note from a", "note from b"])
            closed = machine.show(y)
            self.assertEqual(closed["status"], "closed")
            self.assertEqual([c["text"] for c in closed["comments"]], ["late note from a"])
            self.assertEqual(machine.bead("list")["issues"][0]["conflicted"], ["priority"])
        a.bead("update", x, "--priority", "1")  # a write that has seen both settles it
        self.settle(a, b)
        for machine in (a, b):
            self.assertEqual(machine.show(x)["priority"], 1)
            self.assertNotIn("conflicts", machine.show(x))
        for cwd in (a.checkout, b.checkout):
            self.assertEqual(git("status", "--porcelain", cwd=cwd), "")
            self.assertEqual(git("branch", "-a", cwd=cwd).count("beads"), 0)  # the journal is not a branch

    def test_an_edit_made_after_seeing_another_is_not_a_conflict(self):
        a, b, x, _ = self.two_machines()
        a.bead("update", x, "--priority", "0")
        a.bead("sync")
        b.bead("sync")
        b.bead("update", x, "--priority", "3")
        self.settle(a, b)
        for machine in (a, b):
            self.assertEqual(machine.show(x)["priority"], 3)
            self.assertNotIn("conflicts", machine.show(x))

    def test_removing_a_label_only_removes_what_the_remover_saw(self):
        a, b, x, _ = self.two_machines()
        a.bead("update", x, "--add-label", "urgent")
        self.settle(a, b)
        b.bead("update", x, "--add-label", "urgent")     # b re-asserts it
        a.bead("update", x, "--remove-label", "urgent")  # a removes the one addition it has seen
        self.settle(a, b)
        self.assertEqual(a.show(x)["labels"], ["urgent"])
        a.bead("update", x, "--remove-label", "urgent")  # now a has seen every addition
        self.settle(a, b)
        self.assertNotIn("labels", b.show(x))

    def test_status_disagreement_holds_dependents_until_settled(self):
        a, b, x, y = self.two_machines()
        a.bead("dep", "add", y, x)
        self.settle(a, b)
        a.bead("close", x, "--reason", "done")
        b.bead("update", x, "--status", "deferred")
        self.settle(a, b)
        for machine in (a, b):
            self.assertEqual({o["value"] for o in machine.show(x)["conflicts"]["status"]}, {"closed", "deferred"})
            self.assertEqual(machine.ready(), set())  # x is disputed, and y still waits for it
        self.assertEqual(code(a.run("claim", x, "--json")), "STATUS_CONFLICT")
        a.bead("close", x, "--reason", "confirmed done")
        self.settle(a, b)
        for machine in (a, b):
            self.assertNotIn("conflicts", machine.show(x))
            self.assertEqual(machine.ready(), {y})

    def test_same_child_number_on_two_machines_is_renumbered_identically(self):
        a, b, x, _ = self.two_machines()
        self.assertEqual(a.bead("create", "child made on a", "--parent", x)["id"], f"{x}.1")
        self.assertEqual(b.bead("create", "child made on b", "--parent", x)["id"], f"{x}.1")
        a.bead("sync")
        report = b.bead("sync")
        self.assertEqual([(r["from"], r["to"]) for r in report["renamed"]], [(f"{x}.1", f"{x}.2")])
        self.settle(a, b)
        for machine in (a, b):
            self.assertEqual(machine.show(f"{x}.1")["title"], "child made on a")
            self.assertEqual(machine.show(f"{x}.2")["title"], "child made on b")
            self.assertEqual(machine.show(f"{x}.2")["parent"], x)

    def test_cycle_formed_by_a_merge_is_reported_and_never_ready(self):
        a, b, x, y = self.two_machines()
        a.bead("dep", "add", x, y)
        self.assertEqual(code(a.run("dep", "add", y, x, "--json")), "CYCLE")  # refused locally
        b.bead("dep", "add", y, x)  # valid on b, which has not seen a's edge
        self.settle(a, b)
        for machine in (a, b):
            self.assertEqual(machine.bead("dep", "cycles")["count"], 1)
            self.assertEqual(machine.bead("ready"), [])

    def test_concurrent_sync_from_three_machines_all_succeed(self):
        a, b, x, _ = self.two_machines()
        c = Machine(self.sandbox, "c")
        c.bead("sync")
        for machine in (a, b, c):
            for n in range(3):
                machine.bead("create", f"{machine.name} item {n}")
        with concurrent.futures.ThreadPoolExecutor(3) as pool:
            codes = list(pool.map(lambda m: m.run("sync", "--json").returncode, (a, b, c)))
        self.assertEqual(codes, [0, 0, 0])
        self.settle(a, b, c)
        self.assertEqual(a.bead("list", "--limit", "0")["total"], 11)

    # -- claims -----------------------------------------------------------------------

    def test_online_claim_is_exclusive_across_machines(self):
        a, b, x, y = self.two_machines()
        first = a.bead("claim", x)[0]["claim"]
        self.assertEqual(first["generation"], 1)
        self.assertEqual(code(b.run("claim", x, "--json")), "ALREADY_CLAIMED")  # b never synced by hand
        self.assertEqual(b.show(x)["claim"]["owner"], first["owner"])
        # The same instant, from two machines, the way agents usually claim.
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            attempts = list(pool.map(lambda m: m.run("update", y, "--status", "in_progress", "--json"), (a, b)))
        self.assertEqual(sorted(p.returncode for p in attempts), [0, 1])
        self.assertEqual(code(next(p for p in attempts if p.returncode)), "ALREADY_CLAIMED")
        self.settle(a, b)
        self.assertEqual(a.show(y)["claim"], b.show(y)["claim"])
        self.assertNotIn("lost_claims", a.show(y))  # the refused attempt left no trace in the journal

    def test_claim_lasts_until_release_close_or_takeover(self):
        a, b, x, _ = self.two_machines()
        self.assertEqual(code(a.run("create", "shortcut", "--status", "in_progress", "--json")), "VALIDATION")
        a.bead("claim", x)
        self.assertEqual([(c["id"], c["state"], c["generation"]) for c in b.bead("claims")["claims"]], [])
        b.bead("sync")
        self.assertEqual([(c["id"], c["state"], c["generation"]) for c in b.bead("claims")["claims"]],
                         [(x, "confirmed", 1)])
        self.assertEqual(code(b.run("close", x, "--json")), "CLAIMED_BY_OTHER")
        self.assertEqual(code(b.run("release", x, "--json")), "NOT_OWNER")
        a.bead("release", x, "--reason", "parking it")
        self.assertEqual(a.show(x)["status"], "open")
        self.assertEqual(b.bead("claim", x)[0]["claim"]["generation"], 2)
        b.bead("close", x, "--reason", "done")
        self.settle(a, b)
        for machine in (a, b):
            self.assertEqual(machine.show(x)["status"], "closed")
            self.assertEqual(machine.bead("claims")["claims"], [])

    def test_takeover_fences_the_previous_owner(self):
        a, b, x, y = self.two_machines()
        a.bead("claim", x)
        a.bead("claim", y)
        self.assertEqual(code(b.run("takeover", x, "--json")), "VALIDATION")  # a reason is required
        a.bead("close", y, "--reason", "finished", "--offline")  # written before the takeover, not yet published
        for key in (x, y):
            taken = b.bead("takeover", key, "--reason", "owner crashed; continuing", thread="b-developer")[0]
            self.assertEqual(taken["claim"]["generation"], 2)
        # The old owner comes back and acts on what it still believes.
        self.assertEqual(a.run("close", x, "--reason", "finished", "--json").returncode, 0)
        self.assertEqual(code(a.run("release", x, "--json")), "NOT_OWNER")
        self.settle(a, b)
        for machine in (a, b):
            for key in (x, y):  # neither close took effect, whichever was written first
                issue = machine.show(key)
                self.assertEqual(issue["status"], "in_progress")
                self.assertEqual(issue["claim"]["generation"], 2)
                self.assertIn("b-develo", issue["claim"]["owner"])
                self.assertEqual([s["op"] for s in issue["stale"]], ["close"])
        b.bead("close", x, "--reason", "done", thread="b-developer")
        self.settle(a, b)
        self.assertEqual(a.show(x)["status"], "closed")

    def test_offline_claims_are_provisional(self):
        a, b, x, y = self.two_machines()
        a.bead("claim", x, "--offline")
        b.bead("claim", x, "--offline")
        self.settle(a, b)
        for machine in (a, b):
            issue = machine.show(x)
            self.assertNotIn("claim", issue)  # nobody owns it
            self.assertEqual(len(issue["provisional_claims"]), 2)
            self.assertEqual(issue["status"], "in_progress")
        a.bead("claim", x)  # reconnecting means claiming normally
        self.settle(a, b)
        for machine in (a, b):
            issue = machine.show(x)
            self.assertIn("a-thread", issue["claim"]["owner"])
            self.assertEqual(len(issue["provisional_claims"]), 1)
        a.bead("claim", y)
        b.bead("claim", y, "--offline")  # b cannot see a's claim yet
        self.settle(a, b)
        for machine in (a, b):  # a provisional claim never displaces a confirmed owner
            self.assertIn("a-thread", machine.show(y)["claim"]["owner"])
            self.assertEqual(len(machine.show(y)["provisional_claims"]), 1)

    def test_lost_acknowledgement_is_recovered_on_retry(self):
        a, b, x, _ = self.two_machines()
        crashed = a.run("claim", x, BEAD_TEST_CRASH_AFTER_PUSH="1")  # the remote accepted; a never heard back
        self.assertEqual(crashed.returncode, 99)
        self.assertNotIn("claim", a.show(x))
        again = a.bead("claim", x)[0]["claim"]
        self.assertEqual(again["generation"], 1)
        self.assertEqual([r["op"] for r in a.bead("log", x)].count("claim"), 1)  # found, not repeated
        b.bead("sync")
        self.assertEqual(b.show(x)["claim"]["owner"], again["owner"])

    def test_local_work_after_a_lost_acknowledgement_does_not_fork_the_journal(self):
        a, b, x, _ = self.two_machines()
        crashed = a.run("claim", x, BEAD_TEST_CRASH_AFTER_PUSH="1")
        self.assertEqual(crashed.returncode, 99)
        self.assertNotIn("claim", a.show(x))                 # not counted until the remote confirms it
        self.assertEqual(a.bead("claims")["claims"][0]["state"], "unconfirmed claim")
        a.bead("comments", "add", x, "work continues before anyone syncs")  # appends after the claim, no gap
        b.bead("sync")
        accepted = b.show(x)["claim"]
        self.assertEqual(a.bead("sync")["pulled"], 0)          # nothing to pull: the remote already had our claim
        self.assertEqual(a.show(x)["claim"], accepted)
        self.settle(a, b)
        self.assertEqual([c["text"] for c in b.show(x)["comments"]], ["work continues before anyone syncs"])

    def test_a_refused_claim_leaves_no_trace_anywhere(self):
        a, b, x, _ = self.two_machines()
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            attempts = list(pool.map(lambda m: m.run("claim", x, "--json"), (a, b)))
        self.assertEqual(sorted(p.returncode for p in attempts), [0, 1])
        winner = b if attempts[0].returncode else a
        self.settle(a, b)
        for machine in (a, b):
            claims = [r for r in machine.bead("log", x) if r["op"] == "claim"]
            self.assertEqual(len(claims), 1)                  # the refused attempt was taken back, not recorded
            self.assertIn(f"{winner.name}-thread", claims[0]["thread"])
            self.assertEqual(machine.show(x)["claim"], winner.show(x)["claim"])
            self.assertNotIn("lost_claims", machine.show(x))
            self.assertEqual(machine.bead("info")["integrity"], [])

    def test_an_interrupted_append_is_repaired_before_the_next_write(self):
        a = self.a
        a.bead("init", "--prefix", "ex")
        a.bead("create", "complete record")
        segment = next((a.store() / "events").glob("*/*.jsonl"))
        with open(segment, "ab") as fh:
            fh.write(b'{"v": 1, "id": "01a0c450-0000-7000-8000-00000000', )  # a crash mid-write
        self.assertEqual(a.bead("list")["total"], 1)             # readers skip the torn tail
        a.bead("create", "written after the interruption")
        self.assertEqual(a.bead("list")["total"], 2)             # and the next write did not poison the file
        self.assertTrue(list(segment.parent.glob("*.torn-*")))  # the fragment is kept for diagnosis

    def test_a_crash_before_the_claim_is_written_leaves_no_false_owner(self):
        a, b, x, _ = self.two_machines()
        crashed = a.run("claim", x, BEAD_TEST_CRASH_BEFORE_PUSH="1")  # written as pending, never pushed
        self.assertEqual(crashed.returncode, 98)
        self.assertNotIn("claim", a.show(x))
        self.assertIn("b-thread", b.bead("claim", x)[0]["claim"]["owner"])  # the remote was never claimed
        self.assertEqual(a.bead("sync")["integrity"], 0)
        self.settle(a, b)
        self.assertIn("b-thread", a.show(x)["claim"]["owner"])

    def test_a_sync_that_fetched_before_a_lost_acknowledgement_does_not_void_it(self):
        a, b, x, _ = self.two_machines()
        slow = self.sandbox.root / "slow-git"
        slow.mkdir()
        (slow / "git").write_text('#!/bin/sh\n[ "$1" = fetch ] && sleep 2\nexec /usr/bin/git "$@"\n')
        (slow / "git").chmod(0o755)
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            syncing = pool.submit(a.run, "sync", "--json", PATH=f"{slow}:{os.environ['PATH']}", thread="a-sync")
            time.sleep(0.5)  # the sync is inside its first, slow fetch
            crashed = a.run("claim", x, BEAD_TEST_CRASH_AFTER_PUSH="1", thread="a-claimer")  # accepted, unacknowledged
            synced = syncing.result()
        self.assertEqual(crashed.returncode, 99)
        self.assertEqual(synced.returncode, 0, synced.stderr)
        b.bead("sync")
        self.assertIn("a-claimer", b.show(x)["claim"]["owner"])  # the accepted claim stands everywhere
        self.assertNotIn("void", [r["op"] for r in b.bead("log", x)])

    def test_closing_through_update_publishes_like_close(self):
        a, b, x, _ = self.two_machines()
        a.bead("claim", x)
        a.bead("update", x, "--status", "closed", "--notes", "done via update")
        b.bead("sync")
        self.assertEqual(b.show(x)["status"], "closed")
        self.assertNotIn("claim", b.show(x))

    def test_a_parent_change_cannot_form_a_loop(self):
        a, b, x, y = self.two_machines()
        self.assertEqual(code(a.run("update", x, "--parent", x, "--json")), "CYCLE")
        a.bead("update", y, "--parent", x)
        self.assertEqual(code(a.run("update", x, "--parent", y, "--json")), "CYCLE")
        self.assertEqual(a.bead("dep", "cycles")["count"], 0)

    def test_damaged_lines_and_payloads_are_reported_not_fatal(self):
        a = self.a
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "survivor")["id"]
        segment = next((a.store() / "events").glob("*/*.jsonl"))
        rows = segment.read_text().splitlines()
        rows.insert(1, "this is not json")
        segment.write_text("\n".join(rows) + "\n")
        self.assertEqual(a.show(key)["title"], "survivor")
        self.assertEqual(len(a.bead("info")["integrity"]), 1)
        a.bead("create", "still writable")
        self.assertEqual(a.bead("list")["total"], 2)

    def test_events_record_the_branch_and_a_close_knows_whether_it_is_merged(self):
        a = self.a
        a.bead("init", "--prefix", "ex")
        x = a.bead("create", "feature work")["id"]
        wt = self.sandbox.root / "wt-feature"
        git("worktree", "add", "-q", str(wt), "-b", "feature/x", cwd=a.checkout)
        (wt / "work.txt").write_text("done\n")
        git("add", "-A", cwd=wt)
        git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "implement x", cwd=wt)
        head = git("rev-parse", "HEAD", cwd=wt).strip()
        a.bead("claim", x, cwd=wt)
        a.bead("comments", "add", x, "halfway", cwd=wt)
        a.bead("close", x, "--reason", "done", cwd=wt)
        shown = a.show(x)  # asked from the main checkout, which has not merged the branch
        self.assertEqual(shown["closed_on"], {"branch": "feature/x", "commit": head})
        self.assertEqual(shown["comments"][0]["branch"], "feature/x")
        self.assertIs(shown["merged"], False)
        self.assertEqual([r["branch"] for r in a.bead("log", x)][-1], "feature/x")
        git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "merge", "-q", "feature/x", cwd=a.checkout)
        self.assertIs(a.show(x)["merged"], True)
        self.assertEqual(a.bead("claims")["claims"], [])

    def test_where_is_absent_outside_a_checkout_and_when_switched_off(self):
        a = self.a
        a.bead("init", "--prefix", "ex")
        store = a.store().name
        proc = subprocess.run([BEAD, "create", "from nowhere", "--json"], cwd=self.sandbox.root,
                              env=a.env(BEAD_WORKSPACE=store), capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertNotIn("where", a.show(json.loads(proc.stdout)["id"]))
        (a.checkout / ".beadroll.json").write_text(json.dumps({"workspace": store, "where": False}))
        quiet = a.bead("create", "no branch recorded")["id"]
        self.assertNotIn("where", a.show(quiet))

    def test_claim_needs_the_remote_unless_offline_is_asked_for(self):
        a, b, x, _ = self.two_machines()
        gone = str(self.sandbox.root / "unreachable.git")
        self.assertEqual(code(a.run("claim", x, "--remote", gone, "--json")), "REMOTE_UNREACHABLE")
        self.assertNotIn("claim", a.show(x))
        self.assertEqual(len(a.bead("claim", x, "--offline")[0]["provisional_claims"]), 1)

    # -- identity, integrity, storage -----------------------------------------------

    def test_every_event_carries_machine_agent_and_thread(self):
        a, b, x, _ = self.two_machines()
        b.bead("comments", "add", x, "from a claude session", agent="claude", thread="0f0f0f0f-claude-session")
        self.settle(a, b)
        rows = a.bead("log", x)
        self.assertEqual([(r["agent"], r["thread"]) for r in rows],
                         [("codex", "a-thread-0001"), ("claude", "0f0f0f0f-claude-session")])
        self.assertEqual(len({r["machine"] for r in rows}), 2)
        self.assertEqual(a.show(x)["origin"]["agent"], "codex")
        self.assertEqual(len(a.bead("machines")), 2)
        self.assertEqual(uuid.UUID(rows[0]["event"]).version, 7)

    def test_one_corrupt_event_does_not_stop_the_tracker(self):
        a, b, x, y = self.two_machines()
        store = a.store()
        events = [json.loads(row) for path in (store / "events").glob("*/*.jsonl") for row in path.read_text().splitlines()]
        victim = next(e for e in events if e["op"] == "create" and e["data"]["fields"]["title"] == "Issue X")
        forged = {**victim, "replica": "ffffffff", "data": {**victim["data"], "fields": {"title": "forged"}}}
        (store / "events" / "ffffffff").mkdir()
        (store / "events" / "ffffffff" / "000001.jsonl").write_text(json.dumps(forged) + "\n")
        info = a.bead("info")
        self.assertEqual(len(info["integrity"]), 1)  # the ambiguous event is reported, with both variants kept
        self.assertEqual(info["waiting_events"], 0)  # and nothing else is held up behind it
        self.assertEqual(a.show(y)["title"], "Issue Y")
        self.assertEqual(code(a.run("show", x, "--json")), "ISSUE_NOT_FOUND")
        a.bead("create", "work continues")
        self.assertEqual(len(a.ready()), 2)

    def test_a_copied_machine_identity_is_detected(self):
        a, b, x, _ = self.two_machines()
        twin = Machine(self.sandbox, "twin")
        shutil.copytree(a.home, twin.home)  # a restored backup or cloned disk: same identity, two places
        a.bead("create", "made on the original")
        twin.bead("create", "made on the copy")
        a.bead("sync")
        self.assertEqual(code(twin.run("sync", "--json")), "JOURNAL_FORK")
        b.bead("sync")
        self.assertEqual(b.bead("list")["total"], 3)  # everyone else carries on

    def test_full_segments_are_never_rewritten(self):
        a = self.a
        a.extra = {"BEAD_ROTATE_BYTES": "1500"}
        a.bead("init", "--prefix", "ex")
        for n in range(12):
            a.bead("create", f"issue {n}", "-d", "x" * 300)
        a.bead("sync")

        def published():
            rows = git("--git-dir", str(self.sandbox.origin), "ls-tree", "-r", "refs/beads/journal").splitlines()
            return {row.split("\t")[1]: row.split()[2] for row in rows}

        before = published()
        segments = sorted(p for p in before if p.startswith("events/"))
        self.assertGreater(len(segments), 3)
        a.bead("create", "one more")
        a.bead("sync")
        after = published()
        for path in segments[:-1]:
            self.assertEqual(before[path], after[path])
        b = Machine(self.sandbox, "b")
        b.bead("sync")
        self.assertEqual(b.bead("list", "--limit", "0")["total"], 13)

    def test_tracked_workspace_id_and_a_separate_coordination_remote(self):
        coordination = self.sandbox.bare("coordination.git")
        a = self.a
        a.bead("init", "--prefix", "ex", "--tracked", "--remote", str(coordination))
        a.bead("create", "shared")
        a.bead("sync")
        # A repository with a different origin joins the same tracker by carrying the same .beadroll.json.
        elsewhere = self.sandbox.clone("elsewhere", self.sandbox.bare("another-origin.git"))
        shutil.copy(a.checkout / ".beadroll.json", elsewhere / ".beadroll.json")
        b = Machine(self.sandbox, "b")
        b.bead("sync", cwd=elsewhere)
        self.assertEqual(b.bead("list", cwd=elsewhere)["total"], 1)
        self.assertEqual(b.bead("info", cwd=elsewhere)["workspace_source"], "tracked .beadroll.json")
        # Nothing reached the code remote, and the coordination history holds no code.
        self.assertEqual(git("ls-remote", str(self.sandbox.origin), "refs/beads/*"), "")
        names = git("--git-dir", str(coordination), "ls-tree", "-r", "--name-only", "refs/beads/journal").split()
        self.assertTrue(all(n == "manifest.json" or n.startswith("events/") for n in names))
        self.assertEqual(git("--git-dir", str(coordination), "rev-list", "--count", "--all").strip(),
                         git("--git-dir", str(coordination), "rev-list", "--count", "refs/beads/journal").strip())
        # A different workspace refuses to adopt this ref.
        c = Machine(self.sandbox, "c")
        c.bead("init", "--prefix", "zz")
        self.assertEqual(code(c.run("sync", "--remote", str(coordination), "--json")), "WORKSPACE_MISMATCH")

    def test_replay_does_not_depend_on_delivery_order(self):
        a, b, x, y = self.two_machines()
        c = Machine(self.sandbox, "c")
        c.bead("sync")
        a.bead("update", x, "--priority", "0", "--add-label", "one")
        b.bead("update", x, "--priority", "3", "--remove-label", "one", "--notes", "from b")
        c.bead("update", x, "--priority", "4")
        c.bead("create", "child", "--parent", y)
        b.bead("create", "other child", "--parent", y)
        a.bead("claim", y)
        b.bead("claim", y, "--offline")
        self.settle(a, b, c)
        a.bead("update", x, "--notes", "after seeing everything")
        a.bead("sync")

        bp = load_module()
        os.environ["BEAD_HOME"] = str(a.home)
        self.addCleanup(os.environ.pop, "BEAD_HOME", None)
        ws = bp.Workspace(a.store().name, None, None, {})
        journal = bp.load_journal(ws)

        def view(source):
            st = bp.replay(source)
            issues = sorted(st.issues.values(), key=lambda i: i["id"])
            return json.dumps([bp.export_records(st), [bp.detail(st, i) for i in issues]], sort_keys=True, default=sorted)

        expected = view(journal)
        ownership = [e for i, e in journal.events.items() if i not in {x for ids in journal.order.values() for x in ids}]
        for order in itertools.permutations(journal.order):
            shuffled = bp.Journal()
            for replica in (*order, *order):  # every event delivered twice
                for event_id in journal.order[replica]:
                    shuffled.add(journal.events[event_id], replica)
                for event in ownership:
                    if event["replica"] == replica:
                        shuffled.add(event, replica, sequenced=False)
            self.assertEqual(view(shuffled), expected)
        # An event whose cause has not arrived waits; it is never treated as concurrent.
        own = bp.replica_id()
        partial = bp.Journal()
        for event_id in journal.order[own]:
            partial.add(journal.events[event_id], own)
        waiting = bp.replay(partial).pending
        self.assertTrue(waiting)
        self.assertIn("after seeing everything", json.dumps(waiting))

    # -- beads compatibility --------------------------------------------------------

    def test_import_export_round_trips_beads_records(self):
        records = [
            {"id": "ex-aaa", "title": "Parent", "description": "d", "status": "open", "priority": 1,
             "issue_type": "epic", "created_at": "2026-01-02T03:04:05Z", "created_by": "someone",
             "updated_at": "2026-01-03T00:00:00Z", "source_repo": ".", "compaction_level": 0,
             "original_size": 0, "labels": ["alpha", "beta"]},
            {"id": "ex-aaa.1", "title": "Child", "acceptance_criteria": "it works", "status": "closed",
             "priority": 2, "issue_type": "task", "created_at": "2026-01-02T04:00:00Z",
             "created_by": "someone", "updated_at": "2026-01-04T00:00:00Z",
             "closed_at": "2026-01-04T00:00:00Z", "close_reason": "done", "external_ref": "audit:R-1",
             "source_repo": ".", "compaction_level": 0, "original_size": 0, "future_field": {"kept": True},
             "dependencies": [
                 {"issue_id": "ex-aaa.1", "depends_on_id": "ex-aaa", "type": "parent-child",
                  "created_at": "2026-01-02T04:00:00Z", "created_by": "someone", "metadata": "{}", "thread_id": ""},
                 {"issue_id": "ex-aaa.1", "depends_on_id": "ex-gone", "type": "blocks",
                  "created_at": "2026-01-02T04:00:00Z", "created_by": "someone", "metadata": "{}", "thread_id": ""}],
             "comments": [{"id": 41, "issue_id": "ex-aaa.1", "author": "someone", "text": "first",
                           "created_at": "2026-01-03T00:00:00Z"}]},
            {"id": "ex-zzz", "title": "Removed", "status": "tombstone", "priority": 3, "issue_type": "bug",
             "created_at": "2026-01-02T05:00:00Z", "created_by": "someone",
             "updated_at": "2026-01-05T00:00:00Z", "deleted_at": "2026-01-05T00:00:00Z",
             "deleted_by": "someone", "delete_reason": "duplicate", "source_repo": ".",
             "compaction_level": 0, "original_size": 0},
        ]
        source = self.sandbox.root / "issues.jsonl"
        source.write_text("".join(json.dumps({**r, "source_repo_path": "/private/machine/path"}) + "\n" for r in records))
        a = self.a
        self.assertEqual(a.bead("init", "--prefix", "ex", "--import", str(source))["imported"], 3)
        exported = [json.loads(row) for row in a.export().splitlines()]
        self.assertEqual(exported, records)  # everything but the machine-local path survives
        self.assertEqual(a.bead("import", str(source))["imported"], 0)  # importing again adds nothing
        a.bead("comments", "add", "ex-aaa.1", "second")
        comments = [json.loads(row) for row in a.export().splitlines()][1]["comments"]
        self.assertEqual(comments[0]["id"], 41)                      # a legacy id is kept as imported
        self.assertGreaterEqual(comments[1]["id"], 1_000_000)         # a new one is stable and out of Beads' range
        self.assertEqual(comments[1]["id"], [json.loads(row) for row in a.export().splitlines()][1]["comments"][1]["id"])
        self.assertEqual(a.ready(), {"ex-aaa"})  # the open epic, as in br
        self.assertEqual(a.bead("ready", "--type", "task"), [])


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class CompatAuditCLI(unittest.TestCase):
    def setUp(self):
        self.sandbox=support.Sandbox(); self.addCleanup(self.sandbox.cleanup)
        self.a=support.Machine(self.sandbox,'compat')
        self.a.bead('init','--prefix','ex')
        self.x=self.a.bead('create','Issue X')['id']
        self.y=self.a.bead('create','Issue Y')['id']

    def test_global_json_actor_positions(self):
        for args in [('--json','--actor','reviewer','info'),('info','--json','--actor','reviewer'),('--actor=reviewer','info','--json'),('info','--actor=reviewer','--json')]:
            with self.subTest(args=args):
                result=self.a.run(*args);self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(json.loads(result.stdout)['actor']['name'],'reviewer')

    def test_description_stdin_multi_id_and_filters(self):
        p=subprocess.run([support.BEAD,'update',self.x,self.y,'--description-file','-','--json'],cwd=self.a.checkout,env=self.a.env(),input='multiline\nbody\n',capture_output=True,text=True)
        self.assertEqual(p.returncode,0,p.stderr)
        self.assertEqual([r['description']for r in json.loads(p.stdout)],['multiline\nbody\n']*2)
        self.a.bead('update',self.x,'--add-label','red,green','--priority','0','--assignee','reviewer','--type','bug')
        self.assertEqual(self.a.bead('list','--limit','0')['total'],2)
        self.assertEqual([r['id']for r in self.a.bead('list','--label','red','--label','green','--priority','0','--assignee','reviewer','--type','bug','--title-contains','x')['issues']],[self.x])
        self.assertEqual([r['id']for r in self.a.bead('show',self.x,self.y)],[self.x,self.y])

    def test_multi_id_update_rolls_back_on_missing_issue(self):
        before=self.a.export();p=self.a.run('update',self.x,'missing','--title','changed','--json')
        self.assertEqual(p.returncode,1);self.assertEqual(support.code(p),'ISSUE_NOT_FOUND')
        self.assertEqual(self.a.export(),before)

    def test_domain_error_json_envelope(self):
        p=self.a.run('show','missing','--json')
        self.assertEqual(p.returncode,1);self.assertEqual(p.stdout,'')
        self.assertEqual(support.code(p),'ISSUE_NOT_FOUND')

    def test_parser_error_json_envelope(self):
        p=self.a.run('show','--json')
        self.assertEqual(p.returncode,2);self.assertEqual(p.stdout,'')
        self.assertTrue(p.stderr.lstrip().startswith('{'), 'parser errors bypass JSON error envelope: '+p.stderr)
        self.assertIn('error',json.loads(p.stderr))

    def test_global_options_respect_end_of_options(self):
        p=self.a.run('create','--','--json')
        self.assertEqual(p.returncode,0,'literal --json title was consumed: '+p.stderr)
        self.assertIn('--json',[r['title']for r in self.a.bead('list')['issues']])

    def test_claiming_update_validates_parent_before_committing_claim(self):
        p=self.a.run('update',self.x,'--claim','--parent','missing','--json')
        self.assertNotEqual(p.returncode,0)
        self.assertNotIn('claim',self.a.show(self.x),'failed update left an accepted online claim behind')

    def test_workspace_config_cannot_escape_home(self):
        for wid in [str(self.sandbox.root/'escaped-absolute'),'../escaped-relative']:
            with self.subTest(workspace=wid):
                (self.a.checkout/'.beadroll.json').write_text(json.dumps({'workspace':wid}))
                p=self.a.run('init','--prefix','evil','--json')
                self.assertNotEqual(p.returncode,0,'workspace id wrote outside BEAD_HOME: '+p.stdout)
                self.assertEqual(support.code(p),'CONFIG')

    def test_changed_workspace_config_does_not_delete_old_store(self):
        before=self.a.store();saved=self.a.export()
        (self.a.checkout/'.beadroll.json').write_text(json.dumps({'workspace':'second-workspace'}))
        p=self.a.run('show',self.x,'--json');self.assertEqual(support.code(p),'NOT_INITIALIZED')
        (self.a.checkout/'.beadroll.json').unlink()
        self.assertEqual(self.a.store(),before);self.assertEqual(self.a.export(),saved)

    def test_import_preserves_dependency_metadata_extra_and_tombstone_mutation(self):
        src=self.sandbox.root/'import.jsonl'
        rec={'id':'ex-z','title':'imported','status':'open','priority':2,'issue_type':'task','created_at':'2026-01-01T00:00:00Z','updated_at':'2026-01-01T00:00:00Z','created_by':'audit','future':{'kept':True},'dependencies':[{'issue_id':'ex-z','depends_on_id':self.x,'type':'blocks','created_at':'2026-01-01T00:00:00Z','created_by':'audit','metadata':'{"kept":true}','thread_id':'meta-thread'}]}
        src.write_text(json.dumps(rec)+'\n');self.a.bead('import',str(src));self.a.bead('delete','ex-z','--reason','duplicate')
        got=next(r for r in map(json.loads,self.a.export().splitlines())if r['id']=='ex-z')
        self.assertEqual(got['future'],{'kept':True});self.assertEqual(got['dependencies'],rec['dependencies'])
        self.assertEqual(got['status'],'tombstone');self.assertEqual(got['delete_reason'],'duplicate')

    def test_imported_comment_zero_id_roundtrips(self):
        src=self.sandbox.root/'comment-zero.jsonl';src.write_text(json.dumps({'id':'ex-z','title':'zero comment','comments':[{'id':0,'author':'audit','text':'zero','created_at':'2026-01-01T00:00:00Z'}]})+'\n')
        self.a.bead('import',str(src));p=self.a.run('export')
        self.assertEqual(p.returncode,0,p.stderr)
        self.assertEqual(next(r for r in map(json.loads,p.stdout.splitlines())if r['id']=='ex-z')['comments'][0]['id'],0)

    def test_metadata_excludes_local_machine_identifiers(self):
        self.a.bead('comments','add',self.x,'synthetic comment')
        events=[json.loads(l)for p in (self.a.store()/'events').glob('*/*.jsonl')for l in p.read_text().splitlines()]
        import socket,getpass
        for e in events:
            metadata={k:v for k,v in e.items()if k!='data'}
            text=json.dumps(metadata)
            for forbidden in [str(Path.home()),str(self.sandbox.root),socket.gethostname(),getpass.getuser()+'@']:
                self.assertNotIn(forbidden,text)
            self.assertRegex(e['replica'],r'^[0-9a-f]{8}$')


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class CompatAuditIdentity(unittest.TestCase):
    def test_detect_actor_environment_and_explicit_overrides(self):
        bp=support.load_module()
        fixtures=[({},None,{'agent':'human'}),({'CLAUDE_CODE_SESSION_ID':'claude-id'},None,{'agent':'claude','thread':'claude-id'}),({'CODEX_THREAD_ID':'thread','CODEX_SESSION_ID':'session'},None,{'agent':'codex','thread':'thread'}),({'CODEX_SESSION_ID':'session'},None,{'agent':'codex','thread':'session'}),({'BEAD_AGENT':'other','BEAD_THREAD':'stable','CODEX_THREAD_ID':'thread'},'name',{'agent':'other','thread':'stable','name':'name'})]
        for env,name,want in fixtures:
            with self.subTest(env=env),mock.patch.dict(os.environ,env,clear=True):self.assertEqual(bp.detect_actor(name),want)

    def test_nearest_harness_mocked_process_tree(self):
        bp=support.load_module()
        for proc,want in [('/opt/bin/claude --worker','claude'),('/opt/bin/codex --worker','codex'),('node /opt/claude/cli.js','claude')]:
            with self.subTest(proc=proc),mock.patch.dict(os.environ,{'CLAUDE_CODE_SESSION_ID':'claude','CODEX_THREAD_ID':'codex'},clear=True),mock.patch.object(bp.subprocess,'run',return_value=subprocess.CompletedProcess([],0,'1 '+proc+'\n','')):
                self.assertEqual(bp.detect_actor(None),{'agent':want,'thread':want})

    def test_missing_ps_does_not_crash_nested_actor_detection(self):
        import tempfile, shutil
        with tempfile.TemporaryDirectory(prefix='bp-identity-nops-') as tmp:
            root=Path(tmp);bin_dir=root/'bin';bin_dir.mkdir()
            (bin_dir/'python3').symlink_to(os.sys.executable)
            (bin_dir/'git').symlink_to(shutil.which('git'))
            env={k:v for k,v in os.environ.items()if not k.startswith(('BEAD_','CLAUDE','CODEX','GIT_'))}
            env.update(BEAD_HOME=str(root/'home'),BEAD_WORKSPACE='audit-identity',PATH=str(bin_dir),CLAUDE_CODE_SESSION_ID='claude',CODEX_THREAD_ID='codex')
            p=subprocess.run([support.BEAD,'info','--json'],cwd=root,env=env,capture_output=True,text=True)
            self.assertEqual(p.returncode,0,'missing ps aborts nested identity: '+p.stderr)
            self.assertEqual(json.loads(p.stdout)['actor'],{'agent':'codex','thread':'codex'})

    def test_bp_thread_overrides_detected_thread_without_bp_agent(self):
        bp=support.load_module()
        with mock.patch.dict(os.environ,{'BEAD_THREAD':'override','CODEX_THREAD_ID':'detected'},clear=True):
            self.assertEqual(bp.detect_actor(None),{'agent':'codex','thread':'override'})


# ---- from test_review3.py: third independent review (REVIEW3), 2026-10-06
class LocationReview(unittest.TestCase):
    def test_claims_and_log_report_claim_location_and_later_activity(self):
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "location-a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "located work")["id"]
        worktree = box.root / "located-feature"
        support.git("worktree", "add", "-q", str(worktree), "-b", "feature/location", cwd=a.checkout)
        (worktree / "change.txt").write_text("located\n")
        support.git("add", "-A", cwd=worktree)
        support.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q",
                "-m", "located change", cwd=worktree)
        commit = support.git("rev-parse", "HEAD", cwd=worktree).strip()

        claimed = a.bead("claim", key, cwd=worktree)[0]["claim"]
        first = a.bead("claims")["claims"][0]
        self.assertEqual(claimed["where"], {"branch": "feature/location", "commit": commit})
        self.assertEqual(first["branch"], "feature/location")
        a.bead("comments", "add", key, "activity after claim", cwd=worktree)
        second = a.bead("claims")["claims"][0]
        rows = a.bead("log", key)
        self.assertGreater(second["last_activity"], first["last_activity"])
        self.assertEqual([(row["op"], row["branch"]) for row in rows],
                         [("create", "main"), ("claim", "feature/location"),
                          ("comment", "feature/location")])


# ---- from test_review4.py: fourth independent review (2026-10-07-4-codex), folded in after the fixes
class CommandsAndCompatibilityReview4(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_q05_tombstone_update_cannot_leave_a_releasable_claim(self):
        """Q05: a terminal tombstone transition must not survive with a live claim."""
        a = support.Machine(self.box, "q05-tombstone")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "delete me")["id"]
        a.bead("sync")
        result = a.run("update", key, "--claim", "--status", "tombstone", "--json")
        if result.returncode:
            self.assertEqual(support.code(result), "VALIDATION")
            shown = a.show(key)
            self.assertEqual(shown["status"], "open")
            self.assertNotIn("claim", shown)
            self.assertEqual(list((a.store() / "pending").glob("*.json")), [])
            self.assertEqual(list((a.store() / "events").glob("*/claims/*.json")), [])
            peer = support.Machine(self.box, "q05-rejected-peer")
            peer.bead("sync")
            self.assertEqual(peer.show(key)["status"], "open")
            self.assertNotIn("claim", peer.show(key))
            return  # rejecting this spelling in favor of `delete` is safe
        shown = a.show(key)
        self.assertEqual(shown["status"], "tombstone")
        released = a.run("release", key, "--json")
        self.assertNotEqual(released.returncode, 0, "release resurrected a tombstoned issue")
        self.assertEqual(a.show(key)["status"], "tombstone")
        self.assertNotIn("claim", a.show(key))

    def test_q11_offline_close_ends_a_provisional_claim(self):
        """Q11: close clears seen provisional claims but preserves concurrent ones."""
        a = support.Machine(self.box, "q11-provisional")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "offline work")["id"]
        a.bead("sync")
        concurrent = support.Machine(self.box, "q11-concurrent")
        concurrent.bead("sync")

        # This claim has an earlier id but was made from a stale frontier, so
        # A's later close did not see it and must not erase it.
        concurrent.bead("claim", key, "--offline")
        a.bead("claim", key, "--offline")
        a.bead("close", key, "--offline")
        a.bead("sync")
        concurrent.bead("sync")
        a.bead("sync")
        shown = a.show(key)
        self.assertEqual(shown["status"], "closed")
        provisional = shown.get("provisional_claims", [])
        self.assertEqual(len(provisional), 1)
        self.assertIn("q11-concurrent-thread", provisional[0]["owner"])
        claims = a.bead("claims")["claims"]
        self.assertEqual(len(claims), 1)
        self.assertIn("q11-concurrent-thread", claims[0]["owner"])

    def test_q06_incremental_import_resolves_late_dependency_target(self):
        """Q06: split and one-shot imports must have the same dependencies/readiness."""
        dep = {
            "issue_id": "ex-a", "depends_on_id": "ex-b", "type": "blocks",
            "created_at": "2026-01-01T00:00:00Z", "created_by": "seed",
            "metadata": "{}", "thread_id": "",
        }
        common = {
            "status": "open", "priority": 2, "issue_type": "task",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed",
        }
        issue_a = {"id": "ex-a", "title": "A", **common, "dependencies": [dep]}
        issue_b = {"id": "ex-b", "title": "B", **common}
        a_path, b_path, all_path = (
            self.box.root / "q06-a.jsonl",
            self.box.root / "q06-b.jsonl",
            self.box.root / "q06-all.jsonl",
        )
        a_path.write_text(json.dumps(issue_a) + "\n")
        b_path.write_text(json.dumps(issue_b) + "\n")
        all_path.write_text(json.dumps(issue_a) + "\n" + json.dumps(issue_b) + "\n")

        split = support.Machine(self.box, "q06-split")
        split.bead("init", "--prefix", "ex")
        split.bead("import", str(a_path))
        split.bead("import", str(b_path))
        one = support.Machine(self.box, "q06-one")
        one.bead("init", "--prefix", "ex")
        one.bead("import", str(all_path))

        self.assertEqual(split.export(), one.export(), "interchange data should already agree")
        split_a, one_a = split.show("ex-a"), one.show("ex-a")
        self.assertEqual(split_a["dependencies"], one_a["dependencies"])
        self.assertEqual(split_a["blocked"], one_a["blocked"])
        self.assertEqual(split.ready(), one.ready())

    def test_q10_reserved_dangling_extra_cannot_crash_export(self):
        """Q10: an imported unknown field must not poison internal dependency bookkeeping."""
        a = support.Machine(self.box, "q10-import")
        a.bead("init", "--prefix", "ex")
        record = {
            "id": "ex-one", "title": "one", "status": "open", "priority": 2,
            "issue_type": "task", "_dangling_dependencies": "user supplied",
        }
        path = self.box.root / "q10.jsonl"
        path.write_text(json.dumps(record) + "\n")
        a.bead("import", str(path))
        exported = a.run("export")
        self.assertEqual(exported.returncode, 0, exported.stderr)
        self.assertEqual(json.loads(exported.stdout)["_dangling_dependencies"], "user supplied")


# ---- from test_review4.py: fourth independent review (2026-10-07-4-codex), folded in after the fixes
class DeepTreeReview4(unittest.TestCase):
    def test_q08_deep_valid_dependency_tree_does_not_overflow(self):
        """Q08: dep tree over a valid 1,100-node chain must not recurse into a crash."""
        with tempfile.TemporaryDirectory(prefix="bp-review4-tree-") as root:
            env = {
                **os.environ,
                "BEAD_HOME": str(Path(root) / "home"),
                "BEAD_WORKSPACE": "review4-deep-tree",
                "BEAD_AGENT": "test",
                "BEAD_THREAD": "tree-thread",
            }
            with mock.patch.dict(os.environ, env, clear=True):
                bp = support.load_module()
                workspace = bp.Workspace("review4-deep-tree", None, None, {})
                events, issue_ids, previous = [], [], None
                serial = 0

                def next_id():
                    nonlocal serial
                    serial += 1
                    value = ((1_790_000_000_000 + serial) << 80) | (7 << 76) | (2 << 62) | serial
                    return str(uuid.UUID(int=value))

                def emit(op, issue=None, data=None):
                    nonlocal previous
                    event_id = next_id()
                    event = {
                        "v": 1, "id": event_id, "replica": "tree",
                        "deps": [previous] if previous else [],
                        "actor": {"agent": "test", "thread": "tree-thread"},
                        "op": op, "issue": issue or event_id, "data": data or {},
                    }
                    events.append(event)
                    previous = event_id
                    return event

                emit("init", data={"prefix": "ex"})
                for index in range(1100):
                    issue_ids.append(emit(
                        "create", data={"key": f"ex-{index}", "fields": {"title": str(index)}}
                    )["id"])
                for index in range(1099):
                    emit("dep_add", issue_ids[index], {"to": issue_ids[index + 1], "type": "blocks"})
                bp.append_events(workspace, "tree", events)

            result = subprocess.run(
                [support.BEAD, "dep", "tree", "ex-0", "--json"],
                env=env,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len(json.loads(result.stdout)["tree"]), 1100)


# ---- from test_review5.py: fifth independent review (2026-10-07-5-codex), folded in after the fixes
COMMON = {
    "status": "open",
    "priority": 2,
    "issue_type": "task",
    "created_at": "2026-01-01T00:00:00Z",
    "updated_at": "2026-01-01T00:00:00Z",
    "created_by": "seed",
}


# ---- from test_review5.py: fifth independent review (2026-10-07-5-codex), folded in after the fixes
def strict_json(text: str):
    def reject_nonfinite(value: str):
        raise ValueError(f"non-finite JSON number: {value}")

    return json.loads(text, parse_constant=reject_nonfinite)


# ---- from test_review5.py: fifth independent review (2026-10-07-5-codex), folded in after the fixes
class Review5CLI(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def machine(self, name: str) -> support.Machine:
        machine = support.Machine(self.box, name)
        machine.bead("init", "--prefix", "ex")
        return machine

    def test_e01_human_output_escapes_terminal_controls(self):
        """E01: untrusted fields must not execute terminal protocols or forge lines."""
        machine = self.machine("e01-terminal")
        title = "safe\x1b]52;c;SGVsbG8=\x07\nFORGED: success"
        created = machine.run("create", title)
        self.assertEqual(created.returncode, 0, created.stderr)
        listing = machine.run("list")
        self.assertEqual(listing.returncode, 0, listing.stderr)
        for output in (created.stdout, listing.stdout):
            self.assertNotIn("\x1b", output)
            self.assertNotIn("\x07", output)
            self.assertNotIn("\nFORGED: success", output)

    def test_e02_remote_credentials_are_not_disclosed_or_world_readable(self):
        """E02: remote URL credentials must be redacted and protected at rest."""
        secret = "supersecret"
        remote = f"https://alice:{secret}@example.invalid/private.git"
        machine = support.Machine(self.box, "e02-secret")
        initialized = machine.run("init", "--prefix", "ex", "--remote", remote, "--json")
        self.assertEqual(initialized.returncode, 0, initialized.stderr)
        info = machine.run("info", "--json")
        self.assertEqual(info.returncode, 0, info.stderr)
        self.assertNotIn(secret, info.stdout)
        meta = machine.store() / "meta.json"
        self.assertNotIn(secret, meta.read_text())
        self.assertEqual(stat.S_IMODE(meta.stat().st_mode) & 0o077, 0)

    def test_e03_takeover_fences_an_owners_concurrent_offline_delete(self):
        """E03: a takeover must fence the prior owner's stale terminal transition."""
        owner = self.machine("e03-owner")
        key = owner.bead("create", "terminal race")["id"]
        owner.bead("sync")
        successor = support.Machine(self.box, "e03-successor")
        successor.bead("sync")
        owner.bead("claim", key)

        owner.bead("delete", key, "--offline")
        successor.bead("takeover", key, "--reason", "continue work")
        owner.bead("sync")
        successor.bead("sync")

        shown = successor.show(key)
        self.assertEqual(shown["status"], "in_progress")
        self.assertNotIn("status", shown.get("conflicts", {}))
        self.assertIn("e03-successor-thread", shown["claim"]["owner"])

    def test_e04_invalid_import_fails_instead_of_counting_an_inert_event(self):
        """E04: an import count and exit zero must mean the issue was imported."""
        machine = self.machine("e04-import")
        source = self.box.root / "invalid.jsonl"
        source.write_text(json.dumps({"id": "ex-invalid", "title": "discarded", **COMMON,
                                      "status": "NOT A STATUS"}) + "\n")  # a lower-case custom status imports
        imported = machine.run("import", str(source), "--json")
        self.assertNotEqual(imported.returncode, 0, imported.stdout)
        error = strict_json(imported.stderr)["error"]
        self.assertEqual(error["code"], "VALIDATION")
        self.assertEqual(machine.bead("info")["integrity"], [])

    def test_e05_silent_create_still_honors_json_mode(self):
        """E05: every successful --json command must emit valid JSON."""
        machine = self.machine("e05-silent")
        created = machine.run("create", "machine result", "--silent", "--json")
        self.assertEqual(created.returncode, 0, created.stderr)
        try:
            result = strict_json(created.stdout)
        except ValueError as error:
            self.fail(f"successful --json output was not JSON: {error}; bytes={created.stdout!r}")
        self.assertIsInstance(result, dict)
        self.assertIn("id", result)

    def test_e06_export_never_emits_nonfinite_json_numbers(self):
        """E06: accepted data must not make Beads JSONL violate RFC 8259."""
        machine = self.machine("e06-json")
        source = self.box.root / "nonfinite.jsonl"
        source.write_text(
            '{"id":"ex-nan","title":"bad number","status":"open","priority":2,'
            '"issue_type":"task","created_at":"2026-01-01T00:00:00Z",'
            '"updated_at":"2026-01-01T00:00:00Z","created_by":"seed","score":NaN}\n'
        )
        imported = machine.run("import", str(source), "--json")
        if imported.returncode:
            strict_json(imported.stderr)
            return
        exported = machine.run("export")
        self.assertEqual(exported.returncode, 0, exported.stderr)
        for line in exported.stdout.splitlines():
            try:
                strict_json(line)
            except ValueError as error:
                self.fail(f"export emitted non-RFC-8259 JSON: {error}; line={line!r}")

    def test_e07_closed_downstream_pipe_has_defined_quiet_exit(self):
        """E07: an early pipeline consumer exit must not produce Python flush noise."""
        machine = self.machine("e07-pipe")
        key = machine.bead("create", "pipe target")["id"]
        proc = subprocess.Popen(
            [support.BEAD, "show", key],
            cwd=machine.checkout,
            env=machine.env(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert proc.stdout is not None and proc.stderr is not None
        proc.stdout.close()
        stderr = proc.stderr.read().decode(errors="replace")
        proc.stderr.close()
        returncode = proc.wait()
        self.assertIn(returncode, (0, 141))
        self.assertNotIn("BrokenPipeError", stderr)
        self.assertNotIn("Traceback", stderr)

    def test_e08_nullable_import_values_do_not_crash_render_or_export(self):
        """E08: values accepted by import validation must be safe in every query path."""
        machine = self.machine("e08-nullable")
        comments = self.box.root / "nullable-comment.jsonl"
        comments.write_text(json.dumps({
            "id": "ex-null", "title": "nullable comment", **COMMON,
            "comments": [{"id": 1, "author": None, "text": "hello", "created_at": None}],
        }) + "\n")
        self.assertEqual(machine.run("import", str(comments), "--json").returncode, 0)
        for args in (("show", "ex-null"), ("comments", "ex-null")):
            with self.subTest(args=args):
                result = machine.run(*args)
                self.assertEqual(result.returncode, 0, result.stderr)

        dependencies = self.box.root / "nullable-dependencies.jsonl"
        record = {"id": "ex-deps", "title": "nullable dependency", **COMMON,
                  "dependencies": [
                      {"issue_id": "ex-deps", "depends_on_id": "missing", "type": None},
                      {"issue_id": "ex-deps", "depends_on_id": "missing", "type": "blocks"},
                  ]}
        dependencies.write_text(json.dumps(record) + "\n")
        self.assertEqual(machine.run("import", str(dependencies), "--json").returncode, 0)
        exported = machine.run("export")
        self.assertEqual(exported.returncode, 0, exported.stderr)

    def test_e09_invalid_config_types_return_structured_config_errors(self):
        """E09: malformed tracked config must not escape as Python tracebacks."""
        machine = support.Machine(self.box, "e09-config")
        config = machine.checkout / ".beadroll.json"
        cases = (
            "[]\n",
            '{"workspace":"safe","ref":42}\n',
            '{"workspace":"safe","remote":{"url":"x"}}\n',
        )
        for body in cases:
            with self.subTest(config=body):
                config.write_text(body)
                result = machine.run("info", "--json")
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("Traceback", result.stderr)
                self.assertEqual(strict_json(result.stderr)["error"]["code"], "CONFIG")

    def test_e11_parent_filter_matches_every_direct_parent(self):
        """E11: --parent must match any parent represented in the issue's dependency set."""
        machine = self.machine("e11-parents")
        first = machine.bead("create", "parent one")["id"]
        second = machine.bead("create", "parent two")["id"]
        child = machine.bead("create", "child")["id"]
        machine.bead("dep", "add", child, first, "--type", "parent-child")
        machine.bead("dep", "add", child, second, "--type", "parent-child")
        for parent in (first, second):
            with self.subTest(parent=parent):
                result = machine.bead("list", "--all", "--parent", parent, "--limit", "0")
                self.assertIn(child, {issue["id"] for issue in result["issues"]})

    def test_e12_ascii_locale_cannot_turn_a_committed_create_into_failure(self):
        """E12: rendering in a reduced locale must not report failure after committing."""
        machine = self.machine("e12-ascii")
        result = machine.run(
            "create", "committed but failed",
            LC_ALL="C", LANG="C", PYTHONUTF8="0", PYTHONCOERCECLOCALE="0",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        listed = machine.bead("list", "--all", "--limit", "0")
        self.assertEqual([issue["title"] for issue in listed["issues"]], ["committed but failed"])

    def test_e13_receipt_identity_includes_resolved_file_content(self):
        """E13: changed file input is a new semantic request after lost-ack recovery."""
        machine = self.machine("e13-receipt-file")
        key = machine.bead("create", "receipt target")["id"]
        machine.bead("sync")
        body = self.box.root / "description.txt"
        body.write_text("first body")
        first = machine.run(
            "update", key, "--claim", "--description-file", str(body), "--json",
            BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        self.assertEqual(first.returncode, 99)
        body.write_text("second body")
        retry = machine.run("update", key, "--claim", "--description-file", str(body), "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(machine.show(key)["description"], "second body")

    def test_v01_machine_json_preserves_and_escapes_control_data(self):
        """V01: the standard JSON serializer keeps control data inert and recoverable."""
        machine = self.machine("v01-json-control")
        title = "safe\x1b]52;c;SGVsbG8=\x07\nline"
        result = machine.run("create", title, "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("\x1b", result.stdout)
        self.assertEqual(strict_json(result.stdout)["title"], title)


# ---- from test_review5.py: fifth independent review (2026-10-07-5-codex), folded in after the fixes
class Review5PositiveControls(unittest.TestCase):
    def test_v02_help_needs_no_repository_and_creates_no_state(self):
        """V02: top-level and subcommand help are local, side-effect free operations."""
        with tempfile.TemporaryDirectory(prefix="bead-review5-help-") as tmp:
            root = Path(tmp)
            state = root / "state"
            env = {**os.environ, "BEAD_HOME": str(state)}
            for args in (("--help",), ("create", "--help")):
                result = subprocess.run([support.BEAD, *args], cwd=root, env=env,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("usage:", result.stdout)
                self.assertFalse(state.exists())


# ---- maintainer: help, prime and onboard (roll-is0e, roll-dih8.1)
class HelpAndPrime(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "help-a")

    def test_no_arguments_and_help_print_the_overview_and_exit_zero(self):
        for argv in ((), ("--help",), ("-h",), ("help",)):
            with self.subTest(argv=argv):
                result = self.a.run(*argv)
                self.assertEqual(result.returncode, 0, result.stderr)
                for section in ("Find work", "Do work", "Coordinate", "Agents", "--json"):
                    self.assertIn(section, result.stdout)
                self.assertNotIn("Traceback", result.stderr)
        self.assertIn("help", json.loads(self.a.run("--json").stdout))

    def test_help_command_shows_one_commands_usage(self):
        result = self.a.run("help", "claim")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("usage: bead claim", result.stdout)
        self.assertIn("--offline", result.stdout)
        self.assertNotIn("\\x0a", result.stdout)  # multi-line texts are lines, not one escaped line
        self.assertIn("\n1. Find work", self.a.run("prime").stdout)
        self.assertEqual(support.code(self.a.run("help", "nonsense", "--json")), "USAGE")

    def test_every_command_has_help(self):
        bead = support.load_module()
        choices = next(a for a in bead.build_parser()._actions if isinstance(a, bead.argparse._SubParsersAction)).choices
        self.assertGreater(len(choices), 20)
        for name in choices:
            with self.subTest(command=name):
                result = self.a.run(name, "--help")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f"usage: bead {name}", result.stdout)

    def test_prime_and_onboard_work_with_and_without_a_tracker(self):
        outside = subprocess.run([support.BEAD, "prime"], cwd=self.box.root, env=self.a.env(), capture_output=True, text=True)
        self.assertEqual(outside.returncode, 0, outside.stderr)
        self.assertIn("Not inside a git repository", outside.stdout)
        before = self.a.run("prime")
        self.assertEqual(before.returncode, 0, before.stderr)
        self.assertIn("No tracker here yet", before.stdout)
        self.a.bead("init", "--prefix", "ex")
        key = self.a.bead("create", "primed")["id"]
        self.a.bead("claim", key, "--offline")
        primed = self.a.bead("prime")
        self.assertIn("bead claim ID", primed["prime"])
        self.assertIn("ALREADY_CLAIMED", primed["prime"])
        self.assertIn("Tracker 'ex'", primed["prime"])
        self.assertIn(primed["actor"], primed["prime"])
        self.assertIn("help-a-thread-0001", primed["actor"])
        onboard = self.a.run("onboard")
        self.assertEqual(onboard.returncode, 0, onboard.stderr)
        self.assertIn("bead prime", onboard.stdout)
        self.assertIn("bead ready", onboard.stdout)
        agents = (Path(support.BEAD).resolve().parent / "AGENTS.md").read_text()  # this repository dogfoods it
        self.assertIn(onboard.stdout.strip(), agents, "AGENTS.md's work-tracking block must be bead onboard's output")

# ---- from test_review6.py: sixth independent review (2026-10-07-6-grok), folded in after the fixes
def ops(machine, key):
    return [event["op"] for event in machine.bead("log", key, "--limit", "0")]


class Review6(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def machine(self, name):
        machine = support.Machine(self.box, name)
        machine.bead("init", "--prefix", "ex")
        return machine

    def test_f01_failed_command_does_not_forget_an_accepted_retry(self):
        """F01: a failed command must not make a retry of an accepted command look new."""
        owner = self.machine("f01")
        key = owner.bead("create", "close once")["id"]
        owner.bead("sync")
        crashed = owner.run(
            "update", key, "--claim", "--status", "closed", "--json",
            BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        self.assertEqual(crashed.returncode, 99)
        missed = owner.run("claim", "missing-id", "--json")
        self.assertNotEqual(missed.returncode, 0)
        self.assertEqual(len(list((owner.store() / "pending").glob("*.command"))), 1)
        retry = owner.run("update", key, "--claim", "--status", "closed", "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(ops(owner, key).count("close"), 1)

        parent = self.machine("f01-parent")
        first = parent.bead("create", "first")["id"]
        second = parent.bead("create", "second")["id"]
        child = parent.bead("create", "child", "--parent", first)["id"]
        parent.bead("sync")
        crashed = parent.run(
            "update", child, "--claim", "--parent", second, "--json",
            BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        self.assertEqual(crashed.returncode, 99)
        parent.run("claim", "missing-id", "--json")
        retry = parent.run("update", child, "--claim", "--parent", second, "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(parent.show(child).get("parent"), second)
        self.assertEqual(ops(parent, child).count("dep_remove"), 1)

    def test_f02_locale_retry_keeps_the_accepted_description(self):
        """F02: a retry decodes the same file as the same request, whatever the locale."""
        body = self.box.root / "cafe.txt"
        body.write_bytes("café body\n".encode())
        latin = self.machine("f02-latin")
        key = latin.bead("create", "locale")["id"]
        latin.bead("sync")
        crashed = latin.run(
            "update", key, "--claim", "--description-file", str(body), "--json",
            BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        self.assertEqual(crashed.returncode, 99)
        retry = latin.run(
            "update", key, "--claim", "--description-file", str(body), "--json",
            LC_ALL="en_US.ISO8859-1", LANG="en_US.ISO8859-1",
            PYTHONUTF8="0", PYTHONCOERCECLOCALE="0",
        )
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(latin.show(key).get("description"), "café body\n")
        self.assertEqual(ops(latin, key).count("set"), 1)

        sea = self.machine("f02-c")
        key = sea.bead("create", "locale c")["id"]
        sea.bead("sync")
        crashed = sea.run(
            "update", key, "--claim", "--description-file", str(body), "--json",
            BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        self.assertEqual(crashed.returncode, 99)
        retry = sea.run(
            "update", key, "--claim", "--description-file", str(body), "--json",
            LC_ALL="C", LANG="C", PYTHONUTF8="0", PYTHONCOERCECLOCALE="0",
        )
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertNotIn("Traceback", retry.stderr)
        self.assertEqual(sea.show(key).get("description"), "café body\n")

    def test_f03_credential_urls_are_not_stored(self):
        """F03: uppercase schemes, slash passwords and token userinfo are not stored or shown."""
        cases = {
            "uppercase": "HTTPS://alice:supersecret@example.invalid/private.git",
            "slash": "https://alice:sec/ret@example.invalid/private.git",
            "token": "https://supersecret@example.invalid/private.git",
        }
        for name, url in cases.items():
            with self.subTest(shape=name):
                machine = support.Machine(self.box, name)
                made = machine.run(
                    "init", "--prefix", "ex", "--tracked", "--remote", url, "--json",
                )
                self.assertEqual(made.returncode, 0, made.stderr)
                config = (machine.checkout / ".beadroll.json").read_text()
                info = machine.run("info", "--json")
                metas = "\n".join(path.read_text() for path in machine.home.glob("*/meta.json"))
                blob = config + info.stdout + info.stderr + metas
                self.assertNotIn("supersecret", blob)
                self.assertNotIn("sec/ret", blob)
                self.assertNotIn("s%65cret", blob)

    def test_f04_checkout_files_stay_readable_in_a_shared_tree(self):
        """F04: .beadroll.json and export -o are not forced owner-only."""
        machine = support.Machine(self.box, "f04")
        made = machine.run("init", "--prefix", "ex", "--tracked", "--json")
        self.assertEqual(made.returncode, 0, made.stderr)
        config = machine.checkout / ".beadroll.json"
        target = self.box.root / "shared" / "issues.jsonl"
        target.parent.mkdir()
        exported = machine.run("export", "-o", str(target))
        self.assertEqual(exported.returncode, 0, exported.stderr)
        for path in (config, target):
            mode = stat.S_IMODE(path.stat().st_mode)
            self.assertTrue(mode & 0o040, f"{path.name} is {oct(mode)}")

    def test_f05_human_output_escapes_tab_and_line_separators(self):
        """F05: human output escapes TAB and Unicode line separators."""
        machine = self.machine("f05")
        created = machine.run("create", "col\tFORGED\u2028line\u202etitle")
        self.assertEqual(created.returncode, 0, created.stderr)
        self.assertNotIn("\t", created.stdout)
        self.assertNotIn("\u2028", created.stdout)
        self.assertNotIn("\u202e", created.stdout)

    def test_f06_failed_command_with_closed_stderr_keeps_its_status(self):
        """F06: a closed stderr on a failed command exits with the command status, not 120."""
        machine = self.machine("f06")
        read, write = os.pipe()
        os.close(read)
        proc = subprocess.Popen(
            [BEAD, "create"], cwd=machine.checkout, env=machine.env(),
            stdout=subprocess.PIPE, stderr=write,
        )
        os.close(write)
        proc.communicate()
        self.assertEqual(proc.returncode, 1)

    def test_f07_non_utf8_input_is_a_structured_error(self):
        """F07: non-UTF-8 titles and description files return a JSON error and store nothing."""
        machine = self.machine("f07")
        created = subprocess.run(
            [BEAD, "create", "bad\udcfftitle", "--json"],
            cwd=machine.checkout, env=machine.env(), capture_output=True, text=True,
        )
        self.assertNotEqual(created.returncode, 0)
        self.assertNotIn("Traceback", created.stderr)
        self.assertEqual(strict_json(created.stderr)["error"]["code"], "VALIDATION")
        titles = [issue["title"] for issue in machine.bead("list", "--all", "--limit", "0")["issues"]]
        self.assertFalse(any("\udcff" in title for title in titles))

        raw = self.box.root / "raw.txt"
        raw.write_bytes(b"caf\xff")
        key = machine.bead("create", "file target")["id"]
        updated = machine.run("update", key, "--description-file", str(raw), "--json")
        self.assertNotEqual(updated.returncode, 0)
        self.assertNotIn("Traceback", updated.stderr)
        self.assertEqual(strict_json(updated.stderr)["error"]["code"], "VALIDATION")
        self.assertNotIn("description", machine.show(key))

    def test_f08_import_probes_dependencies_before_writing(self):
        """F08: a bad dependency fails the import; it is not dropped and it is not a traceback."""
        machine = self.machine("f08")
        typed = self.box.root / "typed.jsonl"
        typed.write_text(
            json.dumps({"id": "ex-a", "title": "A", **COMMON, "dependencies": [
                {"issue_id": "ex-a", "depends_on_id": "ex-b", "type": 1}]}) + "\n"
            + json.dumps({"id": "ex-b", "title": "B", **COMMON}) + "\n"
        )
        imported = machine.run("import", str(typed), "--json")
        self.assertNotEqual(imported.returncode, 0, imported.stdout)
        self.assertNotIn("Traceback", imported.stderr)
        self.assertEqual(strict_json(imported.stderr)["error"]["code"], "VALIDATION")
        self.assertEqual(machine.bead("info")["integrity"], [])

        bare = self.box.root / "bare.jsonl"
        bare.write_text(json.dumps({"id": "ex-c", "title": "C", **COMMON, "dependencies": ["ex-b"]}) + "\n")
        imported = machine.run("import", str(bare), "--json")
        self.assertNotEqual(imported.returncode, 0)
        self.assertNotIn("Traceback", imported.stderr)
        self.assertEqual(machine.bead("list", "--all", "--id", "ex-c", "--limit", "0")["total"], 0)

    def test_f09_dangling_dependency_can_be_retracted_before_its_target_exists(self):
        """F09: a dependency removed before its target exists does not reappear, and does not cycle."""
        root = self.box.root
        common = dict(COMMON)
        dangling = {"id": "ex-a", "title": "A", **common, "dependencies": [
            {"issue_id": "ex-a", "depends_on_id": "ex-b", "type": "blocks"}]}
        cleared = {"id": "ex-a", "title": "A", **common}
        target = {"id": "ex-b", "title": "B", **common, "dependencies": [
            {"issue_id": "ex-b", "depends_on_id": "ex-a", "type": "blocks"}]}
        for name, records in (("a", [dangling]), ("cleared", [cleared]), ("b", [target])):
            (root / f"{name}.jsonl").write_text("".join(json.dumps(rec) + "\n" for rec in records))

        with self.subTest("dep remove"):
            retracted = self.machine("f09")
            retracted.bead("import", str(root / "a.jsonl"))
            removed = retracted.run("dep", "remove", "ex-a", "ex-b", "--json")
            self.assertEqual(removed.returncode, 0, removed.stderr)
            retracted.bead("import", str(root / "b.jsonl"))
            self.assertEqual(retracted.show("ex-a").get("dependencies"), [])
            self.assertIn("ex-a", retracted.ready())
            self.assertNotIn("ex-b", retracted.ready())

        with self.subTest("reimport"):
            reimported = self.machine("f09-reimport")
            reimported.bead("import", str(root / "a.jsonl"))
            reimported.bead("import", str(root / "cleared.jsonl"))
            reimported.bead("import", str(root / "b.jsonl"))
            self.assertEqual(
                [(d["id"], d["dependency_type"]) for d in reimported.show("ex-a").get("dependencies", [])],
                [],
            )
            self.assertIn("ex-a", reimported.ready())
            self.assertEqual(reimported.bead("dep", "cycles")["count"], 0)

        bead = support.load_module()
        with self.subTest("replay"):
            create_a, remove, create_b = (
                "01900000-0000-7000-8000-000000000001",
                "01900000-0000-7000-8000-000000000002",
                "01900000-0000-7000-8000-000000000003",
            )
            journal = bead.Journal()
            for event in (
                {"v": 1, "id": create_a, "replica": "m", "deps": [], "op": "create", "issue": create_a,
                 "data": {"key": "ex-a", "fields": {"title": "A", "status": "open", "priority": 2, "issue_type": "task"},
                          "dangling": [{"depends_on_id": "ex-b", "type": "blocks"}]}},
                {"v": 1, "id": remove, "replica": "m", "deps": [create_a], "op": "dep_remove", "issue": create_a,
                 "data": {"to": create_b, "type": "blocks"}},
                {"v": 1, "id": create_b, "replica": "m", "deps": [remove], "op": "create", "issue": create_b,
                 "data": {"key": "ex-b", "fields": {"title": "B", "status": "open", "priority": 2, "issue_type": "task"}}},
            ):
                journal.add(event, "m")
            state = bead.replay(journal)
            self.assertEqual(sorted(state.issues[create_a]["deps"]), [])
            self.assertFalse(bead.is_blocked(state, state.issues[create_a]))

    def test_f10_non_event_json_makes_the_whole_attempt_inert(self):
        """F10: a later JSON value that is not an event voids the whole attempt."""
        writer = self.machine("f10")
        key = writer.bead("create", "attempt")["id"]
        writer.bead("sync")
        reader = support.Machine(self.box, "f10-reader")
        reader.bead("sync")
        writer.bead("update", key, "--claim", "--title", "kept", "--add-label", "kept")
        reader.bead("sync")
        attempt = next(path for path in (reader.store() / "events").glob("*/claims/*.json")
                       if len(path.read_bytes().splitlines()) >= 2)
        attempt.write_bytes(attempt.read_bytes().splitlines()[0] + b"\nnull\n")
        shown = reader.show(key)
        self.assertNotIn("claim", shown)
        self.assertEqual(shown["title"], "attempt")
        self.assertTrue(reader.bead("info")["integrity"])

    def test_f11_stale_pending_twin_does_not_fork_a_matching_attempt(self):
        """F11: a leftover pending twin must not raise ATTEMPT_FORK when the confirmed file matches."""
        writer = self.machine("f11")
        key = writer.bead("create", "twin")["id"]
        writer.bead("sync")
        reader = support.Machine(self.box, "f11-reader")
        reader.bead("sync")
        writer.bead("claim", key)
        reader.bead("sync")
        attempt = next((reader.store() / "events").glob("*/claims/*.json"))
        pending = reader.store() / "pending"
        pending.mkdir(exist_ok=True)
        (pending / f"{attempt.stem}.json").write_bytes(attempt.read_bytes() + b"\n")
        synced = reader.run("sync", "--json")
        self.assertEqual(synced.returncode, 0, synced.stderr)
        self.assertNotIn("ATTEMPT_FORK", synced.stderr)
        self.assertIn("claim", reader.show(key))

    def test_f12_unreadable_pending_file_is_not_fenced_forever(self):
        """F12: one corrupt pending file does not add a fence commit on every later sync."""
        machine = self.machine("f12")
        machine.bead("create", "quiet")
        machine.bead("sync")
        pending = machine.store() / "pending"
        pending.mkdir(exist_ok=True)
        junk = pending / "01900000-0000-7000-8000-0000000000aa.json"
        junk.write_text("{")
        before = int(support.git("--git-dir", str(self.box.origin), "rev-list", "--count", "refs/beads/journal"))
        first = machine.run("sync", "--json")
        second = machine.run("sync", "--json")
        after = int(support.git("--git-dir", str(self.box.origin), "rev-list", "--count", "refs/beads/journal"))
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertFalse(junk.exists())
        self.assertLessEqual(after, before + 1)

    def test_f13_local_state_files_are_strict_json(self):
        """F13: meta, replica and aliases reject non-strict or wrong-shaped JSON without a traceback."""
        machine = self.machine("f13")
        (machine.store() / "meta.json").write_text('{"last_sync": NaN, "name": "ex"}\n')
        info = machine.run("info", "--json")
        self.assertEqual(info.returncode, 0, info.stderr)
        self.assertNotIn("Traceback", info.stderr)
        try:
            strict_json(info.stdout)
        except ValueError as error:
            self.fail(f"info --json was not strict JSON: {error}; {info.stdout[:240]}")

        replica = machine.home / "replica.json"
        saved = replica.read_text()
        replica.write_text('{"id": NaN}\n')
        info = machine.run("info", "--json")
        self.assertNotIn("Traceback", info.stderr)
        if info.returncode == 0:
            try:
                strict_json(info.stdout)
            except ValueError as error:
                self.fail(f"info --json was not strict JSON: {error}; {info.stdout[:240]}")
        else:
            self.assertEqual(strict_json(info.stderr)["error"]["code"], "IDENTITY")
        created = machine.run("create", "after nan identity", "--json")
        self.assertNotIn("Traceback", created.stderr)
        replica.write_text(saved)

        (machine.home / "aliases.json").write_text("[1, 2]\n")
        listed = machine.run("machines", "--json")
        self.assertNotIn("Traceback", listed.stderr)
        self.assertEqual(listed.returncode, 0, listed.stderr)

    def test_f14_future_clock_create_is_a_structured_error(self):
        """F14: a clock past year 2100 does not turn create into a KeyError traceback."""
        machine = self.machine("f14")
        wrapper = self.box.root / "ahead.py"
        wrapper.write_text(textwrap.dedent(f"""
            import time
            time.time_ns = lambda: 4_429_000_000_000_000_000
            import runpy
            runpy.run_path({str(BEAD)!r}, run_name="__main__")
        """))
        created = subprocess.run(
            [sys.executable, str(wrapper), "create", "future", "--json"],
            cwd=machine.checkout, env=machine.env(), capture_output=True, text=True,
        )
        self.assertNotIn("Traceback", created.stderr)
        self.assertNotEqual(created.returncode, 0)
        titles = [issue["title"] for issue in machine.bead("list", "--all", "--limit", "0")["issues"]]
        self.assertNotIn("future", titles)

    def test_f15_timeout_reaps_the_git_helper(self):
        """F15: a fetch that times out does not leave git-upload-pack running."""
        exec_path = Path(subprocess.check_output(["git", "--exec-path"], text=True).strip())
        fake = self.box.root / "git-exec"
        fake.mkdir()
        for entry in exec_path.iterdir():
            if entry.name not in ("git-upload-pack", "git-receive-pack"):
                os.symlink(entry, fake / entry.name)
        log = self.box.root / "helper-pids"
        log.write_text("")
        for name in ("git-upload-pack", "git-receive-pack"):
            script = fake / name
            script.write_text(textwrap.dedent(f"""\
                #!/bin/sh
                echo $$ >> {log}
                sleep 30
                exec {exec_path / name} "$@"
            """))
            script.chmod(0o755)
        machine = self.machine("f15")
        machine.bead("create", "one")
        proc = subprocess.run(
            [BEAD, "sync", "--json"], cwd=machine.checkout,
            env=machine.env(BEAD_NET_TIMEOUT="2", GIT_EXEC_PATH=str(fake)),
            capture_output=True, text=True,
        )
        self.assertNotEqual(proc.returncode, 0)
        self.assertNotIn("Traceback", proc.stderr)
        time.sleep(0.2)
        alive = []
        try:
            for line in log.read_text().split():
                pid = int(line)
                try:
                    os.kill(pid, 0)
                except OSError:
                    continue
                alive.append(pid)
            self.assertEqual(alive, [])
        finally:
            for pid in alive:
                try:
                    os.kill(pid, 9)
                except OSError:
                    pass

    def test_f16_read_only_store_write_is_a_structured_error(self):
        """F16: writing a read-only store fails without a traceback and keeps what was already there."""
        machine = self.machine("f16")
        key = machine.bead("create", "kept")["id"]
        store = machine.store()
        subprocess.run(["chmod", "-R", "a-w", str(store)], check=True)
        try:
            created = machine.run("create", "nope", "--json")
            self.assertNotEqual(created.returncode, 0)
            self.assertNotIn("Traceback", created.stderr)
            shown = machine.run("show", key, "--json")
            self.assertEqual(shown.returncode, 0, shown.stderr)
            self.assertEqual(strict_json(shown.stdout)[0]["title"], "kept")
        finally:
            subprocess.run(["chmod", "-R", "u+w", str(store)], check=True)

    def test_receipt_identity_holds_for_actor_sync_stdin_and_json(self):
        """Verified (no finding): another thread, a sync, changed stdin and --json do not confuse a receipt."""
        machine = self.machine("receipt-ok")
        key = machine.bead("create", "receipt")["id"]
        machine.bead("sync")
        body = self.box.root / "body.txt"
        body.write_text("first body")
        crashed = machine.run(
            "update", key, "--claim", "--description-file", str(body), "--json",
            BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        self.assertEqual(crashed.returncode, 99)
        other = machine.run("claim", "missing-id", "--json", thread="other-thread")
        self.assertNotEqual(other.returncode, 0)
        self.assertEqual(len(list((machine.store() / "pending").glob("*.command"))), 1)
        synced = machine.run("sync", "--json")
        self.assertEqual(synced.returncode, 0, synced.stderr)
        self.assertEqual(len(list((machine.store() / "pending").glob("*.command"))), 1)
        body.write_text("second body")
        retry = machine.run("update", key, "--claim", "--description-file", str(body), "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(machine.show(key)["description"], "second body")

        piped = self.machine("receipt-stdin")
        key = piped.bead("create", "stdin")["id"]
        piped.bead("sync")
        crashed = subprocess.run(
            [BEAD, "update", key, "--claim", "--description-file", "-", "--json"],
            cwd=piped.checkout, env=piped.env(BEAD_TEST_CRASH_AFTER_PUSH="1"),
            input="stdin one", capture_output=True, text=True,
        )
        self.assertEqual(crashed.returncode, 99)
        retry = subprocess.run(
            [BEAD, "update", key, "--claim", "--description-file", "-", "--json"],
            cwd=piped.checkout, env=piped.env(),
            input="stdin two", capture_output=True, text=True,
        )
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(piped.show(key)["description"], "stdin two")

        flagged = self.machine("receipt-json")
        key = flagged.bead("create", "json flag")["id"]
        flagged.bead("sync")
        crashed = flagged.run(
            "update", key, "--claim", "--title", "once", BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        retry = flagged.run("update", key, "--claim", "--title", "once", "--json")
        self.assertEqual(crashed.returncode, 99)
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(ops(flagged, key).count("set"), 1)

    def test_remote_url_shapes_that_are_already_safe(self):
        """Verified (no finding): encoded passwords, ssh:// and scp-like URLs, and a named remote."""
        encoded = support.Machine(self.box, "encoded")
        made = encoded.run(
            "init", "--prefix", "ex", "--remote",
            "https://alice:s%65cret@example.invalid/private.git", "--json",
        )
        self.assertEqual(made.returncode, 0, made.stderr)
        meta = next(encoded.home.glob("*/meta.json")).read_text()
        self.assertNotIn("s%65cret", meta)
        self.assertIn('"https://example.invalid/private.git"', meta)  # G06: over https the user part may be the token

        ssh = support.Machine(self.box, "ssh")
        ssh.run("init", "--prefix", "ex", "--remote", "ssh://alice:supersecret@example.invalid/private.git")
        self.assertNotIn("supersecret", next(ssh.home.glob("*/meta.json")).read_text())
        plain = support.Machine(self.box, "ssh-user")
        plain.run("init", "--prefix", "ex", "--remote", "ssh://alice@example.invalid/private.git")
        self.assertIn("ssh://alice@example.invalid/private.git", next(plain.home.glob("*/meta.json")).read_text())
        scp = support.Machine(self.box, "scp")
        scp.run("init", "--prefix", "ex", "--remote", "git@example.invalid:org/private.git")
        self.assertIn("git@example.invalid:org/private.git", next(scp.home.glob("*/meta.json")).read_text())

        named = support.Machine(self.box, "named")
        named.bead("init", "--prefix", "ex")
        support.git(
            "remote", "set-url", "origin",
            "https://alice:supersecret@example.invalid/private.git",
            cwd=named.checkout,
        )
        (named.checkout / ".beadroll.json").write_text(
            json.dumps({"workspace": "named-ws", "remote": "origin"}) + "\n"
        )
        info = named.run("info", "--json")
        self.assertEqual(info.returncode, 0, info.stderr)
        self.assertNotIn("supersecret", info.stdout)
        self.assertEqual(
            strict_json(info.stdout)["remote"],
            "https://example.invalid/private.git",  # G06: over https the user part may be the token
        )

    def test_prime_matches_ready_and_help_outside_a_repository_is_silent(self):
        """Verified (no finding): prime's ready count matches bead ready, and help creates no state."""
        machine = self.machine("prime")
        open_id = machine.bead("create", "open")["id"]
        machine.bead("create", "blocked", "--deps", f"blocks:{open_id}")
        claimed = machine.bead("create", "claimed")["id"]
        machine.bead("claim", claimed, "--offline")
        deferred = machine.bead("create", "deferred")["id"]
        machine.bead("update", deferred, "--status", "deferred")
        closed = machine.bead("create", "closed")["id"]
        machine.bead("close", closed, "--reason", "done")
        epic = machine.bead("create", "epic", "--type", "epic")["id"]
        machine.bead("create", "child", "--parent", epic)
        primed = machine.bead("prime")["prime"]
        ready = machine.bead("ready")
        self.assertIn(f"{len(ready)} ready", primed)

        outside = self.box.root / "outside"
        outside.mkdir()
        state = self.box.root / "outside-state"
        env = {k: v for k, v in os.environ.items() if not k.startswith(("BEAD_", "GIT_", "CLAUDE", "CODEX"))}
        env["BEAD_HOME"] = str(state)
        for args in (["help"], ["help", "claim"], ["prime"], ["onboard"], ["--help"]):
            with self.subTest(args=args):
                result = subprocess.run([BEAD, *args], cwd=outside, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("Traceback", result.stderr)
                self.assertFalse(state.exists())

    def test_incremental_mutual_import_matches_one_shot(self):
        """Verified (no finding): a mutual dependency imported in two batches matches one import."""
        root = self.box.root
        first = {"id": "ex-a", "title": "A", **COMMON, "dependencies": [
            {"issue_id": "ex-a", "depends_on_id": "ex-b", "type": "blocks"}]}
        second = {"id": "ex-b", "title": "B", **COMMON, "dependencies": [
            {"issue_id": "ex-b", "depends_on_id": "ex-a", "type": "blocks"}]}
        (root / "a.jsonl").write_text(json.dumps(first) + "\n")
        (root / "b.jsonl").write_text(json.dumps(second) + "\n")
        (root / "both.jsonl").write_text(json.dumps(first) + "\n" + json.dumps(second) + "\n")
        split = self.machine("mutual-split")
        split.bead("import", str(root / "a.jsonl"))
        split.bead("import", str(root / "b.jsonl"))
        one = self.machine("mutual-one")
        one.bead("import", str(root / "both.jsonl"))

        def edges(machine, key):
            return sorted((item["id"], item["dependency_type"]) for item in machine.show(key)["dependencies"])

        self.assertEqual(edges(split, "ex-a"), edges(one, "ex-a"))
        self.assertEqual(edges(split, "ex-b"), edges(one, "ex-b"))
        self.assertEqual(split.ready(), one.ready())
        self.assertEqual(split.ready(), set())

    def test_clock_skew_does_not_override_causality(self):
        """Verified (no finding): a write that has seen another still settles when its clock is behind."""
        wrapper = self.box.root / "behind.py"
        wrapper.write_text(textwrap.dedent(f"""
            import time
            _real = time.time_ns
            time.time_ns = lambda: _real() - {48 * 3600 * 1_000_000_000}
            import runpy
            runpy.run_path({str(BEAD)!r}, run_name="__main__")
        """))
        past = self.machine("clock-past")
        key = past.bead("create", "base")["id"]
        past.bead("sync")
        future = support.Machine(self.box, "clock-future")
        future.bead("sync")
        past.bead("update", key, "--title", "from-a")
        past.bead("sync")
        future.bead("sync")
        updated = subprocess.run(
            [sys.executable, str(wrapper), "update", key, "--title", "from-b-behind", "--json"],
            cwd=future.checkout, env=future.env(), capture_output=True, text=True,
        )
        self.assertEqual(updated.returncode, 0, updated.stderr)
        future.bead("sync")
        past.bead("sync")
        shown = past.show(key)
        self.assertEqual(shown["title"], "from-b-behind")
        self.assertNotIn("conflicts", shown)

        key2 = past.bead("create", "race")["id"]
        past.bead("sync")
        future.bead("sync")
        past.bead("update", key2, "--title", "alpha")
        other = subprocess.run(
            [sys.executable, str(wrapper), "update", key2, "--title", "beta", "--json"],
            cwd=future.checkout, env=future.env(), capture_output=True, text=True,
        )
        self.assertEqual(other.returncode, 0, other.stderr)
        past.bead("sync")
        future.bead("sync")
        past.bead("sync")
        raced = past.show(key2)
        self.assertIn("title", raced.get("conflicts", {}))
        self.assertEqual(
            {item["value"] for item in raced["conflicts"]["title"]},
            {"alpha", "beta"},
        )

    def test_closed_stdout_on_success_exits_zero_and_store_stays_private(self):
        """Verified (no finding): a closed stdout after a successful read exits 0; store files stay private."""
        machine = self.machine("pipe-ok")
        key = machine.bead("create", "pipe")["id"]
        read, write = os.pipe()
        os.close(read)
        proc = subprocess.Popen(
            [BEAD, "show", key], cwd=machine.checkout, env=machine.env(),
            stdout=write, stderr=subprocess.PIPE,
        )
        os.close(write)
        _, err = proc.communicate()
        self.assertEqual(proc.returncode, 0)
        self.assertNotIn(b"Traceback", err)
        meta = next(machine.home.glob("*/meta.json"))
        self.assertEqual(stat.S_IMODE(meta.stat().st_mode) & 0o077, 0)

    def test_json_keeps_bidi_text(self):
        """Verified (no finding): JSON keeps bidi characters in the stored title and escapes them as JSON."""
        machine = self.machine("bidi")
        title = "bidi-\u202eJSON"
        created = machine.run("create", title, "--json")
        self.assertEqual(created.returncode, 0, created.stderr)
        self.assertEqual(strict_json(created.stdout)["title"], title)


if __name__ == "__main__":
    unittest.main()


# ---- format policy (2026-10-08): format 1 is frozen, changes are additive
class FormatPolicy(unittest.TestCase):
    def test_unapplicable_events_travel_untouched_and_hold_nothing_up(self):
        """An event this bead cannot apply (an unknown operation, a newer format: what a newer bead would write)
        is set aside and reported on every machine, carried through sync byte for byte, and holds up nothing
        else; so machines upgrade in any order and a mixed fleet is safe."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a, b = support.Machine(box, "a"), support.Machine(box, "b")
        a.bead("init", "--prefix", "ex")
        x = a.bead("create", "x")["id"]
        a.bead("sync")
        b.bead("sync")
        bead = support.load_module()
        segment = next(p for p in (a.store() / "events").glob("*/*.jsonl"))
        last = json.loads(segment.read_text().splitlines()[-1])  # x's create: its id is the issue's uid
        future = [{"v": 1, "id": bead.uuid7(), "replica": last["replica"], "deps": [last["id"]], "op": "archive",
                   "issue": last["id"], "data": {"until": "2030-01-01"}}]
        future.append({"v": 2, "id": bead.uuid7(), "replica": last["replica"], "deps": [future[0]["id"]], "op": "set",
                       "issue": last["id"], "data": {"fields": {"title": "from the future"}}})
        foreign = "".join(json.dumps(e) + "\n" for e in future)
        with segment.open("a") as f:
            f.write(foreign)
        a.bead("update", x, "--title", "after")  # a normal write on top of events this bead does not understand
        a.bead("sync")
        b.bead("sync")
        for m in (a, b):
            problems = sorted(e["problem"][:17] for e in m.bead("info")["integrity"])
            self.assertEqual(problems, ["event format v2 i", "unknown operation"], m.name)
            self.assertEqual(m.show(x)["title"], "after", m.name)
            self.assertEqual(m.ready(), {x}, m.name)
        copy = b.store() / "events" / last["replica"] / segment.name
        self.assertIn(foreign, copy.read_text())  # carried untouched, to be applied by a bead that knows them
        self.assertEqual(a.export(), b.export())


# ---- roll-2psp (2026-10-08): a damaged private sync.git is rebuilt, not reported as the remote's fault
class SyncRepoDamage(unittest.TestCase):
    """linux-1's sync.git held empty loose objects after a drive; every sync then failed as REMOTE_UNREACHABLE
    though the remote was fine. sync.git is a cache of the remote and of events/, so bead sets a damaged one
    aside and makes it again, once per command, on the sync path and on the claim path alike."""
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "a")
        self.a.bead("init", "--prefix", "ex")
        self.key = self.a.bead("create", "x")["id"]
        self.a.bead("sync")
        self.b = support.Machine(self.box, "b")
        self.b.bead("sync")

    def damage(self):
        """Empty the loose object of the commit sync.git remembers as the remote's head: fetch cannot even
        negotiate ('cannot read existing object info')."""
        sync_git = self.a.store() / "sync.git"
        head = support.git("--git-dir", str(sync_git), "rev-parse", "refs/beads/remote").strip()
        loose = sync_git / "objects" / head[:2] / head[2:]
        self.assertTrue(loose.exists(), "the head commit is a loose object in a fresh sync.git")
        loose.chmod(0o644)
        loose.write_bytes(b"")
        return sync_git

    def test_sync_rebuilds_a_damaged_sync_repo_once_and_info_lists_the_copy(self):
        self.b.bead("create", "from b")
        self.b.bead("sync")
        sync_git = self.damage()
        synced = self.a.run("sync", "--json")
        self.assertEqual(synced.returncode, 0, synced.stderr)
        result = strict_json(synced.stdout)
        self.assertTrue(result.get("sync_repo_rebuilt", "").startswith(str(sync_git) + ".damaged-"), result)
        self.assertEqual(result["pulled"], 1)
        self.assertEqual(sorted(i["title"] for i in self.a.bead("list")["issues"]), ["from b", "x"])
        self.assertEqual(self.a.bead("info")["integrity"], [])
        copies = self.a.bead("info")["damaged_sync_repos"]
        self.assertEqual(len(copies), 1)
        self.assertTrue((self.a.store() / copies[0] / "HEAD").exists(), "the damaged repository is kept for a look")
        self.assertEqual(self.a.run("sync", "--json").returncode, 0)  # the rebuilt one is a normal sync.git
        self.assertNotIn("sync_repo_rebuilt", strict_json(self.a.run("sync", "--json").stdout))

    def test_a_claim_rebuilds_too_and_damage_twice_in_one_command_is_reported(self):
        self.damage()
        claimed = self.a.run("claim", self.key, "--json")
        self.assertEqual(claimed.returncode, 0, claimed.stderr)
        self.b.bead("sync")
        self.assertIn("codex:", self.b.show(self.key)["claim"]["owner"])  # the claim went through the rebuilt sync.git
        self.assertEqual(len(self.a.bead("info")["damaged_sync_repos"]), 1)
        # what counts as local damage: git's own read errors, never a line the remote relayed
        bead = support.load_module()
        self.assertTrue(bead.local_damage("error: object file objects/7d/04c9 is empty\nfatal: fetch-pack: invalid index-pack output"))
        self.assertTrue(bead.local_damage("fatal: missing object 7d04c9b4c3031216fa66cefa4cf7e3b79212bb32 for refs/beads/remote"))
        self.assertFalse(bead.local_damage("remote: error: object file objects/7d/04c9 is empty\nremote: fatal: bad object"))
        self.assertFalse(bead.local_damage("ssh: Could not resolve hostname github.com"))
