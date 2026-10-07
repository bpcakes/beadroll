"""Storage: segments, torn tails, rotation, readers against writers, machine identity.

Gathered from: second independent audit (REAUDIT), 2026-10-06. Docstrings and names that cite
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

import tests.support as support
from tests.support import *  # noqa: F401,F403  (BEAD, Machine, Sandbox, load_module, git, code)


# ---- from test_reaudit.py: second independent audit (REAUDIT), 2026-10-06
class StorageReaudit(unittest.TestCase):
    def setUp(self):
        self.sandbox = support.Sandbox()
        self.addCleanup(self.sandbox.cleanup)
        self.a = support.Machine(self.sandbox, 'storage-a')
        self.a.bead('init', '--prefix', 'ex')
        self.key = self.a.bead('create', 'storage survivor')['id']

    def own(self, machine=None):
        machine = machine or self.a
        return machine.bead('info')['machine']

    def segment(self, machine=None):
        machine = machine or self.a
        return sorted((machine.store() / 'events' / self.own(machine)).glob('*.jsonl'))[-1]

    def two(self):
        self.a.bead('sync')
        b = support.Machine(self.sandbox, 'storage-b')
        b.bead('sync')
        return b

    def script(self, name, source):
        path = self.sandbox.root / name
        path.write_text(source)
        return path

    def start(self, machine, driver, *args, **env):
        process = subprocess.Popen([sys.executable, str(driver), *args], cwd=machine.checkout,
            env=machine.env(**env), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, start_new_session=True)
        self.addCleanup(self.stop, process)
        return process

    def stop(self, process):
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
        process.communicate(timeout=10)

    def wait(self, marker, process):
        deadline = time.monotonic() + 10
        while not marker.exists() and time.monotonic() < deadline:
            if process.poll() is not None:
                self.fail(f'worker quit before {marker}: {process.communicate()}')
            time.sleep(.01)
        self.assertTrue(marker.exists(), str(marker))

    def test_invalid_own_remote_record_must_not_disable_sync(self):
        """Replay scopes JSON null, but remote_own must use the same validation boundary."""
        self.a.bead('sync')
        with self.segment().open('ab') as stream:
            stream.write(b'null\n')
        info = self.a.bead('info')
        self.assertEqual(len(info['integrity']), 1)
        self.assertEqual(self.a.show(self.key)['title'], 'storage survivor')
        self.assertEqual(self.a.bead('sync')['integrity'], 1)  # publication itself succeeds
        again = self.a.run('sync', '--json')
        self.assertEqual(again.returncode, 0,
            'a previously quarantined malformed record now disables sync:\n' + again.stderr)

    def test_refused_claiming_update_leaves_the_journal_untouched_under_a_reader(self):
        """Two actual processes race; a real show is paused after it enumerates files. The edits of
        a claiming update travel with the claim, so a refusal removes pending files only: the
        journal is byte-identical before, during and after, and the reader sees no half state."""
        b = self.two()
        publish_ready, publish_go = self.sandbox.root / 'publish-ready', self.sandbox.root / 'publish-go'
        read_ready, read_go = self.sandbox.root / 'read-ready', self.sandbox.root / 'read-go'
        driver = self.script('pause-publish.py', f'''import runpy, sys, time
from pathlib import Path
bp = runpy.run_path({support.BEAD!r})
Link = bp['Link']
real = Link.publish
first = True
def publish(self, *args, **kwargs):
    global first
    if first:
        first = False
        Path({str(publish_ready)!r}).touch()
        while not Path({str(publish_go)!r}).exists(): time.sleep(.005)
    return real(self, *args, **kwargs)
Link.publish = publish
sys.exit(bp['main']())
''')
        journal_before = {p: p.read_bytes() for p in (self.a.store() / 'events' / self.own()).glob('*.jsonl')}
        claimer = self.start(self.a, driver, 'update', self.key, '--claim', '--title', 'refused title',
                             '--add-label', 'refused', '--json', BEAD_ROTATE_BYTES='1')
        self.wait(publish_ready, claimer)
        pending = sorted((self.a.store() / 'pending').glob('*.json'))
        self.assertEqual(len(pending), 1, 'the attempt is one file')
        self.assertEqual([json.loads(l)['op'] for l in pending[0].read_text().splitlines()], ['claim', 'set', 'label_add'],
                         'claim, set and label_add form one attempt')
        self.assertEqual({p: p.read_bytes() for p in (self.a.store() / 'events' / self.own()).glob('*.jsonl')}, journal_before)
        reader_driver = self.script('pause-reader.py', f'''import runpy, sys, time
from pathlib import Path
bp = runpy.run_path({support.BEAD!r})
g = bp['load_journal'].__globals__
real = g['segment_files']
def segment_files(ws):
    paths = real(ws)
    Path({str(read_ready)!r}).touch()
    while not Path({str(read_go)!r}).exists(): time.sleep(.005)
    return paths
g['segment_files'] = segment_files
sys.exit(bp['main']())
''')
        reader = self.start(self.a, reader_driver, 'show', self.key, '--json')
        self.wait(read_ready, reader)
        accepted = b.bead('claim', self.key)[0]['claim']
        publish_go.touch()
        _, claim_err = claimer.communicate(timeout=15)
        self.assertEqual(claimer.returncode, 1, claim_err)
        self.assertEqual(json.loads(claim_err)['error']['code'], 'ALREADY_CLAIMED')
        self.assertEqual(sorted((self.a.store() / 'pending').glob('*.json')), [], 'a refused attempt leaves nothing behind')
        self.assertEqual({p: p.read_bytes() for p in (self.a.store() / 'events' / self.own()).glob('*.jsonl')}, journal_before)
        read_go.touch()
        read_out, read_err = reader.communicate(timeout=10)
        self.assertEqual(reader.returncode, 0, read_err)
        shown = json.loads(read_out)[0]
        self.assertEqual(shown['title'], 'storage survivor')
        self.assertNotIn('refused', shown.get('labels', []))
        self.assertIn(shown.get('claim'), (None, accepted), 'the reader showed something other than nothing or the accepted claim')
        self.a.bead('sync')
        self.assertEqual(self.a.show(self.key)['claim'], accepted)
        self.assertEqual(self.a.show(self.key)['title'], 'storage survivor')

    def test_reader_never_confirms_a_claim_whose_push_was_rejected(self):
        """An unlocked journal/marker snapshot must not promote a refused claim."""
        b = self.two()
        publish_ready, publish_go = self.sandbox.root / 'claim-ready', self.sandbox.root / 'claim-go'
        read_ready, read_go = self.sandbox.root / 'view-ready', self.sandbox.root / 'view-go'
        driver = self.script('pause-claim.py', f'''import runpy, sys, time
from pathlib import Path
bp = runpy.run_path({support.BEAD!r})
Link = bp['Link']
real = Link.publish
first = True
def publish(self, *args, **kwargs):
    global first
    if first:
        first = False
        Path({str(publish_ready)!r}).touch()
        while not Path({str(publish_go)!r}).exists(): time.sleep(.005)
    return real(self, *args, **kwargs)
Link.publish = publish
sys.exit(bp['main']())
''')
        claimer = self.start(self.a, driver, 'claim', self.key, '--json')
        self.wait(publish_ready, claimer)
        failed_id = json.loads(next((self.a.store() / 'pending').glob('*.json')).read_text().splitlines()[0])['id']
        reader_driver = self.script('pause-view.py', f'''import runpy, sys, time
from pathlib import Path
bp = runpy.run_path({support.BEAD!r})
g = bp['view'].__globals__
real = g['load_pending']
def load_pending(ws):
    Path({str(read_ready)!r}).touch()
    while not Path({str(read_go)!r}).exists(): time.sleep(.005)
    return real(ws)
g['load_pending'] = load_pending
sys.exit(bp['main']())
''')
        reader = self.start(self.a, reader_driver, 'show', self.key, '--json')
        self.wait(read_ready, reader)
        accepted = b.bead('claim', self.key)[0]['claim']
        publish_go.touch()
        _, claim_err = claimer.communicate(timeout=15)
        self.assertEqual(claimer.returncode, 1, claim_err)
        self.assertEqual(json.loads(claim_err)['error']['code'], 'ALREADY_CLAIMED')
        self.assertEqual(list((self.a.store() / 'pending').glob('*.json')), [])
        self.assertNotEqual(accepted['id'], failed_id)
        read_go.touch()
        read_out, read_err = reader.communicate(timeout=10)
        self.assertEqual(reader.returncode, 0, read_err)
        shown = json.loads(read_out)[0]
        self.assertNotEqual(shown.get('claim', {}).get('id'), failed_id,
            'reader promoted an online-rejected claim to a confirmed owner: ' + read_out)

    def test_machine_identity_first_use_is_atomic_across_workspaces(self):
        """Two workspaces have separate locks but share the newly-created replica.json. The identity
        is written whole to a temporary file and linked into place; pause right before the link."""
        fresh = support.Machine(self.sandbox, 'identity-fresh')
        ready, go = self.sandbox.root / 'identity-ready', self.sandbox.root / 'identity-go'
        driver = self.script('pause-identity.py', f'''import runpy, sys, time
from pathlib import Path
bp = runpy.run_path({support.BEAD!r})
os = bp['os']
real = os.link
def linked(src, dst, *args, **kwargs):
    if Path(dst).name == 'replica.json':
        Path({str(ready)!r}).touch()
        while not Path({str(go)!r}).exists(): time.sleep(.005)
    return real(src, dst, *args, **kwargs)
os.link = linked
sys.exit(bp['main']())
''')
        first = self.start(fresh, driver, 'init', '--json', BEAD_WORKSPACE='identity-one')
        self.wait(ready, first)
        second = self.start(fresh, support.BEAD, 'init', '--json', BEAD_WORKSPACE='identity-two')
        try:
            second_out, second_err = second.communicate(timeout=2)
        except subprocess.TimeoutExpired:  # a correct implementation may wait for the creator
            go.touch()
            second_out, second_err = second.communicate(timeout=10)
        else:
            go.touch()
        first_out, first_err = first.communicate(timeout=10)
        self.assertEqual(first.returncode, 0, first_err)
        self.assertEqual(second.returncode, 0,
            'another workspace observed the half-written global machine id:\n' + second_err)
        one = fresh.run('info', '--json', BEAD_WORKSPACE='identity-one')
        two = fresh.run('info', '--json', BEAD_WORKSPACE='identity-two')
        self.assertEqual((one.returncode, two.returncode), (0, 0), one.stderr + two.stderr)
        self.assertEqual(json.loads(one.stdout)['machine'], json.loads(two.stdout)['machine'],
                         'the two workspaces ended up with different machine identities')

    def test_machine_identity_can_recover_from_first_use_crash(self):
        """SIGKILL at the write boundary must not permanently disable all writes: the identity file
        is either absent or complete, never half written."""
        fresh = support.Machine(self.sandbox, 'identity-killed')
        marker = self.sandbox.root / 'identity-killed-ready'
        driver = self.script('kill-identity.py', f'''import runpy, sys, time
from pathlib import Path
bp = runpy.run_path({support.BEAD!r})
os = bp['os']
real = os.link
def linked(src, dst, *args, **kwargs):
    if Path(dst).name == 'replica.json':
        Path({str(marker)!r}).touch()
        time.sleep(30)
    return real(src, dst, *args, **kwargs)
os.link = linked
sys.exit(bp['main']())
''')
        process = self.start(fresh, driver, 'init', '--json', BEAD_WORKSPACE='identity-crash')
        self.wait(marker, process)
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate(timeout=10)
        self.assertFalse((fresh.home / 'replica.json').exists(), 'a half-made identity became visible')
        self.assertTrue(list(fresh.home.glob('.replica.*.tmp')), 'the interrupted write should have been in progress')
        retry = fresh.run('init', '--json', BEAD_WORKSPACE='identity-crash')
        self.assertEqual(retry.returncode, 0,
            'first-use crash left a permanent unreadable machine identity:\n' + retry.stderr)

    def test_repair_preserves_a_complete_record_that_only_lacks_newline(self):
        expected = self.a.export()
        segment = self.segment()
        segment.write_bytes(segment.read_bytes().rstrip(b'\n'))
        self.assertEqual(self.a.export(), expected)
        self.a.bead('create', 'after missing separator')
        self.a.bead('sync')
        self.assertEqual(self.a.bead('list')['total'], 2)
        self.assertFalse(list(segment.parent.glob('*.torn-*')))
        self.assertEqual(self.a.bead('info')['integrity'], [])

    def test_repair_foreign_fragment_before_publish_retains_authoritative_events(self):
        b = self.two()
        segment = self.segment()
        relative = segment.relative_to(self.a.store())
        foreign = b.store() / relative
        with foreign.open('ab') as stream:
            stream.write(b'{"v":1,"id":"unfinished')
        before = self.a.export()
        result = b.bead('sync')
        self.assertEqual(result['integrity'], 0)
        self.assertEqual(self.a.export(), before)
        self.assertEqual(b.export(), before)
        self.assertEqual(foreign.read_bytes(), segment.read_bytes())
        self.assertEqual(len(list(foreign.parent.glob('*.torn-*'))), 1)
        self.assertEqual(self.a.bead('sync')['integrity'], 0)

    def test_rewritten_foreign_file_preserves_local_backup_and_published_bytes(self):
        b = self.two()
        segment = self.segment()
        relative = segment.relative_to(self.a.store())
        foreign = b.store() / relative
        original = foreign.read_bytes()
        rows = [json.loads(line) for line in foreign.read_text().splitlines()]
        rows[-1]['data']['fields']['title'] = 'damaged local foreign title'
        damaged = b''.join(json.dumps(row).encode() + b'\n' for row in rows)
        foreign.write_bytes(damaged)
        result = b.bead('sync')
        self.assertEqual(result['forked_journals'], [str(relative)])
        self.assertEqual(foreign.read_bytes(), original)
        backups = list(foreign.parent.glob('*.fork-*'))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), damaged)
        self.assertEqual(self.a.bead('sync')['integrity'], 0)
        self.assertEqual(self.a.export(), b.export())

    def test_copied_identity_divergence_across_rotation_refuses_remote_change(self):
        self.two()
        twin = support.Machine(self.sandbox, 'identity-twin')
        shutil.copytree(self.a.home, twin.home)
        self.a.extra['BEAD_ROTATE_BYTES'] = '1'
        twin.extra['BEAD_ROTATE_BYTES'] = '9999999'
        twin.bead('update', self.key, '--title', 'twin branch')
        self.a.bead('update', self.key, '--title', 'original branch')
        self.a.bead('sync')
        tip = support.git('--git-dir', str(self.sandbox.origin), 'rev-parse', 'refs/beads/journal')
        attempted = twin.run('sync', '--json')
        self.assertEqual(support.code(attempted), 'JOURNAL_FORK')
        self.assertEqual(support.git('--git-dir', str(self.sandbox.origin), 'rev-parse', 'refs/beads/journal'), tip)
        self.assertEqual(self.a.show(self.key)['title'], 'original branch')


if __name__ == "__main__":
    unittest.main()
