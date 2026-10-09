"""Replay: causality, registers, labels and dependencies, quarantine, validation, cycles.
Pure in-process checks on Journal/State; no git, fast.

Gathered from: first independent audit (AUDIT), 2026-10-06; second independent audit (REAUDIT), 2026-10-06; the author's suite; third independent review (REVIEW3), 2026-10-06. Docstrings and names that cite
finding ids (A.., R.., N.., C.., K.., P.., D..) refer to docs/FINDINGS.md."""
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

import tests.support as support
from tests.support import *  # noqa: F401,F403  (BEAD, Machine, Sandbox, load_module, git, code)


# ---- from test_poc.py: the author's suite
class UnitTest(unittest.TestCase):
    def events(self, bead, *specs):
        """Build a journal from (op, machine, deps, data) tuples; returns the journal and the ids."""
        journal, ids = bead.Journal(), []
        for op, machine, deps, data, issue in specs:
            event = {"v": 1, "id": bead.uuid7(), "replica": machine, "deps": [ids[d] for d in deps],
                     "actor": {"agent": "codex", "thread": f"{machine}-t"}, "op": op, "data": data}
            event["issue"] = ids[issue] if issue is not None else event["id"]
            journal.add(event, machine)
            ids.append(event["id"])
            time.sleep(0.002)
        return journal, ids

    def test_malformed_payloads_are_scoped_and_reported(self):
        bead = load_module()
        journal, ids = self.events(bead,
            ("init", "a", [], {"prefix": "ex"}, None),
            ("create", "a", [0], {"key": "ex-ok", "fields": {"title": "fine"}}, None),
            ("create", "a", [1], {"fields": {"title": "no key"}}, None),          # missing key
            ("dep_add", "a", [2], {"type": "blocks"}, 1),                          # missing target
            ("comment", "a", [3], {"text": "x"}, 1))
        journal.add({"v": 1, "id": bead.uuid7(), "replica": "a", "deps": [ids[-1]], "actor": ["list"], "op": "set",
                     "data": {"fields": {"title": "bad actor"}}, "issue": ids[1]}, "a")
        st = bead.replay(journal)
        self.assertEqual(st.issues[ids[1]]["title"], "fine")
        self.assertEqual(len(st.issues[ids[1]]["comments"]), 1)
        self.assertEqual(sorted(e["problem"][:6] for e in st.integrity), ["actor ", "create", "dep_ad"])

    def test_an_ignored_newer_format_event_cannot_change_ownership(self):
        bead = load_module()
        journal, ids = self.events(bead,
            ("init", "a", [], {"prefix": "ex"}, None),
            ("create", "a", [0], {"key": "ex-1", "fields": {"title": "t"}}, None),
            ("claim", "a", [1], {"mode": "online", "generation": 1}, 1),
            ("claim", "b", [2], {"mode": "online", "generation": 2, "takeover": None, "reason": "future"}, 1),
            ("close", "a", [2], {"claim": None, "reason": "done"}, 1))
        journal.events[ids[3]]["v"] = 2                       # a takeover in a format this bead does not know
        journal.events[ids[3]]["data"]["takeover"] = ids[2]
        journal.events[ids[4]]["data"]["claim"] = ids[2]
        st = bead.replay(journal)
        issue = st.issues[ids[1]]
        self.assertEqual(issue["status"], "closed")             # the owner's own close stands
        self.assertIsNone(issue["claim"])
        self.assertEqual(issue["stale"], [])
        self.assertEqual([e["problem"][:12] for e in st.integrity], ["event format"])

    def test_one_remote_has_one_identity_however_it_is_spelled(self):
        bead = load_module()
        spellings = ["git@github.com:Example/Project.git", "https://github.com/example/project",
                     "ssh://git@github.com/example/project.git", "https://github.com/example/project.git/"]
        self.assertEqual({bead.normalize_remote(s) for s in spellings}, {"github.com/example/project"})

    def test_the_coordination_ref_cannot_be_a_branch(self):
        bead = load_module()
        for ref in ("refs/heads/main", "refs/tags/v1", "main", "refs/beads/bad..name"):
            with self.assertRaises(bead.BeadError):
                bead.Workspace("w", None, None, {"ref": ref}).ref
        self.assertEqual(bead.Workspace("w", None, None, {"ref": "refs/tracker/journal"}).ref, "refs/tracker/journal")


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class ReplayAuditFixture:
    def __init__(self):
        self.bead = support.load_module()
        self.events = []
        self.counter = 0
        self.init = self.event('init', replica='seed', data={'prefix': 'au', 'name': 'audit'})
        self.issue = self.create('au-one')

    def event(self, op, issue=None, replica='seed', deps=None, data=None, actor=None):
        self.counter += 1
        value = ((1_790_000_000_000 + self.counter) << 80) | (7 << 76) | (2 << 62) | self.counter
        eid = str(uuid.UUID(int=value))
        event = {'v': 1, 'id': eid, 'replica': replica, 'deps': list(deps or []),
                 'actor': actor or {'agent': 'codex', 'thread': replica + '-thread'},
                 'op': op, 'issue': issue or eid, 'data': data or {}}
        self.events.append(event)
        return event

    def create(self, key, **fields):
        return self.event('create', deps=[self.events[-1]['id']],
                          data={'key': key, 'fields': {'title': key, **fields}})

    def journal(self, events=None):
        journal = self.bead.Journal()
        for e in (self.events if events is None else events):
            journal.add(e, e.get('replica', 'a') if isinstance(e, dict) else 'a')
        return journal

    def state(self, events=None):
        return self.bead.replay(self.journal(events))

    def snapshot(self, journal):
        st = self.bead.replay(journal)
        issues = sorted(st.issues.values(), key=lambda i: i['id'])
        return json.dumps({'export': self.bead.export_records(st),
                           'details': [self.bead.detail(st, i) for i in issues],
                           'ready': [i['id'] for i in issues if i['status'] == 'open'
                                     and not self.bead.is_blocked(st, i)
                                     and 'status' not in self.bead.conflicts(st, i)],
                           'integrity': st.integrity, 'pending': sorted(e['id'] for e in st.pending)},
                          sort_keys=True)


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class ReplayAuditProperties(unittest.TestCase):
    def test_six_machine_3000_event_replay_all_720_enumerations(self):
        f = ReplayAuditFixture()
        items = [f.issue] + [f.create(f'au-{n}') for n in range(1, 30)]
        for item in items[:5]:
            f.event('claim', item['id'], 'seed', [f.events[-1]['id']], {'mode': 'online', 'generation': 1})
        seed_head = f.events[-1]['id']
        replicas = ['seed', *[f'm{n}' for n in range(1, 6)]]
        heads = {replica: seed_head for replica in replicas}
        rng = random.Random(6403)
        for index in range(3000):
            replica = replicas[index % 6]
            issue = rng.choice(items)['id']
            deps = {heads[replica]}
            if index % 13 == 0:
                deps.add(heads[replicas[rng.randrange(6)]])
            choice = index % 8
            if choice == 0:
                op, data = 'set', {'fields': {'priority': rng.randrange(5)}}
            elif choice == 1:
                op, data = 'set', {'fields': {'notes': f'note {index}'}}
            elif choice == 2:
                op, data = 'label_add', {'labels': ['tag' + str(index % 5)]}
            elif choice == 3:
                op, data = 'label_remove', {'labels': ['tag' + str(index % 5)]}
            elif choice == 4:
                op, data = 'claim', {'mode': 'offline'}
            elif choice == 5:
                op, data = 'comment', {'text': f'comment {index}'}
            elif choice == 6:
                op, data = 'dep_add', {'to': rng.choice(items)['id'], 'type': 'related'}
            else:
                op, data = 'dep_remove', {'to': rng.choice(items)['id'], 'type': 'related'}
            e = f.event(op, issue, replica, sorted(deps), data)
            heads[replica] = e['id']
        grouped = {r: [e for e in f.events if e['replica'] == r] for r in heads}
        expected = f.snapshot(f.journal())
        for order in itertools.permutations(heads):
            j = f.bead.Journal()
            # Repeat delivery, including seed events, while preserving each journal's append order.
            for replica in (*order, *reversed(order)):
                for e in grouped[replica]:
                    j.add(e, replica)
            self.assertEqual(f.snapshot(j), expected)
        for _ in range(16):
            j = f.bead.Journal()
            remaining = {r: iter(events) for r, events in grouped.items()}
            while remaining:
                replica = rng.choice(list(remaining))
                event = next(remaining[replica], None)
                if event is None:
                    del remaining[replica]
                    continue
                j.add(event, replica)
                if rng.randrange(2):
                    j.add(event, replica)
            self.assertEqual(f.snapshot(j), expected)

    def test_missing_causes_hold_claim_and_close_of_pending_create(self):
        f = ReplayAuditFixture()
        missing = str(uuid.uuid4())
        predecessor = f.event('comment', f.issue['id'], 'a', [missing], {'text': 'waiting'})
        claim = f.event('claim', f.issue['id'], 'a', [predecessor['id']], {'mode': 'online', 'generation': 1})
        create = f.event('create', replica='b', deps=[missing], data={'key': 'au-pending'})
        close = f.event('close', create['id'], 'b', [create['id']])
        st = f.state()
        self.assertEqual({e['id'] for e in st.pending}, {e['id'] for e in [predecessor, claim, create, close]})
        self.assertIsNone(st.issues[f.issue['id']]['claim'])
        self.assertNotIn(create['id'], st.issues)

    def test_all_registers_keep_concurrent_clear_and_value_then_settle(self):
        bead = support.load_module()
        for field in bead.REGISTERS:
            if field in bead.REQUIRED_FIELDS:  # Q09: clearing a required field is a validation error, not a write
                f = ReplayAuditFixture()
                bad = f.event('set', f.issue['id'], 'a', [f.issue['id']], {'fields': {field: None}})
                st = f.state()
                self.assertIn(bad['id'], [row['event'] for row in st.integrity])
                self.assertEqual(st.issues[f.issue['id']][field], f.issue[field] if field in f.issue else st.issues[f.issue['id']][field])
                continue
            with self.subTest(field=field):
                f = ReplayAuditFixture()
                a = f.event('set', f.issue['id'], 'a', [f.issue['id']], {'fields': {field: None}})
                b = f.event('set', f.issue['id'], 'b', [f.issue['id']], {'fields': {field: 4 if field in ('priority', 'estimated_minutes') else 'value'}})
                st = f.state()
                self.assertEqual(len(f.bead.conflicts(st, st.issues[f.issue['id']])[field]), 2)
                self.assertIn(None, st.issues[f.issue['id']]['reg'][field].values())
                f.event('set', f.issue['id'], 'a', [a['id'], b['id']], {'fields': {field: None}})
                st = f.state()
                self.assertNotIn(field, f.bead.conflicts(st, st.issues[f.issue['id']]))
                self.assertNotIn(field, st.issues[f.issue['id']])

    def test_three_machine_add_wins_labels_and_dependencies(self):
        f = ReplayAuditFixture()
        target = f.create('au-target')
        for op, removal, payload in [('label_add', 'label_remove', {'labels': ['urgent']}),
                                     ('dep_add', 'dep_remove', {'to': target['id'], 'type': 'blocks'})]:
            a = f.event(op, f.issue['id'], 'a', [f.events[-1]['id']], payload)
            b = f.event(op, f.issue['id'], 'b', [a['id']], payload)
            c = f.event(removal, f.issue['id'], 'c', [a['id']], payload)
            st = f.state()
            issue = st.issues[f.issue['id']]
            self.assertTrue('urgent' in issue['labels'] if op == 'label_add' else (target['id'], 'blocks') in issue['deps'])
            f.event(removal, f.issue['id'], 'c', [b['id'], c['id']], payload)
            st = f.state()
            issue = st.issues[f.issue['id']]
            self.assertFalse('urgent' in issue['labels'] if op == 'label_add' else (target['id'], 'blocks') in issue['deps'])

    def test_cycle_after_merge_and_partial_removal(self):
        f = ReplayAuditFixture()
        other = f.create('au-two')
        a = f.event('dep_add', f.issue['id'], 'a', [other['id']], {'to': other['id'], 'type': 'blocks'})
        b = f.event('dep_add', other['id'], 'b', [other['id']], {'to': f.issue['id'], 'type': 'blocks'})
        st = f.state()
        self.assertEqual(len(f.bead.find_cycles(st)), 1)
        self.assertTrue(all(f.bead.is_blocked(st, i) for i in st.issues.values()))
        f.event('dep_remove', other['id'], 'b', [a['id'], b['id']], {'to': f.issue['id'], 'type': 'blocks'})
        st = f.state()
        self.assertEqual(f.bead.find_cycles(st), [])
        self.assertFalse(f.bead.is_blocked(st, st.issues[other['id']]))
        self.assertTrue(f.bead.is_blocked(st, st.issues[f.issue['id']]))

    def test_tombstoned_blocker_unblocks_unless_status_disputed(self):
        f = ReplayAuditFixture()
        child = f.create('au-child')
        edge = f.event('dep_add', child['id'], 'seed', [child['id']], {'to': f.issue['id'], 'type': 'blocks'})
        deleted = f.event('delete', f.issue['id'], 'a', [edge['id']])
        st = f.state()
        self.assertFalse(f.bead.is_blocked(st, st.issues[child['id']]))
        f.event('reopen', f.issue['id'], 'b', [edge['id']])
        st = f.state()
        self.assertIn('status', f.bead.conflicts(st, st.issues[f.issue['id']]))
        self.assertTrue(f.bead.is_blocked(st, st.issues[child['id']]))

    def test_handwritten_offline_takeover_does_not_fence_confirmed_owner(self):
        f = ReplayAuditFixture()
        claim = f.event('claim', f.issue['id'], 'a', [f.issue['id']], {'mode': 'online', 'generation': 1})
        f.event('claim', f.issue['id'], 'b', [claim['id']], {'mode': 'offline', 'takeover': claim['id']})
        f.event('close', f.issue['id'], 'a', [claim['id']], {'claim': claim['id']})
        st = f.state()
        self.assertIsNone(st.issues[f.issue['id']]['claim'])
        self.assertEqual(st.issues[f.issue['id']]['status'], 'closed')
        self.assertEqual(st.issues[f.issue['id']]['stale'], [])

    def test_causal_write_wins_despite_backwards_clock(self):
        f = ReplayAuditFixture()
        a = f.event('set', f.issue['id'], 'a', [f.issue['id']], {'fields': {'priority': 1}})
        b = f.event('set', f.issue['id'], 'b', [a['id']], {'fields': {'priority': 4}})
        b['id'] = '01000000-0000-7000-8000-000000000001'
        st = f.state()
        self.assertEqual(st.issues[f.issue['id']]['priority'], 4)
        self.assertNotIn('priority', f.bead.conflicts(st, st.issues[f.issue['id']]))

    def test_reused_event_ids_are_isolated_across_workspaces(self):
        f = ReplayAuditFixture()
        other = copy.deepcopy(f.events)
        other[-1]['data']['fields']['title'] = 'separate workspace'
        with tempfile.TemporaryDirectory(prefix='bead-audit-workspaces-') as home:
            with mock.patch.dict(os.environ, {'BEAD_HOME': home, 'BEAD_WORKSPACE': 'audit-explicit'}):
                one = f.bead.Workspace('one', None, None, {})
                two = f.bead.Workspace('two', None, None, {})
                f.bead.append_events(one, 'seed', f.events)
                f.bead.append_events(two, 'seed', other)
                states = [f.bead.replay(f.bead.load_journal(ws)) for ws in [one, two]]
                self.assertEqual([st.issues[f.issue['id']]['title'] for st in states], ['au-one', 'separate workspace'])
                self.assertEqual([st.integrity for st in states], [[], []])


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class ReplayAuditBugs(unittest.TestCase):
    def test_non_chain_same_journal_must_not_invent_ancestry(self):
        f = ReplayAuditFixture()
        a = f.event('set', f.issue['id'], 'a', [f.issue['id']], {'fields': {'priority': 1}})
        independent = f.event('comment', f.issue['id'], 'a', [f.issue['id']], {'text': 'did not see a'})
        b = f.event('set', f.issue['id'], 'b', [independent['id']], {'fields': {'priority': 4}})
        st = f.state()
        self.assertFalse(st.saw(b['id'], a['id']), 'same-journal position invents a causal dependency and erases a concurrent value')
        self.assertIn('priority', f.bead.conflicts(st, st.issues[f.issue['id']]))

    def test_non_chain_claim_cannot_bypass_pending_predecessor(self):
        f = ReplayAuditFixture()
        f.event('comment', f.issue['id'], 'a', [str(uuid.uuid4())], {'text': 'missing predecessor'})
        claim = f.event('claim', f.issue['id'], 'a', [f.issue['id']], {'mode': 'online', 'generation': 1})
        st = f.state()
        self.assertIsNone(st.issues[f.issue['id']]['claim'], 'later event in malformed journal becomes confirmed while its predecessor is pending')

    def test_identical_id_in_different_machine_journals_is_order_independent(self):
        f = ReplayAuditFixture()
        a = f.event('set', f.issue['id'], 'a', [f.issue['id']], {'fields': {'priority': 1}})
        b = f.event('set', f.issue['id'], 'b', [f.issue['id']], {'fields': {'priority': 4}})
        views = []
        for order in [('a', 'b'), ('b', 'a')]:
            j = f.journal(f.events[:2])
            for replica in order:
                # Machine b contains an identical misplaced copy of a, then its own concurrent edit.
                for e in ([a] if replica == 'a' else [a, b]):
                    j.add(e, replica)
            views.append(f.snapshot(j))
        self.assertEqual(views[0], views[1], 'first physical location of duplicate id changes version-vector identity and conflicts')

    def test_quarantined_takeover_cannot_supersede_a_valid_close(self):
        f = ReplayAuditFixture()
        claim = f.event('claim', f.issue['id'], 'a', [f.issue['id']], {'mode': 'online', 'generation': 1})
        takeover = f.event('claim', f.issue['id'], 'b', [claim['id']], {'mode': 'online', 'takeover': claim['id'], 'generation': 2})
        close = f.event('close', f.issue['id'], 'a', [claim['id']], {'claim': claim['id']})
        alternate = {**takeover, 'op': 'comment', 'data': {'text': 'conflicting payload'}}
        journals = []
        for variants in [(takeover, alternate), (alternate, takeover)]:
            j = f.journal([e for e in f.events if e['id'] != takeover['id']])
            for variant in variants:
                j.add(variant, 'b')
            journals.append(j)
        self.assertEqual(f.snapshot(journals[0]), f.snapshot(journals[1]),
                         'same two quarantined payloads produce different ownership when enumeration reverses')
        for j in journals:
            st = f.bead.replay(j)
            self.assertEqual(len(st.integrity), 1)
            self.assertIsNone(st.issues[f.issue['id']]['claim'], 'quarantined claim still participates in superseded pre-pass')
            self.assertEqual(st.issues[f.issue['id']]['status'], 'closed')

    def test_quarantined_event_with_missing_cause_does_not_hold_descendants(self):
        f = ReplayAuditFixture()
        bad = f.event('comment', f.issue['id'], 'a', [str(uuid.uuid4())], {'text': 'first payload'})
        child = f.event('set', f.issue['id'], 'a', [bad['id']], {'fields': {'notes': 'valid descendant'}})
        j = f.journal()
        j.add({**bad, 'deps': [f.issue['id']], 'data': {'text': 'second payload'}}, 'b')
        st = f.bead.replay(j)
        self.assertEqual(st.pending, [], 'quarantined event and descendant wait on first payload missing cause')
        self.assertEqual(st.issues[f.issue['id']]['notes'], 'valid descendant')

    def test_unknown_event_version_is_not_applied_silently(self):
        f = ReplayAuditFixture()
        e = f.event('set', f.issue['id'], 'a', [f.issue['id']], {'fields': {'title': 'future semantic version'}})
        e['v'] = 999
        st = f.state()
        self.assertEqual(st.issues[f.issue['id']]['title'], 'au-one')
        self.assertTrue(st.integrity)

    def test_unknown_operation_is_reported_and_does_not_mutate_timestamp(self):
        f = ReplayAuditFixture()
        before = f.state().issues[f.issue['id']]['updated_at']
        f.event('future-op', f.issue['id'], 'a', [f.issue['id']], {'important': 'not understood'})
        st = f.state()
        self.assertEqual(st.issues[f.issue['id']]['updated_at'], before)
        self.assertTrue(st.integrity)

    def test_bad_event_schema_has_scoped_damage(self):
        crashes = []
        for name in ['json-array', 'missing-id', 'missing-op', 'missing-replica', 'bad-id', 'deps-number', 'deps-dict', 'data-array']:
            f = ReplayAuditFixture()
            e = f.event('comment', f.issue['id'], 'a', [f.issue['id']], {'text': 'malformed'})
            if name == 'json-array':
                f.events[-1] = []
            elif name.startswith('missing-'):
                del e[name.removeprefix('missing-')]
            elif name == 'bad-id':
                e['id'] = 'not-a-uuid'
            elif name == 'deps-number':
                e['deps'] = [1]
            elif name == 'deps-dict':
                e['deps'] = [{}]
            else:
                e['data'] = ['invalid']
            try:
                st = f.state()
                if not st.integrity:
                    crashes.append((name, 'not reported in integrity'))
            except Exception as exc:
                crashes.append((name, type(exc).__name__ + ': ' + str(exc)))
        self.assertEqual(crashes, [], 'one malformed event crashes replay or silently waits instead of being scoped and reported')

    def test_second_parent_blockers_hold_child(self):
        f = ReplayAuditFixture()
        first = f.create('au-free-parent')
        second = f.create('au-blocked-parent')
        gate = f.create('au-gate')
        f.event('dep_add', f.issue['id'], 'seed', [f.events[-1]['id']], {'to': first['id'], 'type': 'parent-child'})
        f.event('dep_add', f.issue['id'], 'seed', [f.events[-1]['id']], {'to': second['id'], 'type': 'parent-child'})
        f.event('dep_add', second['id'], 'seed', [f.events[-1]['id']], {'to': gate['id'], 'type': 'blocks'})
        st = f.state()
        self.assertEqual([i['id'] for i in f.bead.open_blockers(st, st.issues[f.issue['id']])], ['au-gate'])


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class ReplayAuditThreadOwnership(unittest.TestCase):
    def test_distinct_threads_with_shared_first_eight_characters_are_not_same_owner(self):
        sandbox = support.Sandbox()
        self.addCleanup(sandbox.cleanup)
        a = support.Machine(sandbox, 'ownership')
        a.bead('init', '--prefix', 'au')
        issue = a.bead('create', 'thread ownership')['id']
        a.bead('claim', issue, thread='01a0c450-0001-7000-8000-000000000001')
        proc = a.run('release', issue, '--json', thread='01a0c450-0002-7000-8000-000000000002')
        self.assertNotEqual(proc.returncode, 0, 'different thread sharing displayed 8-char prefix released the claim')
        self.assertEqual(support.code(proc), 'NOT_OWNER')


# ---- from test_reaudit.py: second independent audit (REAUDIT), 2026-10-06
class ReplayReaudit(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="bead-replay-reaudit-")
        self.addCleanup(self.tmp.cleanup)
        env = patch.dict(os.environ, {"BEAD_HOME": str(Path(self.tmp.name) / "home"), "BEAD_WORKSPACE": "replay-reaudit"})
        env.start()
        self.addCleanup(env.stop)
        self.bead = support.load_module()
        self.serial = 0

    def event(self, op, machine="a", deps=(), issue=None, data=None):
        self.serial += 1
        # Fixed UUIDv7s make all order assertions independent of runtime clocks.
        eid = f"019a1111-1111-7000-8000-{self.serial:012x}"
        return {"v": 1, "id": eid, "replica": machine, "deps": list(deps),
                "actor": {"agent": "codex", "thread": machine + "-thread"},
                "op": op, "issue": issue or eid, "data": data or {}}

    def fixture(self):
        init = self.event("init", data={"prefix": "ex"})
        create = self.event("create", deps=[init["id"]], data={"key": "ex-ok", "fields": {"title": "survivor"}})
        return init, create

    def journal(self, events):
        j = self.bead.Journal()
        for event in events:
            j.add(event, event["replica"])
        return j

    def projection(self, journal):
        st = self.bead.replay(journal)
        return json.dumps({"records": self.bead.export_records(st), "details": [self.bead.detail(st, i) for i in st.issues.values()],
                           "integrity": sorted(st.integrity, key=lambda x: json.dumps(x, sort_keys=True)),
                           "pending": sorted(e["id"] for e in st.pending)}, sort_keys=True)

    def test_malformed_duplicate_order_does_not_change_projection(self):
        init, create = self.fixture()
        malformed = copy.deepcopy(create)
        malformed["op"] = ["create"]
        def build(variant_first):
            j = self.bead.Journal()
            j.add(init, "a")
            # Corruption in another machine's copy; valid envelope cannot be checked for placement.
            rows = [(malformed, "b"), (create, "a")]
            if not variant_first:
                rows.reverse()
            for event, machine in rows:
                j.add(event, machine)
            return j
        left, right = self.bead.replay(build(True)), self.bead.replay(build(False))
        self.assertEqual(self.bead.export_records(left), self.bead.export_records(right))
        self.assertNotIn(create["id"], left.issues, "ambiguous event must be ignored in either order")

    def test_quarantined_variant_cannot_hold_descendants_in_cycle(self):
        init, create = self.fixture()
        child = self.event("create", deps=[create["id"]], data={"key": "ex-child", "fields": {"title": "unrelated survivor"}})
        variant = copy.deepcopy(create)
        variant["deps"] = [child["id"]]
        j = self.journal([init, create, child, variant])
        st = self.bead.replay(j)
        self.assertEqual(st.pending, [], "quarantined event must not hold later frontier events")
        self.assertIn(child["id"], st.issues)
        self.assertEqual(len(st.integrity), 1)

    def test_invalid_timestamp_is_scoped_even_when_event_is_inert(self):
        init, create = self.fixture()
        invalid = self.event("future_op", "b", deps=[create["id"]])
        invalid["id"] = "ffffffff-ffff-7000-8000-000000000000"
        j = self.journal([init, create, invalid])
        st = self.bead.replay(j)
        self.assertEqual(st.issues[create["id"]]["title"], "survivor")
        self.assertTrue(st.integrity)

    def test_non_scalar_actor_thread_is_scoped(self):
        init, create = self.fixture()
        invalid = self.event("comment", "b", deps=[create["id"]], issue=create["id"], data={"text": "bad"})
        invalid["actor"]["thread"] = ["not", "scalar"]
        st = self.bead.replay(self.journal([init, create, invalid]))
        self.assertEqual(st.issues[create["id"]]["comments"], [])
        self.assertTrue(st.integrity)

    def test_malformed_set_is_atomic_and_cannot_overwrite_internal_state(self):
        init, create = self.fixture()
        invalid = self.event("set", "b", deps=[create["id"]], issue=create["id"],
                             data={"fields": {"title": "partial poison", "reg": None}})
        st = self.bead.replay(self.journal([init, create, invalid]))
        self.assertEqual(st.issues[create["id"]]["title"], "survivor")
        self.assertIsInstance(st.issues[create["id"]]["reg"], dict)
        self.assertTrue(st.integrity)

    def test_duplicate_create_uid_does_not_destroy_issue(self):
        init, create = self.fixture()
        overwrite = self.event("create", "b", deps=[create["id"]], issue=create["id"],
                               data={"key": "ex-other", "fields": {"title": "replacement"}})
        st = self.bead.replay(self.journal([init, create, overwrite]))
        self.assertEqual(st.resolve("ex-ok")["title"], "survivor")
        self.assertTrue(st.integrity)

    def test_merged_parent_cycle_matches_br_readiness(self):
        sandbox = support.Sandbox()
        self.addCleanup(sandbox.cleanup)
        a, b = support.Machine(sandbox, "a"), support.Machine(sandbox, "b")
        a.bead("init", "--prefix", "ex")
        x = a.bead("create", "first task")["id"]
        y = a.bead("create", "second task")["id"]
        a.bead("sync")
        b.bead("sync")
        a.bead("update", x, "--parent", y)
        b.bead("update", y, "--parent", x)
        for m in (a, b, a, b):
            m.bead("sync")
        self.assertEqual(a.bead("dep", "cycles")["count"], 1)
        self.assertEqual(a.ready(), {x, y}, "non-epic parent cycles match br 0.5.7 readiness")

    def test_offline_forged_takeover_stays_provisional_and_does_not_fence_close(self):
        init, create = self.fixture()
        claim = self.event("claim", "a", [create["id"]], create["id"], {"mode": "online", "generation": 1})
        fake = self.event("claim", "b", [claim["id"]], create["id"], {"mode": "offline", "generation": 999,
                                                                                "takeover": claim["id"], "reason": "offline"})
        close = self.event("close", "a", [claim["id"]], create["id"], {"claim": claim["id"]})
        st = self.bead.replay(self.journal([init, create, claim, fake, close]))
        issue = st.issues[create["id"]]
        self.assertEqual(issue["status"], "closed")
        self.assertIsNone(issue["claim"])
        self.assertEqual(issue["stale"], [])
        self.assertEqual(issue["claim_generation"], 1)

    def test_orphan_takeover_does_not_fence_another_issues_close(self):
        init, create = self.fixture()
        claim = self.event("claim", "a", [create["id"]], create["id"], {"mode": "online", "generation": 1})
        orphan = self.event("claim", "b", [claim["id"]], "019a1111-1111-7000-8000-111111111111",
                            {"mode": "online", "generation": 2, "takeover": claim["id"], "reason": "wrong target"})
        close = self.event("close", "a", [claim["id"]], create["id"], {"claim": claim["id"]})
        st = self.bead.replay(self.journal([init, create, claim, orphan, close]))
        issue = st.issues[create["id"]]
        self.assertEqual(issue["status"], "closed")
        self.assertIsNone(issue["claim"])
        self.assertEqual(issue["stale"], [])

    def test_partially_invalid_label_add_applies_nothing(self):
        init, create = self.fixture()
        bad = self.event("label_add", "b", [create["id"]], create["id"], {"labels": ["must-not-stick", {}]})
        st = self.bead.replay(self.journal([init, create, bad]))
        self.assertTrue(st.integrity)
        self.assertEqual(st.issues[create["id"]]["labels"], set(), "event reported as invalid must not mutate valid state")

    def test_six_machine_3600_mixed_envelope_events_are_scoped_and_deterministic(self):
        rng = random.Random(901834)
        init, create = self.fixture()
        rows = [(init, "a"), (create, "a")]
        heads = {"a": create["id"]}
        valid_comments = 0
        for n in range(3600):
            machine = rng.choice("abcdef")
            deps = [heads[machine]] if machine in heads else [create["id"]]
            if rng.random() < .2:
                deps.extend(h for m, h in heads.items() if m != machine)
            event = self.event("comment", machine, deps, create["id"], {"text": f"comment {n}"})
            mode = n % 6
            if mode == 0:
                event["data"] = ["invalid type"]
            elif mode == 1:
                event["op"] = "unknown_operation"
            elif mode == 2:
                event["v"] = 2
            elif mode == 3:
                event["actor"] = ["invalid type"]
            elif mode == 4:
                event.update(op="set", data={"fields": []})
            else:
                valid_comments += 1
            rows.append((event, machine))
            heads[machine] = event["id"]
        def build(order):
            j = self.bead.Journal()
            for machine in order:
                for event, owner in rows:
                    if owner == machine:
                        j.add(event, machine)
            return j
        base = build("abcdef")
        st = self.bead.replay(base)
        self.assertEqual(st.pending, [])
        self.assertEqual(st.chain_breaks, {})
        self.assertEqual(len(st.integrity), 3000)
        self.assertEqual(len(st.issues[create["id"]]["comments"]), valid_comments)
        expected = self.projection(base)
        for order in rng.sample(list(itertools.permutations("abcdef")), 18):
            self.assertEqual(self.projection(build(order)), expected)

    def test_deep_valid_dependency_graph_does_not_overflow_cycle_scan(self):
        init, first = self.fixture()
        events, last = [init, first], first["id"]
        issues = [first["id"]]
        for n in range(1500):
            event = self.event("create", "a", [last], data={"key": f"ex-chain-{n}", "fields": {"title": f"chain {n}"}})
            issues.append(event["id"])
            events.append(event)
            last = event["id"]
        for source, target in zip(issues, issues[1:]):
            event = self.event("dep_add", "a", [last], source, {"to": target, "type": "blocks"})
            events.append(event)
            last = event["id"]
        st = self.bead.replay(self.journal(events))
        self.assertEqual(st.integrity, [])
        self.assertEqual(self.bead.find_cycles(st), [])

    def test_valid_six_machine_6000_events_converge_and_ancestry_is_exact(self):
        rng = random.Random(901833)
        init, create = self.fixture()
        events = [init, create]
        heads = {"a": create["id"]}
        parents = {init["id"]: 0, create["id"]: 1}
        idx = {init["id"]: 0, create["id"]: 1}
        for n in range(5998):
            machine = rng.choice("abcdef")
            deps = [heads[machine]] if machine in heads else [create["id"]]
            deps += [head for m, head in heads.items() if m != machine and rng.random() < .08]
            op = rng.choice(["set", "label_add", "label_remove", "comment"])
            if op == "set":
                field = rng.choice(self.bead.REGISTERS)
                value = (n % 5 if field == "priority" else n % 180 if field == "estimated_minutes" else
                         "task" if field == "issue_type" else f"value{n % 31}" if field == "title" else
                         rng.choice([None, f"value{n % 31}"]))
                data = {"fields": {field: value}}
            elif op.startswith("label"):
                data = {"labels": [f"label{n % 13}"]}
            else:
                data = {"text": f"comment{n}"}
            event = self.event(op, machine, deps, create["id"], data)
            ancestors = 0
            for dep in deps:
                ancestors |= parents[dep] | (1 << idx[dep])
            idx[event["id"]] = len(events)
            parents[event["id"]] = ancestors
            events.append(event)
            heads[machine] = event["id"]
        base = self.journal(events)
        expected = self.projection(base)
        st = self.bead.replay(base)
        self.assertEqual(st.integrity, [])
        self.assertEqual(st.pending, [])
        for _ in range(10000):
            later = rng.randrange(1, len(events))
            earlier = rng.randrange(later)
            self.assertEqual(st.saw(events[later]["id"], events[earlier]["id"]), bool(parents[events[later]["id"]] & (1 << earlier)))
        # Arbitrary interleaving of complete machine journals, preserving each journal's bytes.
        orders = rng.sample(list(itertools.permutations("abcdef")), 36)
        for order in orders:
            j = self.bead.Journal()
            for machine in order:
                for eid in base.order[machine]:
                    j.add(base.events[eid], machine)
                    j.add(base.events[eid], machine)
            self.assertEqual(self.projection(j), expected)

    def test_all_registers_preserve_none_against_concurrent_value(self):
        init, create = self.fixture()
        clearable = [k for k in self.bead.REGISTERS if k not in self.bead.REQUIRED_FIELDS]  # Q09: the rest cannot be cleared
        left = self.event("set", "b", [create["id"]], create["id"], {"fields": {k: None for k in clearable}})
        right = self.event("set", "c", [create["id"]], create["id"],
                           {"fields": {k: (1 if self.bead.FIELD_TYPES.get(k) is int else "a value") for k in clearable}})
        j = self.journal([init, create, left, right])
        st = self.bead.replay(j)
        self.assertEqual(set(self.bead.conflicts(st, st.issues[create["id"]])), set(clearable))
        settled = self.event("set", "d", [left["id"], right["id"]], create["id"], {"fields": {k: None for k in clearable}})
        j.add(settled, "d")
        st = self.bead.replay(j)
        self.assertEqual(self.bead.conflicts(st, st.issues[create["id"]]), {})

    def test_missing_predecessor_holds_claim_and_close_then_recovers(self):
        init, create = self.fixture()
        claim = self.event("claim", "b", [create["id"]], create["id"], {"mode": "online", "generation": 1})
        close = self.event("close", "b", [claim["id"]], create["id"], {"claim": claim["id"]})
        j = self.journal([init, claim, close])
        st = self.bead.replay(j)
        self.assertEqual({e["id"] for e in st.pending}, {claim["id"], close["id"]})
        self.assertEqual(st.issues, {})
        j.add(create, "a")
        st = self.bead.replay(j)
        self.assertEqual(st.pending, [])
        self.assertEqual(st.issues[create["id"]]["status"], "closed")
        self.assertIsNone(st.issues[create["id"]]["claim"])

    def test_ignored_op_with_valid_id_does_not_hold_descendant(self):
        init, create = self.fixture()
        unknown = self.event("future_op", "a", [create["id"]])
        child = self.event("comment", "a", [unknown["id"]], create["id"], {"text": "survivor"})
        st = self.bead.replay(self.journal([init, create, unknown, child]))
        self.assertEqual(st.pending, [])
        self.assertEqual(len(st.integrity), 1)
        self.assertEqual([c["text"] for c in st.issues[create["id"]]["comments"]], ["survivor"])


# ---- from test_review3.py: third independent review (REVIEW3), 2026-10-06
class ReplayThirdReview(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='replay3-')
        self.addCleanup(self.temp.cleanup)
        self.env = patch.dict(os.environ, {'BEAD_HOME': str(Path(self.temp.name)/'home'), 'BEAD_WORKSPACE':'replay3'})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.bead = support.load_module()
        self.serial = 0

    def event(self, op, replica='a', deps=(), issue=None, data=None):
        self.serial += 1
        eid = f'019a1111-1111-7000-8000-{self.serial:012x}'
        return {'v':1, 'id':eid, 'replica':replica, 'deps':list(deps), 'actor':{'agent':'test','thread':replica},
                'op':op, 'issue':issue or eid, 'data':data or {}}

    def base(self):
        init = self.event('init', data={'prefix':'ex'})
        create = self.event('create', deps=[init['id']], data={'key':'ex-one','fields':{'title':'survivor'}})
        return init, create

    def journal(self, events, detached=()):
        j = self.bead.Journal()
        for event in events:
            j.add(event, event['replica'], sequenced=event['id'] not in detached)
        j.settle()
        return j

    def test_quarantine_agreed_cycle_is_scoped(self):
        init, create = self.base()
        bad = self.event('comment', 'b', issue=create['id'], data={'text':'variant one'})
        child = self.event('comment', 'c', [bad['id']], create['id'], {'text':'surviving child'})
        bad['deps'] = [child['id']]
        variant = copy.deepcopy(bad)
        variant['data']['text'] = 'variant two'
        j = self.journal([init,create,bad,child,variant])
        try:
            st = self.bead.replay(j)
        except Exception as error:
            self.fail(f'quarantined cycle crashed replay: {error!r}')
        self.assertEqual(st.pending, [])
        self.assertEqual([c['text'] for c in st.issues[create['id']]['comments']], ['surviving child'])
        self.assertTrue(st.integrity)

    def test_quarantine_malformed_scalar_deps_is_scoped(self):
        init, create = self.base()
        comment = self.event('comment', 'b', [create['id']], create['id'], {'text':'ambiguous'})
        variant = copy.deepcopy(comment)
        variant['deps'] = 17
        j = self.journal([init,create,comment,variant])
        try:
            st = self.bead.replay(j)
        except Exception as error:
            self.fail(f'malformed duplicate crashed replay: {error!r}')
        self.assertEqual(st.issues[create['id']]['comments'], [])
        self.assertTrue(st.integrity)

    def test_malformed_imported_comment_does_not_break_queries(self):
        init, create = self.base()
        create['data']['comments'] = [{'text':'missing author and timestamp'}]
        j = self.journal([init,create])
        st = self.bead.replay(j)
        try:
            self.bead.export_records(st)
            for issue in st.issues.values():
                self.bead.detail(st, issue)
        except Exception as error:
            self.fail(f'accepted malformed embedded comment crashes query/export: {error!r}; integrity={st.integrity!r}')
        self.assertTrue(st.integrity, 'malformed embedded comment must make its event inert')
        self.assertNotIn(create['id'], st.issues)

    def test_create_without_issue_id_is_inert(self):
        init, create = self.base()
        del create['issue']
        st = self.bead.replay(self.journal([init,create]))
        self.assertTrue(st.integrity, 'missing create issue ID must not install inaccessible None-keyed issue')
        self.assertNotIn(None, st.issues)

    def test_new_comment_integer_survives_collision_with_later_legacy_import(self):
        init, create = self.base()
        comment = self.event('comment', 'a', [create['id']], create['id'], {'text':'existing'})
        events = [init,create,comment]
        before = self.bead.export_records(self.bead.replay(self.journal(events)))[0]['comments'][0]['id']
        other = self.event('create','a',[comment['id']],data={'key':'ex-two','fields':{'title':'imported'},
               'comments':[{'legacy_id':before,'author':'legacy','text':'legacy','created_at':'2020-01-01T00:00:00Z'}], 'imported':True})
        after = self.bead.export_records(self.bead.replay(self.journal(events+[other])))
        existing = next(r for r in after if r['id']=='ex-one')['comments'][0]['id']
        self.assertEqual(existing,before,'later imported legacy ID renumbers an already-exported bead comment')

    def test_identical_misplaced_duplicate_does_not_reorder_own_journal(self):
        init, create = self.base()
        first = self.event('comment', 'z', [create['id']], create['id'], {'text':'first'})
        second = self.event('comment', 'z', [first['id']], create['id'], {'text':'second'})
        results = []
        for misplaced_first in (False, True):
            j = self.bead.Journal()
            j.add(init, 'a')
            j.add(create, 'a')
            if misplaced_first:
                j.add(second, 'b')
            j.add(first, 'z')
            j.add(second, 'z')
            if not misplaced_first:
                j.add(second, 'b')
            j.settle()
            st = self.bead.replay(j)
            results.append(([c['text'] for c in st.issues[create['id']]['comments']],st.chain_breaks))
        self.assertEqual(results[0],results[1], 'misplaced identical duplicate reordered the actual owner journal')
        self.assertEqual(results[0],(['first','second'],{}))

    def test_valid_colliding_comments_keep_exported_ids_when_other_machine_arrives(self):
        import hashlib
        init, create = self.base()
        seen = {}
        pair = None
        for n in range(100000):
            eid = f'019a1111-2222-7000-8000-{n:012x}'
            number = 1_000_000 + int(hashlib.sha256(eid.encode()).hexdigest()[:8],16) % 9_000_000
            if number in seen:
                pair = (seen[number],eid)
                break
            seen[number] = eid
        self.assertIsNotNone(pair, 'deterministic fixture should find a hash collision')
        earlier = self.event('comment','b',[create['id']],create['id'],{'text':'arrives later'})
        later = self.event('comment','c',[create['id']],create['id'],{'text':'already exported'})
        earlier['id'],later['id'] = pair
        st = self.bead.replay(self.journal([init,create,later]))
        before = self.bead.export_records(st)[0]['comments'][0]['id']
        st = self.bead.replay(self.journal([init,create,later,earlier]))
        self.assertEqual(st.integrity,[])
        comments = self.bead.export_records(st)[0]['comments']
        after = next(c['id'] for c in comments if c['text']=='already exported')
        self.assertEqual(after,before,f'valid comments {pair} collide and remote arrival renumbers existing export')

    def test_optional_register_clears_remain_queryable(self):
        init, create = self.base()
        clear = self.event('set','a',[create['id']],create['id'],{'fields':{'description':None,'design':None,'notes':None,'assignee':None}})
        st = self.bead.replay(self.journal([init,create,clear]))
        self.assertEqual(st.integrity,[])
        self.bead.export_records(st)
        self.bead.detail(st,st.issues[create['id']])
        self.assertEqual(st.issues[create['id']]['title'],'survivor')

    def test_merged_dependency_cycle_reports_remaining_cycle_after_partial_removal(self):
        init, first = self.base()
        second = self.event('create','a',[first['id']],data={'key':'ex-two','fields':{'title':'two'}})
        third = self.event('create','a',[second['id']],data={'key':'ex-three','fields':{'title':'three'}})
        events=[init,first,second,third]
        previous=third['id']
        # Two elementary cycles sharing A->B; removing one leaf edge leaves the other.
        for source,target in [(first,second),(second,first),(second,third),(third,first)]:
            event=self.event('dep_add','a',[previous],source['id'],{'to':target['id'],'type':'blocks'})
            events.append(event);previous=event['id']
        before=self.bead.replay(self.journal(events))
        self.assertEqual(len(self.bead.find_cycles(before)),2)
        removed=self.event('dep_remove','a',[previous],second['id'],{'to':first['id'],'type':'blocks'})
        after=self.bead.replay(self.journal(events+[removed]))
        self.assertEqual(self.bead.find_cycles(after),[['ex-one','ex-two','ex-three']])
        self.assertTrue(all(self.bead.is_blocked(after,issue) for issue in after.issues.values()))

    def test_mixed_detached_ownership_ancestry_matches_exact_graph(self):
        rng = random.Random(71029)
        init, create = self.base()
        events = [init,create]
        ancestors = {init['id']:set(),create['id']:{init['id']}}
        heads = {'a':create['id']}
        detached = set()
        for n in range(1500):
            machine = rng.choice('abcdef')
            deps = ([heads[machine]] if machine in heads else [create['id']])
            deps += [h for m,h in heads.items() if m!=machine and rng.random()<.1]
            event = self.event('comment',machine,deps,create['id'],{'text':str(n)})
            if n%3==0:
                detached.add(event['id'])  # a conditional edit shares ownership-file placement
            ancestors[event['id']] = set(deps).union(*(ancestors[d] for d in deps))
            events.append(event)
            heads[machine] = event['id']
        st = self.bead.replay(self.journal(events,detached))
        self.assertEqual(st.integrity,[])
        self.assertEqual(st.pending,[])
        for _ in range(10000):
            a,b = rng.sample(events,2)
            self.assertEqual(st.saw(a['id'],b['id']),b['id'] in ancestors[a['id']])
        expected = self.bead.export_records(st)
        for _ in range(12):
            order = list('abcdef')
            rng.shuffle(order)
            variant = [e for m in order for e in events if e['replica']==m]
            self.assertEqual(self.bead.export_records(self.bead.replay(self.journal(variant,detached))),expected)


# ---- from test_review3.py: third independent review (REVIEW3), 2026-10-06
class ReplayResiduals(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not os.environ.get("BEAD_HOME"):
            raise RuntimeError("set BEAD_HOME to a scratch path before running these probes")
        cls.bead = load_bead()

    def setUp(self):
        self.serial = 0

    def event(self, op, replica="a", deps=(), issue="self", data=None):
        self.serial += 1
        event_id = f"019a3333-3333-7000-8000-{self.serial:012x}"
        event = {
            "v": 1,
            "id": event_id,
            "replica": replica,
            "deps": list(deps),
            "actor": {"agent": "codex", "thread": replica + "-thread"},
            "op": op,
            "data": data or {},
        }
        if issue != "omitted":
            event["issue"] = event_id if issue == "self" else issue
        return event

    def add_all(self, rows):
        journal = self.bead.Journal()
        for event, physical_replica in rows:
            journal.add(event, physical_replica)
        return journal

    def base(self):
        init = self.event("init", data={"prefix": "ex"})
        create = self.event(
            "create",
            deps=[init["id"]],
            data={"key": "ex-one", "fields": {"title": "survivor"}},
        )
        return init, create

    def test_missing_issue_creates_cannot_alias_and_overwrite(self):
        init = self.event("init", data={"prefix": "ex"})
        first = self.event(
            "create",
            deps=[init["id"]],
            issue="omitted",
            data={"key": "ex-first", "fields": {"title": "first"}},
        )
        second = self.event(
            "create",
            replica="b",
            deps=[first["id"]],
            issue="omitted",
            data={"key": "ex-second", "fields": {"title": "second"}},
        )
        state = self.bead.replay(
            self.add_all([(init, "a"), (first, "a"), (second, "b")])
        )

        # Either normalize omitted create UIDs to their event IDs or reject the
        # malformed creates. They must never share the None UID and overwrite.
        self.assertNotIn(None, state.issues)
        if first["id"] in state.issues or second["id"] in state.issues:
            self.assertEqual(state.resolve("ex-first")["title"], "first")
            self.assertEqual(state.resolve("ex-second")["title"], "second")
        else:
            self.assertGreaterEqual(len(state.integrity), 2)

    def test_identical_misplaced_duplicate_does_not_change_broken_chain_result(self):
        init, create = self.base()
        first = self.event(
            "set",
            replica="a",
            deps=[create["id"]],
            issue=create["id"],
            data={"fields": {"title": "first"}},
        )
        second = self.event(
            "set",
            replica="a",
            deps=[create["id"]],
            issue=create["id"],
            data={"fields": {"title": "second"}},
        )

        def projection(misplaced_first):
            rows = [(init, "a"), (create, "a")]
            if misplaced_first:
                rows.append((second, "b"))
            rows.extend([(first, "a"), (second, "a")])
            if not misplaced_first:
                rows.append((second, "b"))
            state = self.bead.replay(self.add_all(rows))
            issue = state.issues[create["id"]]
            return {
                "title": issue["title"],
                "conflicts": self.bead.conflicts(state, issue),
                "integrity": state.integrity,
                "chain_breaks": state.chain_breaks,
            }

        self.assertEqual(projection(True), projection(False))

    def test_takeover_must_causally_see_the_claim_it_supersedes(self):
        init, create = self.base()
        claim = self.event(
            "claim",
            replica="a",
            deps=[create["id"]],
            issue=create["id"],
            data={"mode": "online", "generation": 1},
        )
        takeover = self.event(
            "claim",
            replica="b",
            deps=[create["id"]],
            issue=create["id"],
            data={
                "mode": "online",
                "generation": 2,
                "takeover": claim["id"],
                "reason": "forged without seeing predecessor",
            },
        )
        close = self.event(
            "close",
            replica="a",
            deps=[claim["id"]],
            issue=create["id"],
            data={"claim": claim["id"], "reason": "done"},
        )
        state = self.bead.replay(
            self.add_all(
                [(init, "a"), (create, "a"), (claim, "a"), (takeover, "b"), (close, "a")]
            )
        )
        self.assertFalse(state.saw(takeover["id"], claim["id"]))
        issue = state.issues[create["id"]]
        self.assertEqual(issue["status"], "closed")
        self.assertIsNone(issue["claim"])
        self.assertEqual(issue["stale"], [])

    def test_quarantined_common_descendant_dependency_is_scoped(self):
        init, create = self.base()
        quarantined = self.event(
            "comment",
            replica="b",
            issue=create["id"],
            data={"text": "variant one"},
        )
        child = self.event(
            "comment",
            replica="c",
            deps=[quarantined["id"]],
            issue=create["id"],
            data={"text": "descendant survives"},
        )
        quarantined["deps"] = [child["id"]]
        variant = {
            **quarantined,
            "data": {"text": "variant two"},
        }
        state = self.bead.replay(
            self.add_all(
                [(init, "a"), (create, "a"), (quarantined, "b"), (child, "c"), (variant, "b")]
            )
        )
        self.assertEqual(state.pending, [])
        self.assertEqual(
            [comment["text"] for comment in state.issues[create["id"]]["comments"]],
            ["descendant survives"],
        )
        self.assertEqual(len(state.integrity), 1)

    def test_malformed_embedded_create_comment_is_scoped(self):
        init, create = self.base()
        malformed = self.event(
            "create",
            replica="b",
            deps=[create["id"]],
            data={
                "key": "ex-bad",
                "fields": {"title": "malformed"},
                "comments": [{"text": "missing author and created_at"}],
            },
        )
        state = self.bead.replay(
            self.add_all([(init, "a"), (create, "a"), (malformed, "b")])
        )

        # A rejected malformed issue is acceptable; so is a fully normalized one.
        # In either case, one bad embedded comment cannot disable global export/detail.
        records = self.bead.export_records(state)
        self.assertTrue(any(record["id"] == "ex-one" for record in records))
        if malformed["id"] in state.issues:
            self.bead.detail(state, state.issues[malformed["id"]])
        else:
            self.assertTrue(state.integrity)

    def test_cycle_scan_reports_each_elementary_cycle(self):
        init = self.event("init", data={"prefix": "ex"})
        rows = [(init, "a")]
        last = init["id"]
        issues = []
        for key in ("a", "b", "c"):
            create = self.event(
                "create",
                deps=[last],
                data={"key": "ex-" + key, "fields": {"title": key}},
            )
            rows.append((create, "a"))
            issues.append(create["id"])
            last = create["id"]
        for source, target in ((0, 1), (0, 2), (1, 2), (2, 0)):
            edge = self.event(
                "dep_add",
                deps=[last],
                issue=issues[source],
                data={"to": issues[target], "type": "blocks"},
            )
            rows.append((edge, "a"))
            last = edge["id"]
        cycles = {tuple(cycle) for cycle in self.bead.find_cycles(self.bead.replay(self.add_all(rows)))}
        self.assertEqual(cycles, {("ex-a", "ex-b", "ex-c"), ("ex-a", "ex-c")})


# ---- maintainer's regression for P01 (REVIEW3)
class ReplayScale(unittest.TestCase):
    def test_ownership_ancestry_stays_small_on_a_long_claim_release_history(self):
        """P01: ancestry over ownership events is a shared bitset per event, not a cumulative set.
        3,000 alternating claim/release events replay in well under a second and keep at most one
        distinct bitset per event."""
        bead = load_module()
        events, prev, uid, active = [], None, None, None
        for i in range(3000):
            eid = str(uuid.UUID(int=((1_770_000_000_000 + i) << 80) | (7 << 76) | (2 << 62)))
            if i == 0:
                op, data, issue = "init", {"prefix": "ex"}, eid
            elif i == 1:
                op, data, issue = "create", {"key": "ex-1", "fields": {"title": "t"}}, eid
                uid = eid
            elif i % 2 == 0:
                op, data, issue = "claim", {"mode": "online", "generation": i // 2}, uid
                active = eid
            else:
                op, data, issue = "release", {"claim": active}, uid
            events.append({"v": 1, "id": eid, "replica": "m", "actor": {"agent": "x", "thread": "t"},
                           "deps": [prev] if prev else [], "op": op, "issue": issue, "data": data})
            prev = eid
        journal = bead.Journal()
        for event in events:
            journal.add(event, "m", sequenced=event["op"] in ("init", "create"))
        started = time.time()
        st = bead.replay(journal)
        self.assertLess(time.time() - started, 1.0)
        self.assertEqual(st.integrity, [])
        self.assertLessEqual(len(st._shared), len(events))
        self.assertTrue(st.saw(events[-1]["id"], events[2]["id"]))
        self.assertFalse(st.saw(events[2]["id"], events[-1]["id"]))
        self.assertIsNone(st.issues[uid]["claim"])  # the history ends on a release


# ---- from test_review4.py: fourth independent review (2026-10-07-4-codex), folded in after the fixes
class ReplayReview4(unittest.TestCase):
    @staticmethod
    def event(number, op, replica="a", deps=(), issue=None, data=None, actor=True):
        event_id = f"019a4444-4444-7000-8000-{number:012x}"
        return {
            "v": 1,
            "id": event_id,
            "replica": replica,
            "deps": list(deps),
            "actor": {"agent": "test", "thread": replica} if actor else [],
            "op": op,
            "issue": issue or event_id,
            "data": data or {},
        }

    def test_q07_invalid_placeholder_cannot_bridge_a_broken_machine_chain(self):
        """Q07: an inert placeholder must not invent predecessor ancestry for its tail."""
        bead = support.load_module()
        init = self.event(1, "init", "seed", data={"prefix": "ex"})
        create = self.event(
            2, "create", "seed", [init["id"]],
            data={"key": "ex-one", "fields": {"title": "one"}},
        )
        first = self.event(
            3, "set", "a", [create["id"]], create["id"], {"fields": {"priority": 1}}
        )
        malformed = self.event(
            4, "comment", "a", [create["id"]], create["id"], {"text": "bad"}, actor=False
        )
        later = self.event(
            5, "set", "a", [malformed["id"]], create["id"], {"fields": {"priority": 4}}
        )
        journal = bead.Journal()
        for event in (init, create, first, malformed, later):
            journal.add(event, event["replica"])
        state = bead.replay(journal)
        self.assertFalse(state.saw(later["id"], first["id"]))
        self.assertEqual(state.issues[create["id"]]["priority"], 1)
        self.assertIn("a", state.chain_breaks)

    def test_q09_required_register_clears_remain_queryable(self):
        """Q09: required-field clears must be rejected or leave every query usable."""
        bead = support.load_module()
        for offset, field in enumerate(("title", "priority", "issue_type"), 10):
            with self.subTest(field=field):
                init = self.event(offset, "init", "seed", data={"prefix": "ex"})
                create = self.event(
                    offset + 1, "create", "seed", [init["id"]],
                    data={"key": f"ex-{field}", "fields": {
                        "title": "one", "priority": 2, "issue_type": "task",
                    }},
                )
                clear = self.event(
                    offset + 2, "set", "seed", [create["id"]], create["id"],
                    {"fields": {field: None}},
                )
                journal = bead.Journal()
                for event in (init, create, clear):
                    journal.add(event, event["replica"])
                state = bead.replay(journal)
                issue = state.issues[create["id"]]
                if state.integrity:
                    self.assertTrue(any(row.get("event") == clear["id"] for row in state.integrity))
                    self.assertEqual(issue[field], create["data"]["fields"][field])
                try:
                    bead.brief(state, issue)
                    bead.line(state, issue)
                    bead.export_records(state)
                except Exception as error:  # the accepted clear must stay query-safe
                    self.fail(f"clearing {field} broke a query: {error!r}")

    def test_d01_cycle_enumerator_matches_every_graph_through_four_nodes(self):
        """D01 verification: every <=4-node directed graph matches a brute-force oracle."""
        bead = support.load_module()

        class State:
            def __init__(self, size, edges):
                self.issues = {
                    str(node): {
                        "uid": str(node), "id": str(node),
                        "deps": {(str(target), "blocks"): {} for target in edges[node]},
                    }
                    for node in range(size)
                }

            def waits_for(self, uid):
                return sorted(target for target, _kind in self.issues[uid]["deps"])

            @staticmethod
            def key(uid):
                return uid

        def brute_force(size, edges):
            cycles = set()
            for start in range(size):
                def walk(path):
                    for target in edges[path[-1]]:
                        if target == start:
                            cycle = tuple(map(str, path))
                            rotations = [cycle[index:] + cycle[:index] for index in range(len(cycle))]
                            cycles.add(min(rotations))
                        elif target not in path and target >= start:
                            walk(path + [target])
                walk([start])
            return cycles

        for size in range(1, 5):
            pairs = list(itertools.product(range(size), repeat=2))
            for mask in range(1 << len(pairs)):
                edges = [set() for _ in range(size)]
                for bit, (source, target) in enumerate(pairs):
                    if mask >> bit & 1:
                        edges[source].add(target)
                expected = brute_force(size, edges)
                actual = {tuple(cycle) for cycle in bead.find_cycles(State(size, edges))}
                self.assertEqual(actual, expected, (size, mask, edges))


# ---- from test_review5.py: fifth independent review (2026-10-07-5-codex), folded in after the fixes
class Review5Replay(unittest.TestCase):
    def test_e10_extra_cannot_override_canonical_export_fields(self):
        """E10: a valid event's compatibility extras cannot forge its exported identity/state."""
        bead = support.load_module()
        init_id = "019a4444-4444-7000-8000-000000000001"
        create_id = "019a4444-4444-7000-8000-000000000002"
        init = {
            "v": 1, "id": init_id, "replica": "peer", "deps": [],
            "actor": {"agent": "test", "thread": "peer"}, "op": "init",
            "issue": init_id, "data": {"prefix": "ex", "name": "test"},
        }
        create = {
            "v": 1, "id": create_id, "replica": "peer", "deps": [init_id],
            "actor": {"agent": "test", "thread": "peer"}, "op": "create",
            "issue": create_id,
            "data": {
                "key": "ex-real",
                "fields": {"title": "real", "status": "tombstone", "priority": 2,
                           "issue_type": "task"},
                "labels": [], "comments": [],
                "extra": {"id": "ex-spoof", "status": "open", "comments": "forged"},
            },
        }
        journal = bead.Journal()
        journal.add(init, "peer")
        journal.add(create, "peer")
        problem = bead.payload_problem(create)
        if problem is not None:
            return
        state = bead.replay(journal)
        self.assertEqual(state.integrity, [])
        exported = bead.export_records(state)[0]
        self.assertEqual(exported["id"], "ex-real")
        self.assertEqual(exported["status"], "tombstone")
        self.assertIsInstance(exported.get("comments", []), list)


# ---- the Rust port's Phase 1 (roll-k2wy.2): input CPython read by accident is refused by every bead
class PortAlignment(unittest.TestCase):
    """U01-U04 and U06-U10 (docs/FINDINGS.md; docs/RUST-PORT.md §5 rows 3, 6, 7, 22, 24, 25 and 27-29):
    what a journal line means no longer depends on the Python reading it, and nothing a create carries
    breaks a query, so Python and Rust beads agree."""

    @staticmethod
    def event(number, op, replica="a", deps=(), issue=None, data=None):
        event_id = f"019a5555-5555-7000-8000-{number:012x}"
        return {"v": 1, "id": event_id, "replica": replica, "deps": list(deps),
                "actor": {"agent": "test", "thread": replica}, "op": op, "issue": issue or event_id, "data": data or {}}

    def read(self, bead, *lines: bytes):
        """A segment of these lines, each terminated: what it yields and what it reports."""
        problems = []
        events = bead.read_segment(b"".join(line + b"\n" for line in lines), "a/000001.jsonl", problems)
        return events, [problem["problem"] for problem in problems]

    def line(self, number, **data) -> bytes:
        return json.dumps(self.event(number, "init", data=data), separators=(",", ":")).encode()

    def test_u01_numbers_beyond_a_float_are_not_json_numbers(self):
        """U01: 1e400 read as infinity, which no export can write; like NaN, it is refused."""
        bead = support.load_module()
        for text in ("1e400", "-1e400", "1E400", "[2.5e308]", '{"x": NaN}', "Infinity"):
            with self.subTest(text=text):
                with self.assertRaisesRegex(ValueError, "is not a JSON number"):
                    bead.loads_strict(text)
        self.assertEqual(bead.loads_strict("[1e308, -0.0, 5e-324]"), [1e308, -0.0, 5e-324])
        events, problems = self.read(bead, self.line(1).replace(b'"data":{}', b'"data":{"x":1e400}'), self.line(2))
        self.assertEqual([event["id"] for event in events], [self.event(2, "init")["id"]])
        self.assertEqual(problems, ["line is not valid JSON"])

    def test_u02_ids_and_deps_with_a_trailing_newline_are_not_uuids(self):
        """U02: `$` let an id or a dependency end in a newline; ids match the UUID shape whole."""
        bead = support.load_module()
        create = self.event(1, "init", data={"prefix": "ex"})
        self.assertIsNone(bead.event_problem(create))
        self.assertEqual(bead.event_problem({**create, "id": create["id"] + "\n"}), "id is not a UUID")
        self.assertEqual(bead.event_problem({**create, "deps": [create["id"] + "\n"]}), "deps is not a list of event ids")
        journal = bead.Journal()
        journal.add({**create, "id": create["id"] + "\n"}, "a")
        state = bead.replay(journal)
        self.assertIsNone(state.prefix)
        self.assertEqual([row["problem"] for row in state.integrity], ["id is not a UUID"])

    def test_u03_nesting_deeper_than_512_levels_is_unreadable_on_every_python(self):
        """U03: Python 3.11 read about 950 levels, 3.12 and 3.13 about 10,000, 3.14 over 100,000; every
        bead now reads 512 and no deeper, counting brackets in strings for nothing."""
        bead = support.load_module()
        self.assertEqual(bead.MAX_DEPTH, 512)
        self.assertEqual(bead.nesting(json.dumps(bead.loads_strict("[" * 512 + "]" * 512))), 512)
        with self.assertRaisesRegex(ValueError, "nested deeper than 512 levels"):
            bead.loads_strict("[" * 513 + "]" * 513)
        self.assertEqual(bead.loads_strict('["' + "[{" * 600 + '"]'), ["[{" * 600])
        deep, fine = ("[" * depth + "]" * depth for depth in (511, 510))  # the event's data adds one level
        events, problems = self.read(bead, self.line(1).replace(b'"data":{}', b'"data":{"x":' + deep.encode() + b"}"),
                                     self.line(2).replace(b'"data":{}', b'"data":{"x":' + fine.encode() + b"}"))
        self.assertEqual([event["id"] for event in events], [self.event(2, "init")["id"]])
        self.assertEqual(problems, ["line is not valid JSON"])

    def test_u04_journal_lines_are_utf8_whatever_their_first_bytes(self):
        """U04: json.loads on bytes guessed UTF-16 or UTF-32 from the first bytes and skipped a UTF-8 BOM,
        so such a line was an event; a journal line is UTF-8, and anything else is unreadable."""
        bead = support.load_module()
        text = self.line(1).decode()
        for name, line in (("bom", b"\xef\xbb\xbf" + text.encode()), ("utf-16-le", text.encode("utf-16-le")),
                           ("utf-16-be", text.encode("utf-16-be")), ("utf-32", text.encode("utf-32")),
                           ("latin-1", text.replace('"test"', '"tést"').encode("latin-1"))):
            with self.subTest(encoding=name):
                events, problems = self.read(bead, line, self.line(2))
                self.assertEqual([event["id"] for event in events], [self.event(2, "init")["id"]])
                self.assertEqual(problems, ["line is not valid JSON"])
        # still UTF-8, surrogates and all: a raw lone surrogate reads as U+FFFD, reported
        events, problems = self.read(bead, text.replace('"test"', '"t\\ud800"').encode())
        self.assertEqual(events[0]["actor"]["agent"], "t�")
        self.assertEqual(problems, ["text is not valid UTF-8; it reads as U+FFFD"])

    def test_u06_key_numbers_are_ascii_digits(self):
        """U06: \\d matched any Unicode digit, so a renamed copy of `x.٣` became `x.3` and a child of a
        parent with child `p.١` counted on from 1; a key's number is ASCII digits only."""
        bead = support.load_module()
        init = self.event(1, "init", "seed", data={"prefix": "ex"})
        parent = self.event(2, "create", "seed", [init["id"]], data={"key": "ex-p", "fields": {"title": "p"}})
        child = self.event(3, "create", "seed", [parent["id"]], data={"key": "ex-p.١", "fields": {"title": "c"}})
        first = self.event(4, "create", "a", [init["id"]], data={"key": "x.٣", "fields": {"title": "one"}})
        second = self.event(5, "create", "b", [init["id"]], data={"key": "x.٣", "fields": {"title": "two"}})
        journal = bead.Journal()
        for event in (init, parent, child, first, second):
            journal.add(event, event["replica"])
        state = bead.replay(journal)
        renamed = state.issues[second["id"]]["id"]
        self.assertTrue(renamed.startswith("x.٣") and len(renamed) > len("x.٣"), renamed)
        self.assertEqual(state.renames, [{"from": "x.٣", "to": renamed, "replica": "b", "uid": second["id"]}])
        self.assertEqual(bead.mint_key(state, "seed", state.issues[parent["id"]]), "ex-p.1")

    def test_u07_payloads_that_reached_the_last_resort_are_refused_by_name(self):
        """U07: three payloads raised inside replay, which then stored Python's exception text (different
        on every Python version); they are refused when validated, with the same text everywhere."""
        bead = support.load_module()
        init = self.event(1, "init", "seed", data={"prefix": "ex"})
        target = self.event(2, "create", "seed", [init["id"]], data={"key": "ex-t", "fields": {"title": "t"}})
        taken = self.event(3, "create", "seed", [target["id"]], data={"key": "x." + "1" * 4301, "fields": {"title": "x"}})
        rows = {"dep_add event has a malformed 'meta'": {"to": target["id"], "meta": [1]},
                "dep_remove event has a malformed 'key'": {"to": target["id"], "key": ["ex-t"]}}
        journal, previous = bead.Journal(), taken
        for event in (init, target, taken):
            journal.add(event, event["replica"])
        for number, (problem, data) in enumerate(rows.items(), 4):
            op = problem.split()[0]
            event = self.event(number, op, "seed", [previous["id"]], target["id"], data)  # one chain: each builds on the last
            self.assertEqual(bead.payload_problem(event), problem)
            journal.add(event, "seed")
            previous = event
        state = bead.replay(journal)
        found = sorted(row["problem"] for row in state.integrity)
        self.assertEqual(found, sorted([*rows, "create event has no valid 'key'"]))
        self.assertFalse(any("could not be applied" in problem for problem in found))
        # a renamed copy counts up from its number, and 10**4300 has one digit too many to print
        self.assertEqual(bead.payload_problem(self.event(9, "create", data={"key": "x." + "9" * 4300})),
                         "create event has no valid 'key'")
        nines = "x." + "9" * 4299
        journal = bead.Journal()
        for number, replica in ((10, "a"), (11, "b")):
            journal.add(self.event(number, "create", replica, data={"key": nines}), replica)
        state = bead.replay(journal)
        self.assertEqual([issue["id"] for issue in state.issues.values()], [nines, "x.1" + "0" * 4299])
        self.assertEqual(state.integrity, [])

    def test_u08_imported_comments_carry_neither_a_uid_nor_a_where(self):
        """U08: a create's comments are imported ones. One naming a uid or a where, which only a comment made
        here has, broke `show` or `export` (a where that is not an object, a uid that is not a string) or
        exported as a comment made here; it is refused."""
        bead = support.load_module()
        comment = {"text": "hi", "author": None, "created_at": None}
        for extra in ({"uid": 5}, {"uid": "x"}, {"uid": None}, {"where": None}, {"where": {"branch": "b"}}):
            with self.subTest(extra=extra):
                create = self.event(2, "create", data={"key": "ex-1", "comments": [{**comment, **extra}]})
                self.assertEqual(bead.payload_problem(create), "create event has a malformed comment")
        init = self.event(1, "init", data={"prefix": "ex"})
        kept = self.event(2, "create", deps=[init["id"]], data={"key": "ex-1", "comments": [comment, {**comment, "legacy_id": 7}]})
        refused = self.event(3, "create", deps=[kept["id"]], data={"key": "ex-2", "comments": [{**comment, "where": None}]})
        journal = bead.Journal()
        for event in (init, kept, refused):
            journal.add(event, "a")
        state = bead.replay(journal)
        self.assertEqual(list(state.issues), [kept["id"]])
        self.assertEqual([row["problem"] for row in state.integrity], ["create event has a malformed comment"])
        self.assertEqual([c["id"] for c in bead.detail(state, state.issues[kept["id"]])["comments"]], [None, 7])
        self.assertEqual([c["id"] for c in bead.export_records(state)[0]["comments"]], [None, 7])

    def test_u10_a_quarantined_id_names_its_issue_only_by_a_string(self):
        """U10: the stand-in for an id with several contents named the issue of the copy placed first; when
        no copy was valid that could be a list or an object, and replay failed hashing it."""
        bead = support.load_module()
        for issue, shown in (([1], None), ({"a": 1}, None), (5, None), ("ex-1", "ex-1")):
            with self.subTest(issue=issue):
                first = {**self.event(1, "create"), "issue": issue}
                journal = bead.Journal()
                for data in ({}, {"x": 1}):
                    journal.add({**first, "data": data}, "a")
                state = bead.replay(journal)
                self.assertEqual(state.integrity, [{"event": first["id"], "issue": shown,
                                                    "problem": "one event id with different contents"}])

    def test_u09_a_create_cannot_clear_created_at(self):
        """U09: an issue created with `created_at: null` (an import of such a record) made `list` and `ready`
        fail comparing its time with the others'; a create cannot clear it, as it cannot clear a title."""
        bead = support.load_module()
        create = self.event(1, "create", data={"key": "ex-1", "fields": {"created_at": None}})
        self.assertEqual(bead.payload_problem(create), "field 'created_at' cannot be cleared")
        for name in ("updated_at", "created_by", "closed_at"):
            with self.subTest(field=name):
                self.assertIsNone(bead.payload_problem(self.event(1, "create", data={"key": "ex-1", "fields": {name: None}})))


if __name__ == "__main__":
    unittest.main()
