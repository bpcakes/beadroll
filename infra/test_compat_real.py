"""infra/compat_real.py: where it writes, with no Beads export to check (its loop over them never runs),
a stand-in bead and no br or bv."""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class CompatRealOutput(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bead-compat-real-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.scratch = self.root / "tmp"
        self.scratch.mkdir()
        (self.root / "documents").mkdir()
        # the bead under test, as the preflight reads it; with no export it is asked only --version
        self.bead = self.root / "bead"
        self.bead.write_text('#!/bin/sh\necho "bead 0.2.0"\n')
        self.bead.chmod(0o755)

    def compat(self, *args):
        env = {key: value for key, value in os.environ.items() if not key.startswith(("BEAD_", "GIT_"))}
        env.update(TMPDIR=str(self.scratch), BEAD_UNDER_TEST=str(self.bead))
        return subprocess.run([sys.executable, str(ROOT / "infra" / "compat_real.py"), "--documents",
                               str(self.root / "documents"), *args], env=env, capture_output=True, text=True,
                              timeout=120)

    def made(self):
        return sorted(path.name for path in self.scratch.iterdir())

    def test_out_names_where_it_writes_and_nothing_else_is_made(self):
        out = self.root / "out"
        proc = self.compat("--out", str(out))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(out.is_dir())
        self.assertEqual(self.made(), [])

    def test_without_out_it_writes_in_a_scratch_directory_of_its_own(self):
        proc = self.compat()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        made = self.made()
        self.assertEqual(len(made), 1, made)
        self.assertTrue(made[0].startswith("bead-compat-real-"), made)


if __name__ == "__main__":
    unittest.main()
