"""Tests of homes.py: which folders are agent homes, and what setup puts in them and takes out.

Run them with `python3 -m unittest discover -s skills/board/scripts`. Each makes a user's home
directory of its own in a scratch folder; none reads or writes the real one.
"""

import json
import os
import shutil
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
import homes  # noqa: E402

SOURCE = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))


def write(path, text=""):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as file:
        file.write(text)


def read(path):
    with open(path, encoding="utf-8") as file:
        return file.read()


class Scratch(unittest.TestCase):
    """A user's home directory with two Claude homes and two Codex homes, and no board in it."""

    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.top = os.path.realpath(scratch.name)
        self.settings = os.path.join(self.top, ".claude", "settings.json")
        self.kept_settings = (
            '{\n  "theme": "dark",\n  "hooks": {\n    "SessionStart": [\n      {\n'
            '        "matcher": "startup",\n        "hooks": [\n          {\n'
            '            "type": "command",\n            "command": "echo café"\n          }\n'
            '        ]\n      }\n    ],\n    "PreCompact": []\n  },\n  "model": "x"\n}\n'
        )
        write(self.settings, self.kept_settings)
        os.makedirs(os.path.join(self.top, ".claude-work", "projects"))
        write(os.path.join(self.top, ".codex", "config.toml"), 'sandbox_mode = "danger-full-access"\n')
        os.makedirs(os.path.join(self.top, ".codex-other", "skills", ".system"))
        os.makedirs(os.path.join(self.top, ".local", "bin"))
        self.chosen = os.path.join(self.top, "state", "setup.json")

    def path(self, *parts):
        return os.path.join(self.top, *parts)

    def setup(self, hooks=True, remove=False, dry_run=False):
        return homes.setup(SOURCE, hooks, remove, dry_run, self.chosen, self.top, {})

    def tree(self):
        """Every path under the scratch home, with what a link points to and what a file holds."""
        found = {}
        for folder, names, files in os.walk(self.top):
            for name in names + files:
                path = os.path.join(folder, name)
                if os.path.islink(path):
                    found[path] = ("link", os.readlink(path))
                elif os.path.isfile(path):
                    with open(path, "rb") as file:
                        found[path] = ("file", file.read())
        return found


class Finding(Scratch):
    def test_homes_are_the_default_folders_and_those_named_after_them_that_hold_a_homes_files(self):
        os.makedirs(self.path(".claude-mem"))  # named like a home, holding nothing of one
        write(self.path(".claude.json"), "{}")  # a file, not a folder
        os.makedirs(self.path(".codexbar"))  # not named after the default with a dash
        found = [(home.agent, os.path.basename(home.path)) for home in homes.found(self.top, {})]
        self.assertEqual(
            found, [("claude", ".claude"), ("claude", ".claude-work"), ("codex", ".codex"), ("codex", ".codex-other")]
        )

    def test_the_home_a_command_runs_under_is_found_wherever_it_is(self):
        elsewhere = self.path("accounts", "claude-x")
        os.makedirs(elsewhere)
        found = homes.found(self.top, {"CLAUDE_CONFIG_DIR": elsewhere, "CODEX_HOME": self.path(".codex")})
        self.assertIn(homes.Home("claude", elsewhere), found)
        self.assertEqual(sum(home.agent == "codex" for home in found), 2)


class Hooks(unittest.TestCase):
    def test_the_hook_is_added_once_and_after_the_hooks_a_file_already_has(self):
        other = {"matcher": "startup", "hooks": [{"type": "command", "command": "echo hi"}]}
        document = {"theme": "dark", "hooks": {"SessionStart": [other], "Stop": []}, "model": "x"}
        once = homes.with_hook(document, "claude")
        self.assertEqual(once["hooks"]["SessionStart"], [other, homes.hook_entry("claude")])
        self.assertEqual(list(once), ["theme", "hooks", "model"])
        self.assertEqual(list(once["hooks"]), ["SessionStart", "Stop"])
        self.assertIs(homes.with_hook(once, "claude"), once)
        self.assertTrue(homes.has_hook(once))
        self.assertFalse(homes.has_hook(document))

    def test_taking_the_hook_out_leaves_the_document_as_it_was_before(self):
        other = {"matcher": "startup", "hooks": [{"type": "command", "command": "echo hi"}]}
        for document in (
            {"theme": "dark", "hooks": {"SessionStart": [other], "Stop": []}, "model": "x"},
            {"theme": "dark", "hooks": {"Stop": []}},
            {"theme": "dark"},
            {},
        ):
            with self.subTest(document=document):
                hooked = homes.with_hook(document, "codex")
                self.assertEqual(homes.without_hook(hooked), document)
                self.assertEqual(list(homes.without_hook(hooked)), list(document))
        untouched = {"hooks": {}}
        self.assertIs(homes.without_hook(untouched), untouched)

    def test_hooks_of_a_shape_no_agent_writes_are_reported_and_never_rewritten(self):
        for document in ({"hooks": []}, {"hooks": {"SessionStart": {}}}):
            with self.subTest(document=document), self.assertRaises(homes.Unusable):
                homes.with_hook(document, "claude")


class Setting(Scratch):
    def test_setup_puts_the_skill_the_command_and_the_hook_into_every_home(self):
        document, lines, left = self.setup()
        self.assertFalse(left, lines)
        folder = self.path(".agents", "skills", "board")
        self.assertTrue(homes.is_skill(folder))
        self.assertEqual(sorted(os.listdir(os.path.join(folder, "scripts"))), ["board", "homes.py", "messages.py"])
        self.assertTrue(os.access(os.path.join(folder, "scripts", "board"), os.X_OK))
        for home in (".claude", ".claude-work"):
            self.assertEqual(os.path.realpath(self.path(home, "skills", "board")), folder)
        self.assertEqual(
            os.path.realpath(self.path(".local", "bin", "board")), os.path.join(folder, "scripts", "board")
        )
        for hooked in (
            self.settings, self.path(".claude-work", "settings.json"),
            self.path(".codex", "hooks.json"), self.path(".codex-other", "hooks.json"),
        ):
            self.assertTrue(homes.has_hook(json.loads(read(hooked))), hooked)
        kept = json.loads(read(self.chosen))
        self.assertIs(kept["hooks"], True)
        self.assertEqual(
            kept["homes"],
            [["claude", self.path(".claude")], ["claude", self.path(".claude-work")],
             ["codex", self.path(".codex")], ["codex", self.path(".codex-other")]],
        )
        self.assertIn({"what": "hooked", "path": self.path(".codex", "hooks.json")}, document["steps"])

    def test_only_the_skills_own_files_are_installed(self):
        source = self.path("copied", "board")
        shutil.copytree(SOURCE, source, ignore=shutil.ignore_patterns("__pycache__"))
        with open(os.path.join(source, "scripts", "._board"), "wb") as left:  # what a copy from macOS leaves
            left.write(b"\x00\x05\x16\x07\xa3")
        write(os.path.join(source, "scripts", ".DS_Store"), "x")
        _, lines, left = homes.setup(source, True, False, False, self.chosen, self.top, {})
        self.assertFalse(left, lines)
        installed = self.path(".agents", "skills", "board")
        self.assertEqual(sorted(os.listdir(os.path.join(installed, "scripts"))), ["board", "homes.py", "messages.py"])
        self.assertTrue(homes.same_skill(source, installed))

    def test_a_settings_file_keeps_everything_it_had_in_its_order_and_spelling(self):
        self.setup()
        after = read(self.settings)
        entry = json.dumps(homes.hook_entry("claude"), indent=2, ensure_ascii=False)
        entry = ",\n" + "\n".join("      " + line for line in entry.splitlines())
        marker = '        ]\n      }'
        self.assertEqual(after, self.kept_settings.replace(marker, marker + entry, 1))

    def test_setup_again_changes_nothing(self):
        self.setup()
        before = self.tree()
        document, _, left = self.setup()
        self.assertFalse(left)
        self.assertEqual({step["what"] for step in document["steps"]}, {"kept"})
        self.assertEqual(self.tree(), before)

    def test_a_dry_run_changes_nothing(self):
        before = self.tree()
        _, lines, _ = self.setup(dry_run=True)
        self.assertEqual(self.tree(), before)
        self.assertTrue(any(line.startswith("would have copied") for line in lines), lines)

    def test_remove_takes_out_what_setup_made_and_leaves_the_rest_as_it_was(self):
        before = self.tree()
        self.setup()
        _, lines, left = self.setup(remove=True)
        self.assertFalse(left, lines)
        after = self.tree()
        made = {path for path in after if path not in before}
        self.assertEqual(made, set(), "left behind")
        self.assertEqual(after[self.settings], before[self.settings])
        self.assertFalse(os.path.exists(self.path(".agents", "skills", "board")))
        self.assertFalse(os.path.exists(self.path(".codex", "hooks.json")))
        self.assertFalse(os.path.exists(self.chosen))

    def test_remove_reaches_a_home_that_only_its_variable_named_when_it_was_set_up(self):
        elsewhere = self.path("accounts", "claude-x")
        os.makedirs(elsewhere)
        before = self.tree()
        homes.setup(SOURCE, True, False, False, self.chosen, self.top, {"CLAUDE_CONFIG_DIR": elsewhere})
        self.assertTrue(os.path.islink(os.path.join(elsewhere, "skills", "board")))
        self.assertTrue(homes.has_hook(json.loads(read(os.path.join(elsewhere, "settings.json")))))
        _, lines, left = self.setup(remove=True)  # run later, from a session of another home
        self.assertFalse(left, lines)
        self.assertEqual({path for path in self.tree() if path not in before}, set(), "left behind")

    def test_remove_with_nothing_set_up_does_nothing(self):
        before = self.tree()
        document, _, left = self.setup(remove=True)
        self.assertEqual((document["steps"], left, self.tree()), ([], False, before))

    def test_without_hooks_the_skill_goes_in_and_a_hook_set_before_comes_out(self):
        self.setup()
        self.setup(hooks=False)
        self.assertEqual(read(self.settings), self.kept_settings)
        self.assertFalse(os.path.exists(self.path(".codex", "hooks.json")))
        self.assertTrue(os.path.islink(self.path(".claude", "skills", "board")))
        self.assertIs(json.loads(read(self.chosen))["hooks"], False)

    def test_what_setup_did_not_make_is_left_and_said(self):
        foreign = self.path(".claude-work", "skills", "board", "SKILL.md")
        write(foreign, "---\nname: another\n---\n")
        write(self.path(".codex", "hooks.json"), "not json")
        _, lines, left = self.setup()
        self.assertTrue(left)
        self.assertEqual(read(foreign), "---\nname: another\n---\n")
        self.assertEqual(read(self.path(".codex", "hooks.json")), "not json")
        self.assertEqual(sum(line.startswith("skipped") for line in lines), 2, lines)
        self.assertTrue(os.path.islink(self.path(".claude", "skills", "board")))

    def test_a_folder_of_something_else_where_the_skill_goes_stops_setup_before_any_change(self):
        write(self.path(".agents", "skills", "board", "notes.txt"), "mine")
        before = self.tree()
        with self.assertRaises(homes.Unusable):
            self.setup()
        self.assertEqual(self.tree(), before)


class Healing(Scratch):
    def test_a_home_made_after_setup_gets_what_the_others_have(self):
        self.setup()
        os.makedirs(self.path(".claude-new", "projects"))
        write(self.path(".codex-new", "auth.json"), "{}")
        done = homes.heal(self.chosen, self.top, {})
        self.assertEqual(
            sorted((step.what, os.path.relpath(step.path, self.top)) for step in done),
            [
                ("hooked", os.path.join(".claude-new", "settings.json")),
                ("hooked", os.path.join(".codex-new", "hooks.json")),
                ("linked", os.path.join(".claude-new", "skills", "board")),
            ],
        )
        self.assertIn(["claude", self.path(".claude-new")], json.loads(read(self.chosen))["homes"])
        self.assertEqual(homes.heal(self.chosen, self.top, {}), [])

    def test_nothing_is_healed_before_setup_or_after_remove(self):
        self.assertEqual(homes.heal(self.chosen, self.top, {}), [])
        self.setup()
        self.setup(remove=True)
        os.makedirs(self.path(".claude-new", "projects"))
        self.assertEqual(homes.heal(self.chosen, self.top, {}), [])
        self.assertFalse(os.path.exists(self.path(".claude-new", "skills")))

    def test_healing_follows_a_setup_without_hooks(self):
        self.setup(hooks=False)
        os.makedirs(self.path(".claude-new", "projects"))
        done = homes.heal(self.chosen, self.top, {})
        self.assertEqual([step.what for step in done], ["linked"])
        self.assertFalse(os.path.exists(self.path(".claude-new", "settings.json")))


class Reporting(Scratch):
    def test_the_rows_say_what_each_home_lacks_and_then_that_nothing_is_missing(self):
        before = homes.rows(SOURCE, self.chosen, self.top, {})
        self.assertFalse(any(row["ok"] for row in before if row["what"] in ("skill", "claude", "codex")))
        self.setup()
        after = {row["text"].split(":")[0]: row for row in homes.rows(SOURCE, self.chosen, self.top, {})}
        for home in (".claude", ".claude-work", ".codex", ".codex-other"):
            self.assertTrue(after[self.path(home)]["ok"], after[self.path(home)])
        self.assertIn("commands run with full access", after[self.path(".codex")]["text"])
        self.assertIn("sandbox until approved", after[self.path(".codex-other")]["text"])

    def test_a_copy_that_differs_from_this_one_is_said_to_differ(self):
        self.setup()
        write(self.path(".agents", "skills", "board", "scripts", "board"), "old")
        skill = [row for row in homes.rows(SOURCE, self.chosen, self.top, {}) if row["what"] == "skill"][0]
        self.assertFalse(skill["ok"])
        self.assertIn("differs", skill["text"])


if __name__ == "__main__":
    unittest.main()
