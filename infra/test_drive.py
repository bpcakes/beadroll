"""infra/drive.py and infra/selected.py choose each machine's bead explicitly and record it before any test
data is written (docs/RUST-PORT.md §8): their own tests, beside them. drive.py runs with `--preflight`
against stand-ins: an `ssh` first on PATH that runs the remote command here, and beads that log each run."""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "infra"))
import selected  # noqa: E402  (the module under test, beside this file)


class DrivePreflight(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bead-drive-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.log = self.root / "runs.log"
        shims = self.root / "shims"
        shims.mkdir()
        # ssh -o BatchMode=yes TARGET COMMAND: the command runs here
        self.executable(shims / "ssh", '#!/bin/sh\nshift 3\nexec /bin/sh -c "$1"\n')
        self.path = f"{shims}{os.pathsep}{os.environ.get('PATH', '')}"
        self.home = self.root / "home"
        (self.home / ".local" / "bin").mkdir(parents=True)

    def executable(self, path, text):
        path.write_text(text)
        path.chmod(0o755)
        return path

    def bead(self, name, version, script=False):
        """A stand-in bead that prints `version` and logs every run; with no `#!` line it is what the probe
        takes for a native build (bash runs it all the same)."""
        body = f'echo "$0 $*" >> {self.log}\necho "{version}"\n'
        return self.executable(self.root / name, ("#!/bin/sh\n" if script else "") + body)

    def drive(self, beads=None, expect="0.2.0", local=None, *args):
        env = {key: value for key, value in os.environ.items() if not key.startswith(("BEAD_", "GIT_"))}
        env.update(PATH=self.path, HOME=str(self.home), BEAD_TEST_REMOTE="git@example.invalid:throwaway.git",
                   BEAD_DRIVE_HOSTS="one=u@one.invalid,two=two.invalid,three=u@three.invalid",
                   BEAD_UNDER_TEST=str(local or self.bead("local-bead", "bead 0.2.0")))
        if beads is not None:
            env["BEAD_DRIVE_BEADS"] = beads
        if expect:
            env["BEAD_EXPECT_VERSION"] = expect
        return subprocess.run([sys.executable, str(ROOT / "infra" / "drive.py"), "--preflight", *args], env=env,
                              capture_output=True, text=True, timeout=120)

    def records(self, proc):
        rows = [json.loads(line.split(" ", 1)[1]) for line in proc.stdout.splitlines() if line.startswith("preflight: ")]
        return {row["role"]: row for row in rows}

    def runs(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_every_machine_is_recorded_and_nothing_else_runs(self):
        beads = {role: self.bead(f"{role}-bead", "bead 0.2.0") for role in ("one", "two", "three")}
        proc = self.drive(",".join(f"{role}={path}" for role, path in beads.items()))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        rows = self.records(proc)
        self.assertEqual(set(rows), {"local", "one", "two", "three"})
        for role, path in beads.items():
            self.assertEqual(rows[role]["path"], str(path))
        for row in rows.values():
            self.assertEqual((row["version"], row["kind"]), ("bead 0.2.0", "native"), row)
            self.assertRegex(row["sha256"], "^[0-9a-f]{16}$")
            self.assertTrue(row["system"], row)
        self.assertEqual(len(self.runs()), 4, self.runs())
        self.assertTrue(all(run.endswith(" --version") for run in self.runs()), self.runs())

    def test_a_machine_that_still_runs_python_stops_the_run(self):
        python = self.bead("python-bead", "bead 0.1.0", script=True)
        rust = self.bead("rust-bead", "bead 0.2.0")
        proc = self.drive(f"one={rust},two={python},three={rust}")
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("two: ", proc.stderr)
        self.assertIn("'bead 0.1.0'", proc.stderr)
        self.assertEqual(self.records(proc)["two"]["kind"], "script")
        self.assertTrue(all(run.endswith(" --version") for run in self.runs()), self.runs())
        # a script that claims the version is still not a native build
        claimed = self.bead("claimed-bead", "bead 0.2.0", script=True)
        proc = self.drive(f"one={rust},two={rust},three={rust}", "0.2.0", claimed)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("local: ", proc.stderr)
        self.assertIn("is a script, not a native build", proc.stderr)

    def test_without_an_expected_version_the_preflight_only_records(self):
        python = self.bead("python-bead", "bead 0.1.0", script=True)
        proc = self.drive(f"one={python}", "", python)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(self.records(proc)["one"]["version"], "bead 0.1.0")

    def test_a_remote_host_runs_its_installed_bead_unless_told_otherwise(self):
        installed = self.executable(self.home / ".local" / "bin" / "bead", 'echo "bead 0.2.0"\n')
        rust = self.bead("rust-bead", "bead 0.2.0")
        proc = self.drive(f"three={rust}")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        rows = self.records(proc)
        self.assertEqual(rows["one"]["path"], str(installed))
        self.assertEqual(rows["two"]["path"], str(installed))
        self.assertEqual(rows["three"]["path"], str(rust))
        installed.unlink()
        proc = self.drive(f"three={rust}")
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual(self.records(proc)["one"]["kind"], "missing")
        self.assertIn("does not exist", proc.stderr)

    def test_every_bead_is_named_by_an_absolute_path(self):
        rust = self.bead("rust-bead", "bead 0.2.0")
        for beads, local, said in [
            ("one=relative/bead", None, "BEAD_DRIVE_BEADS gives one 'relative/bead', which is not an absolute path"),
            (f"four={rust}", None, "BEAD_DRIVE_BEADS names 'four', which is not one of one, three, two"),
            ("", "relative/bead", "BEAD_UNDER_TEST must be an absolute path, not 'relative/bead'"),
        ]:
            with self.subTest(beads=beads, local=local):
                proc = self.drive(beads, "0.2.0", local)
                self.assertNotEqual(proc.returncode, 0)
                self.assertIn(said, proc.stderr)
                self.assertNotIn("preflight: {", proc.stdout)
        self.assertEqual(self.runs(), [])


class LocalSelection(unittest.TestCase):
    def test_the_local_bead_is_the_one_named_else_the_checkouts_else_the_installed_one(self):
        self.assertEqual(selected.local_bead({"BEAD_UNDER_TEST": "/opt/bead"}), "/opt/bead")
        with self.assertRaises(SystemExit):
            selected.local_bead({"BEAD_UNDER_TEST": "target/debug/bead"})
        with tempfile.TemporaryDirectory(prefix="bead-select-test-") as tree:
            with mock.patch.object(selected, "SOURCE_TREE", tree):
                self.assertEqual(selected.local_bead({}), os.path.expanduser("~/.local/bin/bead"))
                Path(tree, "bead").write_text("#!/bin/sh\n")
                self.assertEqual(selected.local_bead({}), os.path.join(tree, "bead"))

    def test_the_verdict_wants_the_version_from_a_native_build(self):
        row = {"path": "/b", "version": "bead 0.2.0", "kind": "native", "sha256": "0" * 16}
        self.assertIsNone(selected.verdict(row, "0.2.0"))
        self.assertIsNone(selected.verdict({**row, "kind": "script"}, ""))
        self.assertIn("not 'bead 0.2.0'", selected.verdict({**row, "version": "bead 0.1.0"}, "0.2.0"))
        self.assertIn("not a native build", selected.verdict({**row, "kind": "script"}, "0.2.0"))
        self.assertIn("does not exist", selected.verdict({**row, "kind": "missing"}, "0.2.0"))


class SoakPreflight(unittest.TestCase):
    def soak(self, bead, expect):
        with tempfile.TemporaryDirectory(prefix="bead-soak-test-") as tmp:
            log = Path(tmp) / "soak.log"
            env = {key: value for key, value in os.environ.items() if not key.startswith("BEAD_")}
            if expect:
                env["BEAD_EXPECT_VERSION"] = expect
            proc = subprocess.run([sys.executable, str(ROOT / "infra" / "soak.py"), str(bead), tmp, "0", "1", str(log)],
                                  env=env, capture_output=True, text=True, timeout=120)
            return proc, [json.loads(line) for line in log.read_text().splitlines()]

    def test_the_soak_records_its_bead_first_and_refuses_another(self):
        with tempfile.TemporaryDirectory(prefix="bead-soak-bead-") as tmp:
            python = Path(tmp) / "bead"
            python.write_text('#!/bin/sh\nif [ "$1" = --version ]; then echo "bead 0.1.0"; else echo "{}"; fi\n')
            python.chmod(0o755)
            proc, rows = self.soak(python, "")
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(rows[0]["preflight"], str(python))
            self.assertEqual((rows[0]["version"], rows[0]["kind"]), ("bead 0.1.0", "script"))
            self.assertEqual(rows[-1]["done"], True)
            proc, rows = self.soak(python, "0.2.0")
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("not a native 'bead 0.2.0'", proc.stderr)
            self.assertEqual(len(rows), 1, rows)


if __name__ == "__main__":
    unittest.main()
