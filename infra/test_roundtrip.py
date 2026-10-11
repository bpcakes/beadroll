"""infra/roundtrip.py must fail when a selected check fails, using synthetic results only: its own
tests, beside it (they left tests/test_check.py; ./check.sh and cargo xtask check run them)."""
import contextlib
import io
import json
import os
import runpy
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parent.parent
# roundtrip.py imports selected from beside it, which `python3 infra/roundtrip.py` finds there and
# runpy.run_path does not, so this puts infra/ where a run of the script would have it
sys.path.insert(0, str(ROOT / "infra"))


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
        # the bead under test: a native build as the preflight reads it, never run (subprocess.run is replaced)
        self.bead = self.root / "bead"
        self.bead.write_bytes(b"\x7fELF stand-in\n")

    def roundtrip(self, failed_command=None, changed=False, version="bead 0.2.0", expect=""):
        def files(pattern):
            return [str(self.source)] if pattern.endswith("*/.beads/issues.jsonl") else []

        def bead(args, **kwargs):
            self.commands.append(args)
            if args[1:] == ["--version"]:
                return subprocess.CompletedProcess(args, 0, version + "\n", "")
            if args[1] == failed_command:
                return subprocess.CompletedProcess(args, 17, "", "synthetic command failure")
            record = {**self.record, "title": "Changed"} if changed else self.record
            output = json.dumps(record) + "\n" if args[1] == "export" else "{}"
            return subprocess.CompletedProcess(args, 0, output, "")

        output = io.StringIO()
        environ = {"RT_HOME": str(self.root / "store"), "BEAD_UNDER_TEST": str(self.bead), "BEAD_EXPECT_VERSION": expect}
        with mock.patch.dict(os.environ, environ), \
                mock.patch("glob.glob", side_effect=files), \
                mock.patch("subprocess.run", side_effect=bead), contextlib.redirect_stdout(output):
            if not expect:
                del os.environ["BEAD_EXPECT_VERSION"]
            try:
                runpy.run_path(str(ROOT / "infra" / "roundtrip.py"), run_name="__main__")
            except SystemExit as error:
                return error.code, output.getvalue()
        return 0, output.getvalue()

    def test_identical_records_succeed(self):
        status, output = self.roundtrip()
        self.assertEqual(status, 0, output)
        self.assertIn("TOTAL 1/1 records identical", output)
        self.assertEqual(self.commands[0], [str(self.bead), "--version"])
        self.assertIn("--offline", self.commands[1])
        record = json.loads(output.split("preflight: ", 1)[1].splitlines()[0])
        self.assertEqual((record["path"], record["version"], record["kind"]), (str(self.bead), "bead 0.2.0", "native"))

    def test_the_bead_is_checked_before_any_data_is_written(self):
        status, output = self.roundtrip(version="bead 0.1.0", expect="0.2.0")
        self.assertNotEqual(status, 0, output)
        self.assertIn("reports 'bead 0.1.0', not 'bead 0.2.0'", status)
        self.assertEqual(self.commands, [[str(self.bead), "--version"]])
        status, output = self.roundtrip(expect="0.2.0")
        self.assertEqual(status, 0, output)

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


class StandaloneRun(unittest.TestCase):
    def test_the_tests_run_alone_by_their_dotted_name(self):
        # a fresh interpreter with only the repository root to import from, as `python3 -m unittest
        # infra.test_roundtrip` starts: roundtrip.py's own import of selected must still find it
        proc = subprocess.run([sys.executable, "-m", "unittest", "infra.test_roundtrip.RoundtripFailureReporting"],
                              cwd=ROOT, capture_output=True, text=True, timeout=120)
        self.assertEqual(proc.returncode, 0, proc.stderr[-2000:])
