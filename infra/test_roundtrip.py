"""infra/roundtrip.py must fail when a selected check fails, using synthetic results only: its own
tests, beside it (they left tests/test_check.py; ./check.sh and cargo xtask check run them)."""
import contextlib
import io
import json
import os
import runpy
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parent.parent


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
