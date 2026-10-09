"""The verification commands must fail when a selected check fails, using synthetic results only."""
import contextlib
import io
import json
import os
import runpy
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tests import test_sweep as sweep


ROOT = Path(__file__).resolve().parent.parent


class CheckScript(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bead-check-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        shutil.copy2(ROOT / "check.sh", self.root / "check.sh")
        (self.root / "bin").mkdir()
        python = self.root / "bin" / "python3"
        python.write_text('''#!/bin/sh
if [ "$1" = "-m" ]; then
    echo "Ran 1 test in 0.001s"
    if [ "$CHECK_TEST_MODULE_STATUS" = "0" ]; then echo OK; else echo FAILED; fi
    exit "$CHECK_TEST_MODULE_STATUS"
fi
echo "roundtrip details"
echo "roundtrip summary"
if [ "$CHECK_TEST_REAL_STATUS" != "0" ]; then echo "roundtrip diagnostic" >&2; fi
exit "$CHECK_TEST_REAL_STATUS"
''')
        python.chmod(0o755)

    def check(self, module_status=0, real_status=0):
        env = dict(os.environ, PATH=str(self.root / "bin") + os.pathsep + os.environ["PATH"],
                   BEAD_HOME=str(self.root / "home"), CHECK_TEST_MODULE_STATUS=str(module_status),
                   CHECK_TEST_REAL_STATUS=str(real_status))
        return subprocess.run([str(self.root / "check.sh"), "--real"], env=env,
                              capture_output=True, text=True, timeout=10)

    def test_success_keeps_the_roundtrip_summary(self):
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[-1], "roundtrip summary")
        self.assertNotIn("roundtrip details", result.stdout)

    def test_roundtrip_failure_fails_the_check_and_keeps_diagnostics(self):
        result = self.check(real_status=17)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("roundtrip diagnostic", result.stderr)
        self.assertIn("roundtrip details", result.stdout)

    def test_successful_roundtrip_does_not_hide_a_failed_module(self):
        result = self.check(module_status=19)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("roundtrip summary", result.stdout)


class SweepFailureReporting(unittest.TestCase):
    def run_sweep(self, schedule, ok, mode):
        op, fault, pair = schedule
        report = dict(op=op, fault=fault, pair=pair, ok=ok,
                      violations=[] if ok else ["injected failure"])
        with mock.patch.dict(os.environ, BEAD_SWEEP=mode, BEAD_SWEEP_GC="0", BEAD_SWEEP_CONCURRENT="0"), \
                mock.patch.object(sweep, "schedules", return_value=[schedule]), \
                mock.patch.object(sweep, "SLICE", [schedule]), \
                mock.patch.object(sweep, "run_schedule", return_value=report):
            result = unittest.TestResult()
            sweep.Sweep("test_sweep").run(result)
        return result

    def test_every_failed_schedule_fails_the_sweep(self):
        # The ten G12 cases used to be accepted even after the underlying bug was fixed.
        schedules = [(op, "crash-before-push", pair) for op in ("close", "delete")
                     for pair in ("RD", "RO", "RS", "OR", "SR")]
        schedules.append(("claim", "crash-after-push", "RO"))
        for mode in ("", "full"):
            for schedule in schedules:
                with self.subTest(mode=mode, schedule=schedule):
                    result = self.run_sweep(schedule, False, mode)
                    self.assertEqual(result.errors, [])
                    self.assertEqual(len(result.failures), 1)
                    self.assertIn("injected failure", result.failures[0][1])

    def test_successful_schedules_pass(self):
        for mode in ("", "full"):
            with self.subTest(mode=mode):
                result = self.run_sweep(("close", "crash-before-push", "RS"), True, mode)
                self.assertTrue(result.wasSuccessful(), (result.errors, result.failures))

    def test_default_slice_retries_crashed_owner_close_and_delete(self):
        for op in ("close", "delete"):
            with self.subTest(op=op):
                self.assertTrue(any(verb == op and fault == "crash-before-push"
                                    and pair in ("RD", "RO", "RS", "OR", "SR")
                                    for verb, fault, pair in sweep.SLICE))


class RoundtripFailureReporting(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bead-roundtrip-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "tracker" / ".beads" / "issues.jsonl"
        self.source.parent.mkdir(parents=True)
        (self.source.parent.parent / ".git").mkdir()
        self.record = {"id": "x-a", "title": "Example", "status": "open"}
        self.source.write_text(json.dumps(self.record) + "\n")
        self.commands = []

    def roundtrip(self, failed_command=None, changed=False):
        def files(pattern):
            return [str(self.source)] if pattern.endswith("*/.beads/issues.jsonl") else []

        def bead(args, **kwargs):
            self.commands.append(args)
            if args[1] == failed_command:
                return subprocess.CompletedProcess(args, 17, "", "synthetic command failure")
            record = {**self.record, "title": "Changed"} if changed else self.record
            output = json.dumps(record) + "\n" if args[1] == "export" else "{}"
            return subprocess.CompletedProcess(args, 0, output, "")

        output = io.StringIO()
        with mock.patch.dict(os.environ, RT_HOME=str(self.root / "store")), \
                mock.patch("glob.glob", side_effect=files), \
                mock.patch("subprocess.run", side_effect=bead), contextlib.redirect_stdout(output):
            try:
                runpy.run_path(str(ROOT / "infra" / "roundtrip.py"), run_name="__main__")
            except SystemExit as error:
                return error.code, output.getvalue()
        return 0, output.getvalue()

    def test_identical_records_succeed(self):
        status, output = self.roundtrip()
        self.assertEqual(status, 0, output)
        self.assertIn("TOTAL 1/1 records identical", output)
        self.assertIn("--offline", self.commands[0])

    def test_failed_commands_fail_the_roundtrip(self):
        for command in ("init", "export", "ready"):
            with self.subTest(command=command):
                status, output = self.roundtrip(failed_command=command)
                self.assertNotEqual(status, 0, output)
                self.assertIn("synthetic command failure", output)

    def test_changed_records_fail_the_roundtrip(self):
        status, output = self.roundtrip(changed=True)
        self.assertNotEqual(status, 0, output)
        self.assertIn("TOTAL 0/1 records identical", output)
        self.assertIn("DIFF", output)
