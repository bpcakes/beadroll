"""infra/environment.py: what a driver's bead and git are not handed of the caller's environment."""
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import environment


class Scrubbed(unittest.TestCase):
    CALLER = {"PATH": "/bin", "HOME": "/home/x", "BEAD_HOME": "/elsewhere", "GIT_DIR": "/repo.git",
              "CLAUDE_CODE_SESSION_ID": "s", "CODEX_THREAD_ID": "t",
              "CURSOR_CONVERSATION_ID": "u", "CURSOR_AGENT": "1", "BV_OUTPUT_FORMAT": "json"}

    def test_what_routes_bead_or_git_or_names_an_agent_is_dropped(self):
        with mock.patch.dict(os.environ, self.CALLER, clear=True):
            self.assertEqual(environment.scrubbed(),
                             {"PATH": "/bin", "HOME": "/home/x", "BV_OUTPUT_FORMAT": "json"})

    def test_another_tools_prefixes_are_dropped_as_well(self):
        with mock.patch.dict(os.environ, self.CALLER, clear=True):
            self.assertEqual(environment.scrubbed("BV_"), {"PATH": "/bin", "HOME": "/home/x"})


if __name__ == "__main__":
    unittest.main()
