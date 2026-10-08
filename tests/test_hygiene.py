"""The repository carries no trace of any maintainer's environment: no home directories, no LAN or
tailnet hostnames, no ssh targets. The patterns are shapes, not names, so this file leaks nothing itself.
The maintainer's machines and throwaway remote reach the scripts through the environment
(`BEAD_DRIVE_HOSTS`, `BEAD_TEST_REMOTE`, `BEAD_TEST_LOCAL_REPO`)."""
import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SHAPES = {
    "a home directory": re.compile(r"/(?:Users|home)/[A-Za-z][A-Za-z0-9._-]*/"),
    "a .local hostname": re.compile(r"\b[a-z0-9][a-z0-9-]*\.local\b"),
    "an ssh target": re.compile(r"\b[a-z][a-z0-9_-]*@[a-z0-9][a-z0-9-]*(?:-[0-9]+)\b"),  # user@host-01, not bead@localhost
}
ALLOWED = {"/Users/<name>/", "/home/<name>/", "/home/hostname/", "<hostname>.local"}  # placeholders in the reports


class RepositoryHygiene(unittest.TestCase):
    def test_tracked_files_name_no_machine_or_user(self):
        tracked = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True).stdout
        found = []
        for name in filter(None, tracked.decode().split("\0")):
            path = ROOT / name
            if not path.is_file() or path.suffix in (".png", ".jsonl"):
                continue
            try:
                text = path.read_text()
            except UnicodeDecodeError:
                continue
            for what, shape in SHAPES.items():
                for match in shape.finditer(text):
                    if match.group() in ALLOWED:
                        continue
                    line = text.count("\n", 0, match.start()) + 1
                    found.append(f"{name}:{line}: {what}: {match.group()}")
        self.assertEqual(found, [], "\n" + "\n".join(found))
