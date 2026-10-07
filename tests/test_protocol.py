"""Ownership protocol: claims, attempts, pending and fences, lost acknowledgements, takeovers,
sync races. Real subprocesses against local bare remotes.

Gathered from: first independent audit (AUDIT), 2026-10-06; second independent audit (REAUDIT), 2026-10-06; third independent review (REVIEW3), 2026-10-06. Docstrings and names that cite
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

import contextlib
import tests.support as support
from tests.support import *  # noqa: F401,F403  (BEAD, Machine, Sandbox, load_module, git, code)


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class SyncAudit(unittest.TestCase):
    def setUp(self):
        self.sandbox = support.Sandbox()
        self.addCleanup(self.sandbox.cleanup)
        self.a = support.Machine(self.sandbox, 'a')
        self.a.bead('init', '--prefix', 'audit')
        self.key = self.a.bead('create', 'Original issue')['id']

    def two(self):
        self.a.bead('sync')
        b = support.Machine(self.sandbox, 'b')
        b.bead('sync')
        return b

    def own_segment(self, machine=None):
        machine = machine or self.a
        own = machine.bead('info')['machine']
        return sorted((machine.store() / 'events' / own).glob('*.jsonl'))[-1]

    def remote_blob(self, path):
        return support.git('--git-dir', str(self.sandbox.origin), 'show', 'refs/beads/journal:' + path)

    def wait_marker(self, path, process):
        until = time.monotonic() + 15
        while not path.exists() and time.monotonic() < until:
            if process.poll() is not None:
                self.fail('worker exited before marker: ' + repr(process.communicate()))
            time.sleep(.01)
        self.assertTrue(path.exists(), 'instrumented fault point was not reached')

    def git_wrapper(self, action, marker, after=True):
        folder = self.sandbox.root / ('wrapper-' + action)
        folder.mkdir(exist_ok=True)
        script = folder / 'git'
        # Only delay the specified transport operation. All other git commands remain real.
        script.write_text('#!' + sys.executable + '\n' +
                          'import os, subprocess, sys, time\n' +
                          'from pathlib import Path\n' +
                          'args = sys.argv[1:]\n' +
                          'if args and args[0] == ' + repr(action) + ':\n' +
                          ('    status = subprocess.call([' + repr(shutil.which('git')) + ', *args])\n' if after else '') +
                          '    Path(' + repr(str(marker)) + ').write_text("ready")\n' +
                          '    time.sleep(20)\n' +
                          ('    sys.exit(status)\n' if after else '') +
                          'os.execv(' + repr(shutil.which('git')) + ', ["git", *args])\n')
        script.chmod(0o755)
        return str(folder) + os.pathsep + os.environ['PATH']

    def test_torn_tail_reader_ignores_only_unterminated_fragment(self):
        segment = self.own_segment()
        with segment.open('ab') as handle:
            handle.write(b'{"v":1,"id":"interrupted')
        self.assertEqual(self.a.show(self.key)['title'], 'Original issue')
        self.assertEqual(self.a.bead('info')['events'], 2)

    def test_bug_torn_tail_followed_by_successful_append_must_stay_readable(self):
        segment = self.own_segment()
        with segment.open('ab') as handle:
            handle.write(b'{"v":1,"id":"interrupted')
        self.assertEqual(self.a.show(self.key)['title'], 'Original issue')
        created = self.a.run('create', 'Acknowledged after interruption', '--json')
        self.assertEqual(created.returncode, 0, created.stderr)
        read = self.a.run('list', '--all', '--json')
        self.assertEqual(read.returncode, 0, 'successful append poisoned the journal: ' + read.stderr)
        self.assertEqual(json.loads(read.stdout)['total'], 2)

    def test_bug_file_size_limit_partial_append_must_not_poison_next_write(self):
        segment = self.own_segment()
        limit = segment.stat().st_size + 100
        def cap_size():
            resource.setrlimit(resource.RLIMIT_FSIZE, (limit, limit))
        proc = subprocess.run([support.BEAD, 'create', 'disk-full simulation', '-d', 'x' * 100000, '--json'],
                              cwd=self.a.checkout, env=self.a.env(), text=True, capture_output=True,
                              preexec_fn=cap_size)
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual(segment.stat().st_size, limit)
        self.assertEqual(self.a.show(self.key)['title'], 'Original issue')
        after = self.a.run('create', 'After full disk cleared', '--json')
        self.assertEqual(after.returncode, 0, after.stderr)
        read = self.a.run('list', '--all', '--json')
        self.assertEqual(read.returncode, 0, 'partial failed write poisons the next accepted append: ' + read.stderr)

    def test_bug_foreign_fork_report_must_not_replace_published_journal(self):
        b = self.two()
        segment = self.own_segment()
        path = str(segment.relative_to(self.a.store()))
        remote_before = self.remote_blob(path)
        foreign = b.store() / path
        rows = [json.loads(row) for row in foreign.read_text().splitlines()]
        rows[-1]['data']['fields']['title'] = 'rewritten foreign history'
        foreign.write_text(''.join(json.dumps(row, separators=(',', ':')) + '\n' for row in rows))
        result = b.run('sync', '--json')
        if result.returncode:
            self.assertEqual(support.code(result), 'JOURNAL_FORK')
        else:
            self.assertEqual(json.loads(result.stdout)['forked_journals'], [path])
        self.assertEqual(self.remote_blob(path), remote_before,
                         'sync reported a foreign fork but then replaced the good remote copy with its local fork')
        self.assertEqual(self.a.run('sync', '--json').returncode, 0)

    def test_bug_copied_machine_id_fork_across_segments_is_detected(self):
        self.two()
        twin = support.Machine(self.sandbox, 'twin')
        shutil.copytree(self.a.home, twin.home)
        # Original starts a new segment, while restored copy still appends the old one.
        self.a.extra['BEAD_ROTATE_BYTES'] = '1'
        twin.extra['BEAD_ROTATE_BYTES'] = '1000000'
        twin.bead('update', self.key, '--title', 'copied identity writer')
        self.a.bead('update', self.key, '--title', 'original writer')
        self.a.bead('sync')
        result = twin.run('sync', '--json')
        self.assertNotEqual(result.returncode, 0,
                            'a shared-machine causal fork split over two segments was accepted without an integrity warning')
        self.assertEqual(support.code(result), 'JOURNAL_FORK')

    def test_bug_segment_boundary_fork_must_not_erase_concurrent_title(self):
        self.two()
        twin = support.Machine(self.sandbox, 'twin')
        shutil.copytree(self.a.home, twin.home)
        self.a.extra['BEAD_ROTATE_BYTES'] = '1'
        twin.extra['BEAD_ROTATE_BYTES'] = '1000000'
        twin.bead('update', self.key, '--title', 'copied identity writer')
        self.a.bead('update', self.key, '--title', 'original writer')
        self.a.bead('sync')
        result = twin.run('sync', '--json')
        if result.returncode:
            self.assertEqual(support.code(result), 'JOURNAL_FORK')
            return
        shown = twin.show(self.key)
        self.assertIn('title', shown.get('conflicts', {}),
                      'concurrent title vanished because segment order invented ancestry: ' + json.dumps(shown))

    def test_bug_kill_mid_append_then_successful_write_must_stay_readable(self):
        marker = self.sandbox.root / 'append-started'
        driver = self.sandbox.root / 'interrupted-append.py'
        driver.write_text("""import builtins, runpy, sys, time
from pathlib import Path
n = runpy.run_path(BEAD_PATH)
import os
real_fdopen = os.fdopen  # segments are opened with os.open + os.fdopen (0600); this is the append boundary
class PartialAppend:
    def __init__(self, handle): self.handle = handle
    def __enter__(self): return self
    def __exit__(self, *exc): return self.handle.__exit__(*exc)
    def write(self, data):
        self.handle.write(data[:100])
        self.handle.flush()
        Path(MARKER_PATH).write_text('partial append reached')
        time.sleep(20)
        return self.handle.write(data[100:]) + 100
    def __getattr__(self, key): return getattr(self.handle, key)
def instrumented_fdopen(fd, mode='r', *args, **kwargs):
    result = real_fdopen(fd, mode, *args, **kwargs)
    return PartialAppend(result) if mode == 'ab' else result
os.fdopen = instrumented_fdopen
sys.exit(n['main']())
""".replace('BEAD_PATH', repr(support.BEAD)).replace('MARKER_PATH', repr(str(marker))))
        worker = subprocess.Popen([sys.executable, str(driver), 'create', 'killed writer', '--json'],
                                  cwd=self.a.checkout, env=self.a.env(), stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            self.wait_marker(marker, worker)
            self.assertEqual(self.a.show(self.key)['title'], 'Original issue')
            os.killpg(worker.pid, signal.SIGKILL)
            worker.communicate(timeout=5)
        finally:
            if worker.poll() is None:
                os.killpg(worker.pid, signal.SIGKILL)
                worker.communicate()
        self.assertEqual(self.a.show(self.key)['title'], 'Original issue')
        later = self.a.run('create', 'accepted after killed writer', '--json')
        self.assertEqual(later.returncode, 0, later.stderr)
        result = self.a.run('sync', '--json')
        self.assertEqual(result.returncode, 0, 'killed append and next accepted write prevent sync: ' + result.stderr)

    def test_slow_push_holds_lock_then_local_writer_completes_without_loss(self):
        self.two()
        marker = self.sandbox.root / 'slow-push'
        self.a.bead('create', 'before slow push')
        sync = subprocess.Popen([support.BEAD, 'sync', '--json'], cwd=self.a.checkout,
                                env=self.a.env(PATH=self.git_wrapper('push', marker)), stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True, start_new_session=True)
        writer = None
        try:
            self.wait_marker(marker, sync)
            writer = subprocess.Popen([support.BEAD, 'create', 'during push', '--json'], cwd=self.a.checkout,
                                      env=self.a.env(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            time.sleep(.3)
            self.assertIsNone(writer.poll(), 'writer should wait for the store lock')
            os.killpg(sync.pid, signal.SIGKILL)
            sync.communicate(timeout=5)
            out, err = writer.communicate(timeout=5)
            self.assertEqual(writer.returncode, 0, err)
        finally:
            if sync.poll() is None:
                os.killpg(sync.pid, signal.SIGKILL)
                sync.communicate()
            if writer is not None and writer.poll() is None:
                writer.kill()
                writer.communicate()
        self.a.bead('sync')
        self.assertEqual(self.a.bead('list')['total'], 3)
        self.assertEqual(self.a.bead('info')['waiting_events'], 0)

    def test_parallel_rotation_preserves_causal_predecessor_and_all_events(self):
        self.a.extra['BEAD_ROTATE_BYTES'] = '600'
        jobs = [f'parallel {i}' for i in range(50)]
        with concurrent.futures.ThreadPoolExecutor(12) as pool:
            results = list(pool.map(lambda title: self.a.run('create', title, '--json'), jobs))
        self.assertTrue(all(p.returncode == 0 for p in results), [p.stderr for p in results if p.returncode])
        segments = sorted((self.a.store() / 'events').glob('*/*.jsonl'))
        self.assertGreater(len(segments), 10)
        events = [json.loads(line) for p in segments for line in p.read_text().splitlines()]
        self.assertEqual(len(events), 52)
        for previous, event in zip(events, events[1:]):
            self.assertIn(previous['id'], event['deps'])
        self.assertEqual(self.a.bead('info')['waiting_events'], 0)
        self.a.bead('sync')
        b = support.Machine(self.sandbox, 'b')
        b.bead('sync')
        self.assertEqual(self.a.export(), b.export())

    def test_deleted_local_segment_is_restored_from_remote(self):
        self.a.extra['BEAD_ROTATE_BYTES'] = '200'
        for i in range(4):
            self.a.bead('create', f'rotated {i}')
        self.a.bead('sync')
        expected = self.a.export()
        paths = sorted((self.a.store() / 'events').glob('*/*.jsonl'))
        paths[1].unlink()
        self.assertGreater(self.a.bead('info')['waiting_events'], 0)
        self.a.bead('sync')
        self.assertEqual(self.a.export(), expected)
        self.assertEqual(self.a.bead('info')['waiting_events'], 0)

    def test_remote_rollback_and_deleted_ref_restore_from_surviving_copy(self):
        self.a.bead('sync')
        old_tip = support.git('--git-dir', str(self.sandbox.origin), 'rev-parse', 'refs/beads/journal').strip()
        self.a.bead('create', 'survives rollback')
        self.a.bead('sync')
        expected = self.a.export()
        support.git('--git-dir', str(self.sandbox.origin), 'update-ref', 'refs/beads/journal', old_tip)
        self.a.bead('sync')
        b = support.Machine(self.sandbox, 'b')
        b.bead('sync')
        self.assertEqual(b.export(), expected)
        support.git('--git-dir', str(self.sandbox.origin), 'update-ref', '-d', 'refs/beads/journal')
        self.a.bead('sync')
        c = support.Machine(self.sandbox, 'c')
        c.bead('sync')
        self.assertEqual(c.export(), expected)

    def test_claim_lost_ack_after_real_push_kill_is_recovered(self):
        b = self.two()
        marker = self.sandbox.root / 'pushed'
        env = self.a.env(PATH=self.git_wrapper('push', marker))
        worker = subprocess.Popen([support.BEAD, 'claim', self.key, '--json'], cwd=self.a.checkout, env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            self.wait_marker(marker, worker)
            os.killpg(worker.pid, signal.SIGKILL)
            worker.communicate(timeout=5)
        finally:
            if worker.poll() is None:
                os.killpg(worker.pid, signal.SIGKILL)
                worker.communicate()
        self.assertNotIn('claim', self.a.show(self.key))
        recovered = self.a.bead('claim', self.key)[0]['claim']
        self.assertEqual(recovered['generation'], 1)
        self.assertEqual([r['op'] for r in self.a.bead('log', self.key)].count('claim'), 1)
        b.bead('sync')
        self.assertEqual(b.show(self.key)['claim'], recovered)

    def test_bug_local_write_after_lost_ack_must_not_fork_accepted_claim(self):
        b = self.two()
        result = self.a.run('claim', self.key, '--json', BEAD_TEST_CRASH_AFTER_PUSH='1')
        self.assertEqual(result.returncode, 99)
        self.assertNotIn('claim', self.a.show(self.key))
        local = self.a.run('comments', 'add', self.key, 'local work after lost acknowledgement', '--json')
        self.assertEqual(local.returncode, 0, local.stderr)
        b.bead('sync')
        accepted = b.show(self.key)['claim']
        recovered = self.a.run('sync', '--json')
        self.assertEqual(recovered.returncode, 0,
                         'an accepted online claim and acknowledged local comment now occupy forked own journals: ' + recovered.stderr)
        self.assertEqual(self.a.show(self.key)['claim'], accepted)
        self.assertEqual(self.a.show(self.key)['comments'][0]['text'], 'local work after lost acknowledgement')

    def test_claim_timeout_after_real_applied_push_is_recovered(self):
        b = self.two()
        marker = self.sandbox.root / 'pushed'
        result = self.a.run('claim', self.key, '--json', PATH=self.git_wrapper('push', marker), BEAD_NET_TIMEOUT='0.4')
        self.assertTrue(marker.exists())
        if result.returncode == 0:
            # The attempt stayed pending after the unanswered push; the next round of the same
            # command fetched the head, found the attempt there and confirmed it.
            recovered = json.loads(result.stdout)[0]['claim']
        else:
            self.assertIn(support.code(result), ('REMOTE_UNREACHABLE', 'UNRESOLVED'))
            self.assertNotIn('claim', self.a.show(self.key))
            recovered = self.a.bead('claim', self.key)[0]['claim']
        self.assertEqual(recovered['generation'], 1)
        self.assertEqual(self.a.show(self.key)['claim'], recovered)
        b.bead('sync')
        self.assertEqual(b.show(self.key)['claim'], recovered)

    def test_killed_slow_fetch_does_not_block_local_writer(self):
        self.two()
        marker = self.sandbox.root / 'fetching'
        worker = subprocess.Popen([support.BEAD, 'sync', '--json'], cwd=self.a.checkout,
                                  env=self.a.env(PATH=self.git_wrapper('fetch', marker, after=False)),
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            self.wait_marker(marker, worker)
            result = subprocess.run([support.BEAD, 'create', 'during stalled fetch', '--json'], cwd=self.a.checkout,
                                    env=self.a.env(), capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            os.killpg(worker.pid, signal.SIGKILL)
            worker.communicate(timeout=5)
        finally:
            if worker.poll() is None:
                os.killpg(worker.pid, signal.SIGKILL)
                worker.communicate()
        self.a.bead('sync')
        self.assertEqual(self.a.bead('list')['total'], 2)

    def test_manifest_missing_and_foreign_workspace_are_refused(self):
        self.a.bead('sync')
        code_tip = support.git('--git-dir', str(self.sandbox.origin), 'rev-parse', 'main').strip()
        support.git('--git-dir', str(self.sandbox.origin), 'update-ref', 'refs/beads/journal', code_tip)
        self.assertEqual(support.code(self.a.run('sync', '--json')), 'MANIFEST_MISSING')
        support.git('--git-dir', str(self.sandbox.origin), 'update-ref', '-d', 'refs/beads/journal')
        self.a.bead('sync')
        self.assertEqual(support.code(self.a.run('sync', '--json', BEAD_WORKSPACE='a-different-workspace')), 'WORKSPACE_MISMATCH')

    def test_retry_exhaustion_retains_local_edits_and_no_partial_claim(self):
        self.two()
        self.a.bead('create', 'unpublished edit')
        # Move the actual local remote forward immediately before each attempted push.
        # The transport and fast-forward rejection remain real; only backoff is suppressed.
        counter = self.sandbox.root / 'rejections'
        folder = self.sandbox.root / 'hostile-git'
        folder.mkdir()
        wrapper = folder / 'git'
        real_git = shutil.which('git')
        wrapper.write_text('#!' + sys.executable + '\n' +
            'import os, subprocess, sys\nfrom pathlib import Path\n' +
            'args = sys.argv[1:]\n' +
            'if args and args[0] == "push":\n' +
            '    base = [' + repr(real_git) + ', "--git-dir", ' + repr(str(self.sandbox.origin)) + ']\n' +
            '    env = dict(os.environ)\n' +
            '    env.update(GIT_AUTHOR_NAME="audit", GIT_AUTHOR_EMAIL="audit@example.invalid", GIT_COMMITTER_NAME="audit", GIT_COMMITTER_EMAIL="audit@example.invalid")\n' +
            '    def git(*words): return subprocess.check_output([*base, *words], env=env, text=True).strip()\n' +
            '    tip = git("rev-parse", "refs/beads/journal")\n' +
            '    tree = git("rev-parse", tip + "^{tree}")\n' +
            '    moved = git("commit-tree", tree, "-p", tip, "-m", "hostile advancement")\n' +
            '    git("update-ref", "refs/beads/journal", moved)\n' +
            '    with Path(' + repr(str(counter)) + ').open("a") as out: out.write("moved\\n")\n' +
            'os.execv(' + repr(real_git) + ', ["git", *args])\n')
        wrapper.chmod(0o755)
        driver = self.sandbox.root / 'fast-retry.py'
        driver.write_text('import runpy, sys\nn = runpy.run_path(' + repr(support.BEAD) + ')\n' +
                          'n["time"].sleep = lambda _: None\nsys.exit(n["main"]())\n')
        before = self.a.export()
        for index, args in enumerate((('claim', self.key), ('sync',)), 1):
            with self.subTest(command=args[0]):
                result = subprocess.run([sys.executable, str(driver), *args, '--json'], cwd=self.a.checkout,
                                        env=self.a.env(PATH=str(folder) + os.pathsep + os.environ['PATH']),
                                        capture_output=True, text=True)
                self.assertEqual(support.code(result), 'SYNC_CONFLICT')
                self.assertEqual(len(counter.read_text().splitlines()), 12 * index)
                self.assertEqual(self.a.export(), before)
                self.assertNotIn('claim', self.a.show(self.key))
        self.a.bead('sync')
        self.assertEqual(self.a.bead('info')['waiting_events'], 0)


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class RootAudit(unittest.TestCase):
    def setUp(self):
        self.sandbox = support.Sandbox()
        self.addCleanup(self.sandbox.cleanup)
        self.a = support.Machine(self.sandbox, "root-a")
        self.a.bead("init", "--prefix", "root")
        self.key = self.a.bead("create", "ownership check")["id"]
        self.a.bead("sync")

    def machine(self, name):
        m = support.Machine(self.sandbox, name)
        m.bead("sync")
        return m

    def settle(self, machines):
        for m in machines * 2:
            report = m.bead("sync")
            self.assertEqual(report["forked_journals"], [])
            self.assertEqual(report["waiting_events"], 0)
        expected = machines[0].show(self.key)
        for m in machines[1:]:
            self.assertEqual(m.show(self.key), expected)
        return expected

    def test_six_simultaneous_claims_have_one_winner(self):
        machines = [self.a] + [self.machine(f"root-{n}") for n in range(5)]
        with concurrent.futures.ThreadPoolExecutor(6) as pool:
            results = list(pool.map(lambda m: m.run("claim", self.key, "--json"), machines))
        self.assertEqual(sorted(r.returncode for r in results), [0,1,1,1,1,1])
        for r in results:
            if r.returncode:
                self.assertEqual(support.code(r), "ALREADY_CLAIMED")
        winner = json.loads(next(r.stdout for r in results if r.returncode == 0))[0]["claim"]
        self.assertEqual(self.settle(machines)["claim"], winner)

    def test_claim_racing_release_matches_accepted_outcome(self):
        b = self.machine("root-b")
        first = self.a.bead("claim", self.key)[0]["claim"]
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            release = pool.submit(self.a.run, "release", self.key, "--json")
            claim = pool.submit(b.run, "claim", self.key, "--json")
            r, c = release.result(), claim.result()
        self.assertEqual(r.returncode, 0, r.stderr)
        issue = self.settle([self.a, b])
        if c.returncode:
            self.assertEqual(support.code(c), "ALREADY_CLAIMED")
            self.assertNotIn("claim", issue)
        else:
            self.assertEqual(issue["claim"]["id"], json.loads(c.stdout)[0]["claim"]["id"])
            self.assertEqual(issue["claim"]["generation"], first["generation"]+1)

    def test_two_takeovers_are_serial_and_generation_increases(self):
        b, c = self.machine("root-b"), self.machine("root-c")
        self.a.bead("claim", self.key)
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            results = list(pool.map(lambda m: m.run("takeover", self.key, "--reason", "synthetic", "--json"), [b,c]))
        self.assertEqual([r.returncode for r in results], [0,0])
        claims = [json.loads(r.stdout)[0]["claim"] for r in results]
        self.assertEqual(sorted(c["generation"] for c in claims), [2,3])
        self.assertEqual(self.settle([self.a,b,c])["claim"], next(c for c in claims if c["generation"]==3))

    def test_takeover_racing_release_ends_at_new_owner(self):
        b = self.machine("root-b")
        self.a.bead("claim", self.key)
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            released = pool.submit(self.a.run, "release", self.key, "--json")
            taken = pool.submit(b.run, "takeover", self.key, "--reason", "synthetic", "--json")
            r,t = released.result(), taken.result()
        self.assertEqual(t.returncode, 0, t.stderr)
        if r.returncode: self.assertEqual(support.code(r), "NOT_OWNER")
        final = self.settle([self.a,b])
        self.assertEqual(final["claim"]["id"], json.loads(t.stdout)[0]["claim"]["id"])
        self.assertEqual(final["claim"]["generation"], 2)

    def test_forced_close_tombstone_and_reopen_generation(self):
        b = self.machine("root-b")
        self.a.bead("claim", self.key)
        b.bead("sync")
        b.bead("close", self.key, "--force")
        self.assertEqual(support.code(self.a.run("claim", self.key, "--json")), "VALIDATION")
        self.assertNotIn("claim", self.a.show(self.key))
        self.a.bead("reopen", self.key)
        self.assertEqual(self.a.bead("claim", self.key)[0]["claim"]["generation"], 2)
        self.a.bead("release", self.key)
        self.a.bead("delete", self.key)
        self.assertEqual(support.code(self.a.run("claim", self.key, "--json")), "VALIDATION")

    def test_provisional_release_and_same_thread_online_upgrade(self):
        self.a.bead("claim", self.key, "--offline")
        self.a.bead("release", self.key, "--offline")
        issue = self.a.show(self.key)
        self.assertEqual(issue["status"], "open")
        self.assertNotIn("provisional_claims", issue)
        self.a.bead("claim", self.key, "--offline")
        issue = self.a.bead("claim", self.key)[0]
        self.assertEqual(issue["claim"]["generation"], 1)
        self.assertNotIn("provisional_claims", issue)

    def test_nonowner_cannot_bypass_close_guard_with_status_update(self):
        self.a.bead("claim", self.key, thread="owner-thread")
        close = self.a.run("close", self.key, "--json", thread="other-thread")
        self.assertEqual(support.code(close), "CLAIMED_BY_OTHER")
        changed = self.a.run("update", self.key, "--status", "closed", "--json", thread="other-thread")
        self.assertNotEqual(changed.returncode, 0, "update --status closed bypassed close ownership check and left claim live")

    def test_claimed_issue_is_not_ready_after_nonowner_status_update(self):
        self.a.bead("claim", self.key, thread="owner-thread")
        self.a.run("update", self.key, "--status", "open", "--json", thread="other-thread")
        self.assertNotIn(self.key, self.a.ready(), "ready offers an issue that still has a confirmed owner")

    def test_clock_moves_back_but_later_observed_edit_settles_register(self):
        self.a.bead("update", self.key, "--priority", "0")
        wrapper = self.sandbox.root / "skew.py"
        wrapper.write_text("import importlib.machinery,importlib.util,sys\n"
                           "loader=importlib.machinery.SourceFileLoader('skewbp',sys.argv.pop(1))\n"
                           "bp=importlib.util.module_from_spec(importlib.util.spec_from_loader(loader.name,loader))\n"
                           "loader.exec_module(bp)\n"
                           "real=bp.time.time_ns\n"
                           "bp.time.time_ns=lambda: real()-86400*1000000000\n"
                           "sys.exit(bp.main())\n")
        p = subprocess.run([sys.executable,str(wrapper),support.BEAD,"update",self.key,"--priority","4","--json"],
                           env=self.a.env(),cwd=self.a.checkout,capture_output=True,text=True)
        self.assertEqual(p.returncode,0,p.stderr)
        issue=self.a.show(self.key)
        self.assertEqual(issue["priority"],4)
        self.assertNotIn("conflicts",issue)


# ---- from test_reaudit.py: second independent audit (REAUDIT), 2026-10-06
class ProtocolReaudit(unittest.TestCase):
    def setUp(self):
        self.sandbox = support.Sandbox()
        self.addCleanup(self.sandbox.cleanup)
        self.a = support.Machine(self.sandbox, 'a')
        self.a.bead('init', '--prefix', 'ex')
        self.key = self.a.bead('create', 'ownership target')['id']
        self.a.bead('sync')
        self.b = support.Machine(self.sandbox, 'b')
        self.b.bead('sync')
        self.mod = support.load_module()
        workspace_id = self.a.store().name
        self.actor = {'agent': 'codex', 'thread': 'a-thread-0001'}
        self.env = mock.patch.dict(os.environ, self.a.env(BEAD_WORKSPACE=workspace_id), clear=True)
        self.env.start()
        self.addCleanup(self.env.stop)
        self.ws = self.mod.Workspace(workspace_id, None, str(self.sandbox.origin), {})

    def interrupted_claim(self, *flags):
        args = self.mod.build_parser().parse_args(['update', self.key, '--claim', *flags]) if flags else self.mod.build_parser().parse_args(['claim', self.key])
        with mock.patch.object(self.mod.Link, 'publish', side_effect=self.mod.BpError('REMOTE_UNREACHABLE', 'fault: before remote accepted')):
            with self.assertRaises(self.mod.BpError):
                args.func(args, self.ws, self.actor)
        self.assertNotIn('claim', self.a.show(self.key))
        return self.pending_ids()

    def pending_ids(self):  # the unsettled attempt: one file per event, ownership and its edits alike
        return [json.loads(line)['id'] for p in sorted((self.a.store() / 'pending').glob('*.json'))
                for line in p.read_text().splitlines() if line.strip()]

    def sync(self):
        return self.mod.synchronise(self.ws, str(self.sandbox.origin), self.actor)

    def settle(self):
        for machine in (self.a, self.b, self.a, self.b):
            machine.bead('sync')
        self.assertEqual(self.a.export(), self.b.export())
        self.assertEqual(self.a.show(self.key), self.b.show(self.key))
        return self.a.show(self.key)

    def test_refused_void_publish_must_not_activate_unaccepted_claim(self):
        self.interrupted_claim()
        self.a.bead('comments', 'add', self.key, 'local work after failed claim')
        original = self.mod.Link.publish
        accepted = []
        def race(link, tip, note='sync', **kwargs):
            if not accepted:
                accepted.append(self.b.bead('claim', self.key)[0]['claim'])
            return original(link, tip, note, **kwargs)
        with mock.patch.object(self.mod.Link, 'publish', race), mock.patch.object(self.mod.time, 'sleep'):
            report = self.sync()
        self.assertGreaterEqual(report['attempts'], 2)
        final = self.settle()
        self.assertEqual(final.get('claim'), accepted[0], 'failed A claim displaced the claim B actually acquired online after void publication lost a CAS race')

    def test_failed_void_publish_retry_must_not_activate_unaccepted_claim(self):
        self.interrupted_claim()
        self.a.bead('comments', 'add', self.key, 'local work after failed claim')
        with mock.patch.object(self.mod.Link, 'publish', side_effect=self.mod.BpError('REMOTE_UNREACHABLE', 'fault: void push unanswered')):
            with self.assertRaises(self.mod.BpError):
                self.sync()
        accepted = self.b.bead('claim', self.key)[0]['claim']
        final = self.settle()
        self.assertEqual(final.get('claim'), accepted, 'retry took back only the void and permanently activated original unaccepted claim')

    def test_failed_void_push_must_leave_original_claim_unconfirmed(self):
        self.interrupted_claim()
        self.a.bead('comments', 'add', self.key, 'local work after failed claim')
        with mock.patch.object(self.mod.Link, 'publish', side_effect=self.mod.BpError('REMOTE_UNREACHABLE', 'fault: void push unanswered')):
            with self.assertRaises(self.mod.BpError):
                self.sync()
        self.assertNotIn('claim', self.a.show(self.key), 'unconfirmed void masks itself but unmasks original claim while remote never accepted either')

    def test_failed_multievent_update_void_retry_must_not_commit_edits(self):
        self.interrupted_claim('--title', 'unaccepted title', '--add-label', 'unaccepted')
        self.a.bead('comments', 'add', self.key, 'independent work')
        with mock.patch.object(self.mod.Link, 'publish', side_effect=self.mod.BpError('REMOTE_UNREACHABLE', 'fault: void push unanswered')):
            with self.assertRaises(self.mod.BpError):
                self.sync()
        final = self.settle()
        self.assertEqual(final['title'], 'ownership target', 'failed atomic update edits revived when retry rolled back their void records')
        self.assertNotIn('unaccepted', final.get('labels', []))
        self.assertNotIn('claim', final)

    def test_quiet_reconcile_of_unaccepted_claim_preserves_independent_work(self):
        self.interrupted_claim()
        self.a.bead('comments', 'add', self.key, 'independent work')
        final = self.settle()
        self.assertNotIn('claim', final)
        self.assertEqual([c['text'] for c in final['comments']], ['independent work'])
        self.assertEqual(self.a.bead('info')['integrity'], [])

    def test_multievent_update_lost_ack_applies_whole_transaction(self):
        crashed = self.a.run('update', self.key, '--claim', '--title', 'accepted title', '--add-label', 'accepted', BEAD_TEST_CRASH_AFTER_PUSH='1')
        self.assertEqual(crashed.returncode, 99)
        before = self.a.show(self.key)
        self.assertEqual(before['title'], 'ownership target')
        self.assertNotIn('claim', before)
        final = self.settle()
        self.assertEqual(final['title'], 'accepted title')
        self.assertEqual(final['labels'], ['accepted'])
        self.assertEqual(final['claim']['thread'], 'a-thread-0001')
        self.assertEqual(self.a.bead('info')['integrity'], [])

    def test_nonzero_applied_push_with_interleaved_writer_must_not_fork(self):
        original = self.mod.run_git
        pushed = []
        written = []
        def lose_ack(args, **kwargs):
            result = original(args, **kwargs)
            if args[0] == 'push' and not pushed and result.returncode == 0:
                pushed.append(True)
                result.returncode = 1
                result.stderr = b'connection closed after remote accepted'
            return result
        def interleaved_writer(_delay):
            if not written:
                written.append(True)
                self.a.bead('comments', 'add', self.key, 'writer acquires lock between retries')
        args = self.mod.build_parser().parse_args(['claim', self.key])
        error = None
        with mock.patch.object(self.mod, 'run_git', lose_ack), mock.patch.object(self.mod.time, 'sleep', interleaved_writer):
            try:
                self.mod.cmd_claim(args, self.ws, self.actor)
            except self.mod.BpError as exc:
                error = exc.code
        self.assertEqual(pushed, [True])
        self.assertEqual(written, [True])
        self.b.bead('sync')
        self.assertIn('claim', self.b.show(self.key))
        self.assertIsNone(error, 'accepted claim was rolled back on ambiguous git failure; local writer now forks the journal: ' + str(error))

    def test_delayed_push_after_timeout_and_absent_fetch_must_not_fork(self):
        """A push the client gave up on may still land later. The claim is pending meanwhile; the
        fences bp pushes to settle it also stall behind the same hook, so when the hook releases,
        one of several commits on the same head wins. Either way the attempt settles and no
        journal forks."""
        started = self.sandbox.root / 'hook-started'
        release = self.sandbox.root / 'hook-release'
        finished = self.sandbox.root / 'hook-finished'
        hook = self.sandbox.origin / 'hooks' / 'pre-receive'
        hook.write_text('#!/bin/sh\n' +
            'touch ' + shlex.quote(str(started)) + '\n' +
            'n=0\nwhile [ ! -f ' + shlex.quote(str(release)) + ' ] && [ "$n" -lt 400 ]; do sleep 0.05; n=$((n+1)); done\n' +
            'touch ' + shlex.quote(str(finished)) + '\nexit 0\n')
        hook.chmod(0o755)
        previous_tip = support.git('--git-dir', str(self.sandbox.origin), 'rev-parse', 'refs/beads/journal').strip()
        try:
            result = self.a.run('claim', self.key, '--json', BEAD_NET_TIMEOUT='0.3')
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn(support.code(result), ('REMOTE_UNREACHABLE', 'UNRESOLVED'))
            self.assertTrue(started.exists(), 'timed out before receive hook')
            self.assertFalse(finished.exists(), 'hook did not remain in flight')
            self.assertEqual(len(self.pending_ids()), 1, 'the unanswered attempt must stay pending')
            self.assertNotIn('claim', self.a.show(self.key))
            stalled = self.a.run('sync', '--json', BEAD_NET_TIMEOUT='0.3')  # the fence stalls behind the hook too
            self.assertEqual(stalled.returncode, 1, stalled.stdout)
            self.assertEqual(support.code(stalled), 'REMOTE_UNREACHABLE')
            self.assertEqual(len(self.pending_ids()), 1, 'an unsettled attempt is never dropped on a guess')
            self.a.bead('comments', 'add', self.key, 'work while the attempt is unsettled')
        finally:
            release.touch()
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                current = support.git('--git-dir', str(self.sandbox.origin), 'rev-parse', 'refs/beads/journal').strip()
                if current != previous_tip:
                    break
                time.sleep(0.02)
        self.assertNotEqual(current, previous_tip, 'fault injection did not produce a delayed successful push')
        time.sleep(0.5)  # let the losing receive-packs finish failing
        recovered = self.a.run('sync', '--json')
        self.assertEqual(recovered.returncode, 0, 'late remote acceptance collided with new local tail: ' + recovered.stderr)
        self.assertEqual(self.pending_ids(), [], 'the attempt is settled one way or the other')
        final = self.settle()
        self.assertEqual([c['text'] for c in final['comments']], ['work while the attempt is unsettled'])
        self.assertEqual(self.a.bead('info')['integrity'], [])
        if 'claim' in final:  # the delayed claim won the race: it is confirmed everywhere
            self.assertEqual(final['claim']['thread'], 'a-thread-0001')
        else:  # a fence won: the claim died in flight and nobody ever saw it
            self.assertEqual(self.a.bead('claims')['claims'], [])
            self.assertEqual(self.b.bead('claims')['claims'], [])

    def test_successful_push_with_nonzero_exit_must_preserve_accepted_claim(self):
        wrapper = self.sandbox.root / 'git-wrapper'
        wrapper.mkdir()
        git_path = subprocess.run(['which', 'git'], capture_output=True, text=True, check=True).stdout.strip()
        # Run a real local push, then inject a lost transport acknowledgement as status 1.
        (wrapper / 'git').write_text('#!/bin/sh\nif [ "$1" = push ]; then\n  "' + git_path + '" "$@" || exit $?\n  exit 1\nfi\nexec "' + git_path + '" "$@"\n')
        (wrapper / 'git').chmod(0o755)
        result = self.a.run('claim', self.key, '--json', PATH=str(wrapper) + os.pathsep + os.environ['PATH'])
        self.b.bead('sync')
        accepted = self.b.show(self.key)['claim']
        self.assertEqual(result.returncode, 0, result.stderr)
        self.a.bead('comments', 'add', self.key, 'work after retry')
        self.assertEqual(self.a.bead('sync')['integrity'], 0)
        self.assertEqual(self.a.show(self.key)['claim'], accepted)


# ---- from test_reaudit.py: second independent audit (REAUDIT), 2026-10-06
class RootReaudit(unittest.TestCase):
    def setUp(self):
        self.box=support.Sandbox();self.addCleanup(self.box.cleanup)
        self.a=support.Machine(self.box,'root2-a')
        self.a.bead('init','--prefix','re')
        self.key=self.a.bead('create','root re-audit')['id']
        self.a.bead('sync')

    def test_importing_legacy_comment_does_not_renumber_existing_local_comment(self):
        self.a.bead('comments','add',self.key,'existing local comment')
        before=next(r for r in map(json.loads,self.a.export().splitlines()) if r['id']==self.key)['comments'][0]['id']
        src=self.box.root/'legacy.jsonl'
        src.write_text(json.dumps({'id':'re-legacy','title':'legacy issue','status':'open','priority':2,'issue_type':'task',
              'created_at':'2026-01-01T00:00:00Z','updated_at':'2026-01-01T00:00:00Z','created_by':'audit',
              'comments':[{'id':41,'issue_id':'re-legacy','author':'audit','text':'legacy','created_at':'2026-01-01T00:00:00Z'}]})+'\n')
        self.a.bead('import',str(src))
        after=next(r for r in map(json.loads,self.a.export().splitlines()) if r['id']==self.key)['comments'][0]['id']
        self.assertEqual(after,before,'single-machine import renumbered an already exported comment')

    def test_forced_close_through_update_publishes_like_forced_close(self):
        self.a.bead('claim',self.key)
        b=support.Machine(self.box,'root2-b');b.bead('sync')
        result=b.bead('update',self.key,'--status','closed','--force')[0]
        self.assertEqual(result['status'],'closed');self.assertNotIn('claim',result)
        self.a.bead('sync')
        self.assertEqual(self.a.show(self.key)['status'],'closed','forced update close was never published')

    def test_claim_update_cycle_validation_is_atomic_for_all_targets(self):
        child=self.a.bead('create','child','--parent',self.key)['id']
        before=self.a.export()
        proc=self.a.run('update',self.key,child,'--claim','--parent',child,'--title','must not persist','--json')
        self.assertEqual(proc.returncode,1)
        self.assertEqual(support.code(proc),'CYCLE')
        self.assertEqual(self.a.export(),before)
        self.assertEqual(self.a.bead('claims')['claims'],[])
        c=support.Machine(self.box,'root2-c');c.bead('sync')
        self.assertEqual(c.bead('claims')['claims'],[])

    def test_failed_claim_does_not_publish_bundled_field_edit(self):
        self.a.bead('claim',self.key)
        b=support.Machine(self.box,'root2-b');b.bead('sync')
        proc=b.run('update',self.key,'--claim','--title','must not persist','--json')
        self.assertEqual(support.code(proc),'ALREADY_CLAIMED')
        b.bead('sync');self.a.bead('sync')
        self.assertEqual(self.a.show(self.key)['title'],'root re-audit')
        self.assertEqual(b.show(self.key)['title'],'root re-audit')


# ---- from test_review3.py: third independent review (REVIEW3), 2026-10-06
class Protocol3(unittest.TestCase):
    def setUp(self):
        self.sandbox=support.Sandbox(); self.addCleanup(self.sandbox.cleanup)
        self.a=support.Machine(self.sandbox,'protocol3-a'); self.a.bead('init','--prefix','ex')
        self.key=self.a.bead('create','before')['id']; self.a.bead('sync')
        self.b=support.Machine(self.sandbox,'protocol3-b'); self.b.bead('sync')
        env=mock.patch.dict(os.environ,self.a.env(BEAD_WORKSPACE=self.a.store().name),clear=True)
        env.start(); self.addCleanup(env.stop)
        self.bp=support.load_module(); self.ws=self.bp.Workspace(self.a.store().name,None,str(self.sandbox.origin),{})
        self.actor={'agent':'codex','thread':'protocol3-a-thread-0001'}

    def pending_ids(self):  # an attempt is one file holding every event that lands with it
        return [e['id'] for p in self.bp.pending_files(self.ws) for e in self.bp.read_attempt(p)]

    def test_fence_wins_against_a_real_push_still_running_after_timeout(self):
        """The first actual receive-pack stalls; subsequent fence commit is accepted first."""
        gate=self.sandbox.root/'release-first-hook'; first=self.sandbox.root/'first-hook'; finished=self.sandbox.root/'hook-finished'
        hook=self.sandbox.origin/'hooks'/'pre-receive'
        hook.write_text('#!/bin/sh\nif mkdir '+shlex.quote(str(first))+' 2>/dev/null; then\n'
            ' n=0\n while [ ! -f '+shlex.quote(str(gate))+' ] && [ "$n" -lt 200 ]; do sleep .05; n=$((n+1)); done\n'
            ' touch '+shlex.quote(str(finished))+'\nfi\nexit 0\n')
        hook.chmod(0o755)
        self.addCleanup(gate.touch)
        link=self.bp.Link(self.ws,str(self.sandbox.origin))
        with self.bp.locked(self.ws):
            tip=link.fetch(); link.merge(tip); tx=self.bp.Tx(self.ws,self.actor)
            self.bp.claim_builder(self.bp.build_parser().parse_args(['claim',self.key]),self.actor,[self.key])(tx,'online')
            self.bp.write_attempt_file(self.ws.dir/'pending'/f"{tx.ownership[0]['id']}.json",tx.ownership)
            with mock.patch.object(self.bp,'NET_TIMEOUT',.25): outcome=link.publish(tip,note='claim',pending=tx.ownership)
        self.assertEqual(outcome,'uncertain'); self.assertTrue(first.exists()); self.assertFalse(finished.exists())
        self.assertEqual(len(self.pending_ids()),1); self.assertNotIn('claim',self.a.show(self.key))
        self.a.bead('comments','add',self.key,'independent local work')
        synced=self.bp.synchronise(self.ws,str(self.sandbox.origin),self.actor)
        fenced=synced['tip']; self.assertEqual(self.pending_ids(),[])
        gate.touch(); deadline=time.monotonic()+10
        while not finished.exists() and time.monotonic()<deadline: time.sleep(.01)
        self.assertTrue(finished.exists()); time.sleep(.2)
        self.assertEqual(support.git('--git-dir',str(self.sandbox.origin),'rev-parse','refs/beads/journal').strip(),fenced)
        self.b.bead('sync'); self.a.bead('sync')
        self.assertNotIn('claim',self.a.show(self.key)); self.assertNotIn('claim',self.b.show(self.key))
        self.assertEqual(self.a.export(),self.b.export())
        self.assertEqual([c['text'] for c in self.a.show(self.key)['comments']],['independent local work'])
        self.assertEqual(self.a.bead('info')['integrity'],[])

    def test_reader_sees_all_or_none_of_accepted_claim_and_bundled_edits(self):
        """A real reader runs around the local confirmation write of an accepted attempt: an attempt is
        one file, so the reader sees nothing of it or all of it."""
        args=self.bp.build_parser().parse_args(['update',self.key,'--claim','--title','after','--add-label','accepted'])
        original=self.bp.write_atomic; observed=[]
        def around_write(path,data):
            if path.parent.name=='claims':
                observed.append(self.a.show(self.key))
                original(path,data)
                observed.append(self.a.show(self.key))
            else:
                original(path,data)
        with mock.patch.object(self.bp,'write_atomic',around_write):
            self.bp.cmd_update(args,self.ws,self.actor)
        self.assertEqual(len(observed),2,'the attempt must be confirmed by exactly one write')
        for current in observed:
            self.assertIn(('claim' in current,current['title'],current.get('labels',[])),
                             [(False,'before',[]),(True,'after',['accepted'])],
                             'accepted atomic update is visible as a confirmed claim with its bundled edits missing')
        self.assertEqual((observed[1]['title'],observed[1]['labels']),('after',['accepted']))

    def test_kill_during_local_confirmation_does_not_leave_partial_atomic_update(self):
        """Remote accepts all files; SIGKILL stops local confirmation after its first file."""
        marker=self.sandbox.root/'confirmation-first-file'
        driver=self.sandbox.root/'pause-confirmation.py'
        # Stop inside the atomic write of the attempt file: the bytes are on disk under a temporary
        # name, the rename into place has not happened.
        driver.write_text("import os,runpy,sys,time\nfrom pathlib import Path\n"
            +"ns=runpy.run_path("+repr(support.BEAD)+")\ng=ns['main'].__globals__\nreal=g['os'].replace\n"
            +"def pause(src,dst):\n if Path(dst).parent.name=='claims':\n  Path("+repr(str(marker))+").touch()\n  while True: time.sleep(.01)\n real(src,dst)\n"
            +"g['os'].replace=pause\nraise SystemExit(ns['main'](sys.argv[1:]))\n")
        process=subprocess.Popen([sys.executable,str(driver),'update',self.key,'--claim','--title','after',
                                  '--add-label','accepted','--json'],cwd=self.a.checkout,env=self.a.env(),
                                  stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        try:
            deadline=time.monotonic()+10
            while not marker.exists() and process.poll() is None and time.monotonic()<deadline: time.sleep(.01)
            self.assertTrue(marker.exists(),'confirmation fault boundary was not reached')
        finally:
            if process.poll() is None: process.kill()
            process.communicate(timeout=10)
        self.assertEqual(process.returncode,-9)
        partial=self.a.show(self.key)
        self.b.bead('sync'); remote=self.b.show(self.key)
        self.a.bead('sync'); recovered=self.a.show(self.key)
        self.assertEqual(recovered['title'],'after'); self.assertEqual(recovered['labels'],['accepted'])
        self.assertEqual(recovered['claim'],remote['claim'])
        self.assertIn(('claim' in partial,partial['title'],partial.get('labels',[])),
                      [(False,'before',[]),(True,'after',['accepted'])],
                      'SIGKILL leaves accepted claim visible while other events of the transaction remain only pending')
        self.assertEqual(self.a.bead('info')['integrity'],[],'a half-written attempt file must never be read as events')

    def test_recovered_atomic_update_is_not_applied_again_after_a_later_observed_edit(self):
        original=self.bp.run_git; applied=[]; written=[]
        def lose_ack(args,**kwargs):
            result=original(args,**kwargs)
            if args[0]=='push' and result.returncode==0 and not applied:
                applied.append(True); result.returncode=1; result.stderr=b'connection closed after remote accepted'
            return result
        def between_retries(_delay):
            if not written:
                written.append(True)
                self.a.bead('sync')
                self.assertEqual(self.a.show(self.key)['title'],'accepted first')
                self.a.bead('update',self.key,'--title','later observed edit')
        args=self.bp.build_parser().parse_args(['update',self.key,'--claim','--title','accepted first'])
        with mock.patch.object(self.bp,'run_git',lose_ack), mock.patch.object(self.bp.time,'sleep',between_retries):
            self.bp.cmd_update(args,self.ws,self.actor)
        self.assertEqual(applied,[True]); self.assertEqual(written,[True])
        self.b.bead('sync'); self.a.bead('sync')
        self.assertEqual(self.a.show(self.key)['title'],'later observed edit',
                         'acknowledgement recovery executes the already accepted bundled edit again, overwriting later work that had seen it')

    def test_release_recovers_a_successful_push_with_lost_acknowledgement(self):
        self.a.bead('claim',self.key)
        original=self.bp.run_git; applied=[]
        def lose_ack(args,**kwargs):
            result=original(args,**kwargs)
            if args[0]=='push' and result.returncode==0 and not applied:
                applied.append(True); result.returncode=1; result.stderr=b'connection closed after remote accepted'
            return result
        args=self.bp.build_parser().parse_args(['release',self.key]); failure=None
        with mock.patch.object(self.bp,'run_git',lose_ack), mock.patch.object(self.bp.time,'sleep'):
            try: self.bp.cmd_release(args,self.ws,self.actor)
            except self.bp.BpError as e: failure=e.code
        self.assertEqual(applied,[True]); self.b.bead('sync')
        self.assertNotIn('claim',self.b.show(self.key)); self.assertNotIn('claim',self.a.show(self.key))
        self.assertEqual(self.pending_ids(),[])
        self.assertIsNone(failure,'accepted release recovered from remote but command reports '+str(failure))

    def test_mixed_ownership_file_and_journal_ancestry_across_six_machines(self):
        import random, uuid
        events=[]; previous=None; active=None; uid=None
        for index in range(2000):
            eid=str(uuid.UUID(int=((1_770_000_000_000+index)<<80)|(7<<76)|(2<<62)))
            replica='machine'+str(index%6)
            if index==0: op,data,issue='init',{'prefix':'ex'},eid
            elif index==1: op,data,issue='create',{'key':'ex-mixed','fields':{'title':'mixed'}},eid; uid=eid
            elif index%10==2: op,data,issue='claim',{'mode':'online','generation':index//10+1},uid; active=eid
            elif index%10==7: op,data,issue='release',{'claim':active},uid
            else: op,data,issue='comment',{'text':'ordinary journal event'},uid
            event={'v':1,'id':eid,'replica':replica,'actor':{'agent':'audit','thread':'one-owner'},
                   'deps':[previous] if previous else [],'op':op,'issue':issue,'data':data}
            events.append(event); previous=eid
        rng=random.Random(3106); expected=None
        for _ in range(6):
            journal=self.bp.Journal(); replicas=['machine'+str(i) for i in range(6)]; rng.shuffle(replicas)
            for replica in replicas:
                for e in events:
                    if e['replica']==replica: journal.add(e,replica,sequenced=e['op'] not in ('claim','release'))
            state=self.bp.replay(journal)
            self.assertEqual(state.integrity,[]); self.assertEqual(state.pending,[])
            for _ in range(1000):
                first,second=rng.sample(range(len(events)),2)
                self.assertEqual(state.saw(events[first]['id'],events[second]['id']),first>second)
            snapshot=json.dumps(self.bp.detail(state,state.issues[uid]),sort_keys=True)
            if expected is None: expected=snapshot
            else: self.assertEqual(snapshot,expected)

    def test_rejected_fences_leave_attempt_pending_and_excluded(self):
        crash=self.a.run('claim',self.key,BEAD_TEST_CRASH_BEFORE_PUSH='1')
        self.assertEqual(crash.returncode,98); before=self.pending_ids(); self.assertEqual(len(before),1)
        hook=self.sandbox.origin/'hooks'/'pre-receive'; hook.write_text('#!/bin/sh\nexit 1\n'); hook.chmod(0o755)
        with mock.patch.object(self.bp.time,'sleep'):
            with self.assertRaises(self.bp.BpError) as caught: self.bp.synchronise(self.ws,str(self.sandbox.origin),self.actor)
        self.assertEqual(caught.exception.code,'SYNC_CONFLICT'); self.assertEqual(self.pending_ids(),before)
        self.assertNotIn('claim',self.a.show(self.key)); self.assertEqual(self.a.show(self.key)['title'],'before')
        hook.unlink(); self.a.bead('sync'); self.assertEqual(self.pending_ids(),[])
        self.assertNotIn('claim',self.a.show(self.key))


# ---- from test_review3.py: third independent review (REVIEW3), 2026-10-06
class ActualTransportReview(unittest.TestCase):
    def test_timed_out_accepted_release_returns_success(self):
        """Actual receive-pack timeout: a remotely accepted release must not be rebuilt."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "timeout-a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "release target")["id"]
        a.bead("sync")
        b = support.Machine(box, "timeout-b")
        b.bead("sync")
        a.bead("claim", key)

        first = box.root / "first-release-push"
        finished = box.root / "first-release-finished"
        hook = box.origin / "hooks" / "pre-receive"
        hook.write_text(
            "#!/bin/sh\n"
            + f'if mkdir "{first}" 2>/dev/null; then\n'
            + "  sleep 0.15\n"
            + f'  touch "{finished}"\n'
            + "fi\nexit 0\n"
        )
        hook.chmod(0o755)

        result = a.run("release", key, "--json", BEAD_NET_TIMEOUT="0.1")
        deadline = time.monotonic() + 10
        while not finished.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        self.assertTrue(finished.exists(), "actual receive-pack did not finish")
        b.bead("sync")
        self.assertNotIn("claim", a.show(key))
        self.assertNotIn("claim", b.show(key))
        self.assertEqual(list((a.store() / "pending").glob("*.json")), [])
        self.assertEqual(result.returncode, 0,
                         "remote accepted the release, but recovery rebuilt it: " + result.stderr)

    def test_crash_after_accepted_release_can_be_retried_idempotently(self):
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "crash-a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "release target")["id"]
        a.bead("sync")
        b = support.Machine(box, "crash-b")
        b.bead("sync")
        a.bead("claim", key)

        crashed = a.run("release", key, "--json", BEAD_TEST_CRASH_AFTER_PUSH="1")
        self.assertEqual(crashed.returncode, 99)
        retry = a.run("release", key, "--json")
        b.bead("sync")
        self.assertNotIn("claim", a.show(key))
        self.assertNotIn("claim", b.show(key))
        self.assertEqual(list((a.store() / "pending").glob("*.json")), [])
        self.assertEqual(retry.returncode, 0,
                         "accepted release was confirmed, but a command retry reports: " + retry.stderr)


# ---- from test_review4.py: fourth independent review (2026-10-07-4-codex), folded in after the fixes
def remote_blob(remote: Path, revision_path: str) -> bytes:
    return subprocess.run(
        ["git", "--git-dir", str(remote), "show", revision_path],
        check=True,
        capture_output=True,
    ).stdout


# ---- from test_review4.py: fourth independent review (2026-10-07-4-codex), folded in after the fixes
class ProtocolReview4(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_q01_damaged_immutable_attempt_cannot_replace_remote_transaction(self):
        """Q01: a damaged foreign attempt must be refused or repaired, never republished."""
        a, b, c = (support.Machine(self.box, name) for name in ("q01-a", "q01-b", "q01-c"))
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "before")["id"]
        a.bead("sync")
        b.bead("sync")
        a.bead("update", key, "--claim", "--title", "accepted", "--add-label", "kept")
        b.bead("sync")

        attempts = list((b.store() / "events").glob("*/claims/*.json"))
        attempt = next(path for path in attempts if len(path.read_text().splitlines()) == 3)
        rel = attempt.relative_to(b.store()).as_posix()
        accepted = remote_blob(self.box.origin, f"refs/beads/journal:{rel}")
        attempt.write_text(attempt.read_text().splitlines()[0] + "\n")

        # A repair may succeed or a fork check may refuse.  In either case the
        # authoritative remote transaction must remain complete for a fresh peer.
        b.run("sync", "--json")
        c.bead("sync")
        shown = c.show(key)
        self.assertEqual(shown["title"], "accepted")
        self.assertEqual(shown.get("labels"), ["kept"])
        self.assertEqual(remote_blob(self.box.origin, f"refs/beads/journal:{rel}"), accepted)

    def test_q01_malformed_attempt_is_inert_as_a_whole(self):
        """Q01: one malformed line must make the whole ownership attempt inert."""
        a = support.Machine(self.box, "q01-malformed")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "survivor")["id"]
        records = [
            json.loads(line)
            for path in (a.store() / "events").glob("*/*.jsonl")
            for line in path.read_text().splitlines()
        ]
        created = next(event for event in records if event["op"] == "create")
        claim_id = "019a4444-0001-7000-8000-000000000001"
        claim = {
            "v": 1,
            "id": claim_id,
            "replica": "corrupt",
            "deps": [created["id"]],
            "actor": {"agent": "test", "thread": "corrupt-thread"},
            "op": "claim",
            "issue": created["id"],
            "data": {"mode": "online", "generation": 1},
        }
        path = a.store() / "events" / "corrupt" / "claims" / f"{claim_id}.json"
        path.parent.mkdir(parents=True)
        path.write_bytes(json.dumps(claim, separators=(",", ":")).encode() + b"\n{")

        shown = a.show(key)
        self.assertNotIn("claim", shown, "a valid prefix of a malformed attempt became confirmed")
        self.assertTrue(a.bead("info")["integrity"])

    def test_q02_retry_after_accepted_claiming_parent_move_keeps_parent(self):
        """Q02: retrying an accepted claiming parent move must not remove the new parent."""
        a = support.Machine(self.box, "q02-a")
        a.bead("init", "--prefix", "ex")
        first = a.bead("create", "first parent")["id"]
        second = a.bead("create", "second parent")["id"]
        child = a.bead("create", "child", "--parent", first)["id"]
        a.bead("sync")
        peer = support.Machine(self.box, "q02-peer")
        peer.bead("sync")

        crashed = a.run(
            "update", child, "--claim", "--parent", second, "--json",
            BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        self.assertEqual(crashed.returncode, 99)
        self.assertEqual(a.show(child)["parent"], first)
        retry = a.run("update", child, "--claim", "--parent", second, "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(a.show(child).get("parent"), second)
        peer.bead("sync")
        self.assertEqual(peer.show(child).get("parent"), second)

    def test_q02_retry_after_accepted_claiming_close_emits_one_close(self):
        """Q02: recovery must not append a second close with new time/provenance."""
        a = support.Machine(self.box, "q02-close")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "close once")["id"]
        a.bead("sync")
        crashed = a.run(
            "update", key, "--claim", "--status", "closed", "--json",
            BEAD_TEST_CRASH_AFTER_PUSH="1",
        )
        self.assertEqual(crashed.returncode, 99)
        retry = a.run("update", key, "--claim", "--status", "closed", "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        closes = [event for event in a.bead("log", key, "--limit", "0") if event["op"] == "close"]
        self.assertEqual(len(closes), 1, "the recovered command appended a second close")

    def test_q03_crash_after_confirmation_before_cleanup_is_idempotent(self):
        """Q03: a final attempt plus its pending twin must be recognized as accepted."""
        a = support.Machine(self.box, "q03-a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "release once")["id"]
        a.bead("claim", key)

        with mock.patch.dict(os.environ, a.env(), clear=True), contextlib.chdir(a.checkout):
            bp = support.load_module()
            ws = bp.find_workspace()
            actor = bp.detect_actor(None)
            args = bp.build_parser().parse_args(["release", key])
            with mock.patch.object(bp, "drop_pending", side_effect=RuntimeError("after confirm")):
                with self.assertRaisesRegex(RuntimeError, "after confirm"):
                    bp.cmd_release(args, ws, actor)
            self.assertEqual(len(bp.pending_files(ws)), 1)
            self.assertEqual(len(bp.claim_files(ws)), 2)

        self.assertEqual(a.show(key)["status"], "open")
        retry = a.run("release", key, "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(a.show(key)["status"], "open")
        self.assertEqual(list((a.store() / "pending").glob("*.json")), [])
        releases = [event for event in a.bead("log", key, "--limit", "0") if event["op"] == "release"]
        self.assertEqual(len(releases), 1, "recovery retained or appended a second release")

    def test_q04_future_manifest_is_refused_without_remote_rewrite(self):
        """Q04: an older client must not replace a future manifest/tree at the remote tip."""
        a = support.Machine(self.box, "q04-a")
        a.bead("init", "--prefix", "ex")
        a.bead("create", "published")
        a.bead("sync")

        attacker = self.box.clone("q04-attacker")
        support.git(
            "fetch", "-q", "origin",
            "refs/beads/journal:refs/remotes/origin/beads-journal",
            cwd=attacker,
        )
        support.git("checkout", "-q", "--detach", "refs/remotes/origin/beads-journal", cwd=attacker)
        (attacker / "manifest.json").write_text(
            json.dumps({"format": 2, "workspace": a.store().name}) + "\n"
        )
        (attacker / "future-state.bin").write_text("future state must survive\n")
        support.git("add", "-A", cwd=attacker)
        support.git(
            "-c", "user.name=review", "-c", "user.email=review@example.invalid",
            "commit", "-q", "-m", "future tracker format", cwd=attacker,
        )
        support.git("push", "-q", "--force", "origin", "HEAD:refs/beads/journal", cwd=attacker)
        future_tip = support.git(
            "--git-dir", str(self.box.origin), "rev-parse", "refs/beads/journal"
        ).strip()

        fresh = support.Machine(self.box, "q04-fresh")
        result = fresh.run("sync", "--json")
        after = support.git(
            "--git-dir", str(self.box.origin), "rev-parse", "refs/beads/journal"
        ).strip()
        names = support.git(
            "--git-dir", str(self.box.origin), "ls-tree", "-r", "--name-only", after
        ).splitlines()
        manifest = json.loads(
            support.git("--git-dir", str(self.box.origin), "show", f"{after}:manifest.json")
        )
        self.assertNotEqual(result.returncode, 0, "future format was accepted")
        self.assertEqual(after, future_tip, "sync rewrote the future-format head")
        self.assertEqual(manifest["format"], 2)
        self.assertIn("future-state.bin", names)


if __name__ == "__main__":
    unittest.main()
