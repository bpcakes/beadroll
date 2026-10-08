"""The base under the protocol: git environment and remotes, URLs and credentials, store files and
modes, output streams, imports and robustness. Gathered from the seventh independent review
(2026-10-07-7-claude); G.. docstrings name findings in FINDINGS.md, V.. tests are properties that held."""
import concurrent.futures
import contextlib
import http.server
import itertools
import json
import os
import re
import shlex
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

import tests.support as support


class G01_TrackedRemoteIsNeverAGitOption(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_tracked_remote_that_looks_like_an_option_runs_nothing(self):
        """G01: a committed .beadroll.json `remote` beginning with `--upload-pack=` reaches `git fetch` as an
        option, so `bead claim` and `bead sync` run the command it names."""
        a = support.Machine(self.box, "g01")
        claim_marker, sync_marker = self.box.root / "RAN-ON-CLAIM", self.box.root / "RAN-ON-SYNC"
        config = a.checkout / ".beadroll.json"
        config.write_text(json.dumps({"workspace": "ws-g01-0001", "remote": f"--upload-pack=touch {claim_marker} ;"}))
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "work")["id"]
        claim = a.run("claim", key, "--json")
        self.assertFalse(claim_marker.exists(),
                         "bead claim passed the tracked remote to git as an option and ran the command it named "
                         f"(rc={claim.returncode})")
        config.write_text(json.dumps({"workspace": "ws-g01-0001", "remote": f"--upload-pack=touch {sync_marker} ;"}))
        sync = a.run("sync", "--json")
        self.assertFalse(sync_marker.exists(),
                         "bead sync passed the tracked remote to git as an option and ran the command it named "
                         f"(rc={sync.returncode})")
        self.assertNotEqual(sync.returncode, 0, "an option-shaped remote cannot be a successful coordination remote")

        # The fix must keep honest remotes working, including a relative path that begins with '-'.
        b = support.Machine(self.box, "g01c")
        bare = b.checkout / "-coord.git"
        support.git("init", "-q", "--bare", str(bare))
        (b.checkout / ".beadroll.json").write_text(json.dumps({"workspace": "ws-g01-0002", "remote": "-coord.git"}))
        b.bead("init", "--prefix", "ex")
        other = b.bead("create", "work")["id"]
        honest = b.run("claim", other, "--json")
        self.assertEqual(honest.returncode, 0, honest.stderr)
        self.assertIn("refs/beads/journal", support.git("for-each-ref", "--format=%(refname)", cwd=bare),
                      "the claim was not published to the dash-named remote")


def _g02_seed_repo(box, name, origin, config):
    """A clone of `origin` carrying a committed .beadroll.json, pushed to its origin."""
    checkout = box.clone(name, origin=origin)
    (checkout / ".beadroll.json").write_text(json.dumps(config) + "\n")
    support.git("add", "-A", cwd=checkout)
    support.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "cfg", cwd=checkout)
    support.git("push", "-q", "origin", "HEAD:main", cwd=checkout)
    return checkout

def _g02_journal_refs(bare):
    return subprocess.run(["git", "--git-dir", str(bare), "for-each-ref", "--format=%(refname)"],
                          capture_output=True, text=True).stdout

class G02_TrackedConfigCannotAdoptAnotherRepositorysStore(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_foreign_derived_workspace_id_is_refused(self):
        """G02: a committed .beadroll.json naming another repository's URL-derived workspace id (plus a remote of
        its own) binds the checkout to that repository's existing store, reads it, publishes it to the config's
        remote, and leaves that store's remembered remote switched."""
        bp = support.load_module()
        victim = support.Machine(self.box, "victim")
        victim.bead("init", "--prefix", "p")
        secret = victim.bead("create", "SECRET acquisition plan", "-p", "1")["id"]
        victim.bead("sync")
        pid = victim.bead("info")["workspace"]
        self.assertEqual(pid, bp.hashlib.sha256(bp.normalize_remote(str(self.box.origin)).encode()).hexdigest()[:16],
                         "precondition: the victim's workspace id is the public URL-derived id")

        attacker_bare, collect = self.box.bare("attacker.git"), self.box.bare("collect.git")
        _g02_seed_repo(self.box, "hostile-seed", attacker_bare, {"workspace": pid, "remote": str(collect)})
        victim_in_m = self.box.clone("victim-in-m", origin=attacker_bare)

        # A routine read inside the attacker's checkout must not reach the victim's store.
        ready = victim.run("ready", "--json", cwd=victim_in_m)
        self.assertEqual((ready.returncode, support.code(ready) if ready.returncode else None), (1, "CONFIG"),
                         f"a foreign checkout read the victim's store: rc={ready.returncode} out={ready.stdout[:200]}")
        # A routine sync must not publish the victim's journal to the config's remote.
        sync = victim.run("sync", "--json", cwd=victim_in_m)
        self.assertNotEqual(sync.returncode, 0, "sync inside the hostile checkout succeeded against the victim's store")
        self.assertNotIn("refs/beads/journal", _g02_journal_refs(collect),
                         "the victim's journal was published to the config's own remote")
        # The victim's own checkout still coordinates through its real origin.
        self.assertEqual(victim.bead("info")["remote"], os.path.realpath(self.box.origin),
                         "the victim's own store was redirected to the config's remote")
        self.assertIn(secret, {i["id"] for i in victim.bead("list")["issues"]})

        # Control: an `init --tracked` config (a uuid7 workspace) copied into a repository with a different
        # origin still joins the same tracker, so the fix does not block the supported sharing path.
        a = support.Machine(self.box, "a")
        coordination = self.box.bare("coordination.git")
        a.bead("init", "--prefix", "ex", "--tracked", "--remote", str(coordination))
        a.bead("create", "shared")
        a.bead("sync")
        elsewhere = self.box.clone("elsewhere", origin=self.box.bare("another-origin.git"))
        shutil.copy(a.checkout / ".beadroll.json", elsewhere / ".beadroll.json")
        b = support.Machine(self.box, "b")
        joined = b.run("sync", "--json", cwd=elsewhere)
        self.assertEqual(joined.returncode, 0, joined.stderr)
        self.assertEqual(b.bead("list", cwd=elsewhere)["total"], 1)
        self.assertEqual(b.bead("info", cwd=elsewhere)["workspace_source"], "tracked .beadroll.json")

    def test_tracked_remote_change_does_not_silently_move_an_existing_store(self):
        """G02: editing a committed .beadroll.json `remote` to a different endpoint silently republishes an
        existing store's journal there; the change must require consent."""
        a = support.Machine(self.box, "a")
        good, evil, third = self.box.bare("good.git"), self.box.bare("evil.git"), self.box.bare("third.git")
        a.bead("init", "--prefix", "p", "--tracked", "--remote", str(good))
        a.bead("create", "work")
        a.bead("sync")
        config_path = a.checkout / ".beadroll.json"
        config = json.loads(config_path.read_text())
        config["remote"] = str(evil)
        config_path.write_text(json.dumps(config) + "\n")

        sync = a.run("sync", "--json")
        self.assertEqual((sync.returncode, support.code(sync) if sync.returncode else None), (1, "CONFIG"),
                         f"the store's remote was switched by an edited config without consent: {sync.stdout[:200]}")
        self.assertNotIn("refs/beads/journal", _g02_journal_refs(evil),
                         "the journal was republished to the config's new remote")
        # Consent through an explicit --remote is still honoured.
        self.assertEqual(a.run("sync", "--remote", str(evil), "--json").returncode, 0,
                         "an explicit --remote switch was refused")
        # So is the documented switch, `init --tracked --remote` (the route the CONFIG error names).
        a.bead("init", "--tracked", "--remote", str(third))
        switched = a.run("sync", "--json")
        self.assertEqual(switched.returncode, 0, f"a switch made with `init --tracked --remote` was refused: "
                                                 f"{switched.stderr[:300]}")
        self.assertIn("refs/beads/journal", _g02_journal_refs(third))


def _g03_origin_refs(box):
    return support.git("--git-dir", str(box.origin), "for-each-ref", "--format=%(refname)")

class G03_DamagedMetaMovesCoordinationToOrigin(unittest.TestCase):
    """G03: without --tracked, the coordination remote named with `init --remote` / `sync --remote` is
    recorded only in the store's meta.json. Workspace.meta() turns a file it cannot read or parse into
    {}, so remote() falls back to origin: the claim is settled against origin (two confirmed owners),
    the tracker is pushed into the code origin, and save_meta rewrites meta.json with origin."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.coord = self.box.bare("coord.git")
        self.a = support.Machine(self.box, "a")
        self.a.bead("init", "--prefix", "ex", "--remote", str(self.coord))  # coordination remote is not origin
        self.key = self.a.bead("create", "contested")["id"]
        self.other = self.a.bead("create", "uncontested")["id"]
        self.a.bead("sync")
        self.b = support.Machine(self.box, "b")
        self.b.bead("sync", "--remote", str(self.coord))  # b remembers coord in meta.json ("last used")
        self.meta = self.b.store() / "meta.json"
        self.assertEqual(json.loads(self.meta.read_text())["remote"], str(self.coord.resolve()))

    def _damage(self):
        self.meta.write_text(self.meta.read_text().rstrip().rstrip("}") + ",}\n")  # a hand edit's trailing comma

    def _contest(self):
        self.a.bead("claim", self.key)  # a holds the issue, confirmed through coord
        claimed = self.b.run("claim", self.key, "--json")
        self.assertNotIn("refs/beads/journal", _g03_origin_refs(self.box),
                         "the tracker was published to the code origin")
        self.assertNotEqual(claimed.returncode, 0,
                            "b confirmed a claim that a holds, by settling it against origin: " + claimed.stdout)
        self.assertIn("meta.json", json.loads(claimed.stderr)["error"]["message"])

    def test_malformed_meta_does_not_move_coordination_to_origin(self):
        """G03: a meta.json with a trailing comma silently moves claims to origin; a remote named with
        --remote must still work (and be reported as accepted) while the file is damaged."""
        self._damage()
        self._contest()
        damaged = self.meta.read_text()
        self.b.bead("init")  # init does not consult the remote; it must not overwrite what it could not read
        self.assertEqual(self.meta.read_text(), damaged, "init replaced a meta.json it could not read")
        claimed = self.b.run("claim", self.other, "--remote", str(self.coord), "--json")
        self.assertEqual(claimed.returncode, 0, claimed.stderr)
        self.a.bead("sync")
        self.assertEqual(self.a.show(self.other)["claim"]["owner"], self.b.show(self.other)["claim"]["owner"])
        self.assertEqual(json.loads(self.meta.read_text())["remote"], str(self.coord.resolve()))
        self.assertNotIn("refs/beads/journal", _g03_origin_refs(self.box))

    @unittest.skipIf(hasattr(os, "geteuid") and os.geteuid() == 0, "root reads a mode-0 file")
    def test_unreadable_meta_does_not_move_coordination_to_origin(self):
        """G03: a meta.json this user cannot read (EACCES) silently moves claims to origin."""
        self.meta.chmod(0)
        try:
            self._contest()
        finally:
            if self.meta.exists():
                self.meta.chmod(0o600)

    def test_owner_close_with_damaged_meta_stays_off_origin(self):
        """G03: an owner's close with a damaged meta.json publishes the tracker to origin; it must be
        recorded locally, succeed, and leave origin alone."""
        self.b.bead("claim", self.key)
        self._damage()
        closed = self.b.run("close", self.key)
        self.assertNotIn("refs/beads/journal", _g03_origin_refs(self.box),
                         "the owner's close published the tracker to the code origin")
        self.assertEqual(closed.returncode, 0, closed.stderr)
        self.assertIn("not yet published", closed.stdout)
        self.assertEqual(self.b.show(self.key)["status"], "closed")
        info = self.b.run("info", "--json")
        self.assertEqual(info.returncode, 0, info.stderr)  # info diagnoses the file instead of stopping on it
        self.assertIn("meta.json", json.loads(info.stdout)["remote"])


def _g04_commit_config(machine):
    support.git("add", ".beadroll.json", cwd=machine.checkout)
    support.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "tracker",
                cwd=machine.checkout)
    support.git("push", "-q", "origin", "HEAD:main", cwd=machine.checkout)

def _g04_outcome(proc):
    """The owner a successful claim reports, or the error code of a refused one."""
    if proc.returncode:
        return proc.stderr.strip()[:200]
    return "confirmed owner " + json.loads(proc.stdout)[0]["claim"]["owner"]

class G04_TrackedInitKeepsHandSetConfig(unittest.TestCase):
    """G04: `init --tracked` rebuilds .beadroll.json from scratch, keeping only workspace and remote."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_g04_reinit_keeps_where_false_ref_and_unknown_keys(self):
        """G04: re-running `init --tracked --remote X` keeps `"where": false`, `"ref"` and keys bead does not
        know; without them the next event records branch and commit again and the checkout changes ref."""
        m = support.Machine(self.box, "g04")
        made = m.run("init", "--prefix", "ex", "--tracked", "--json")
        self.assertEqual(made.returncode, 0, made.stderr)
        config_path = m.checkout / ".beadroll.json"
        config = json.loads(config_path.read_text())
        config.update({"where": False, "ref": "refs/beads/team", "later_key": 1})
        config_path.write_text(json.dumps(config, indent=1) + "\n")
        other = self.box.bare("other.git")
        again = m.run("init", "--tracked", "--remote", str(other), "--json")
        self.assertEqual(again.returncode, 0, again.stderr)
        m.bead("create", "after re-init")
        rewritten = json.loads(config_path.read_text())
        segment = sorted(m.store().glob("events/*/*.jsonl"))[-1]
        last = json.loads(segment.read_text().splitlines()[-1])
        self.assertEqual(rewritten.get("remote"), str(other))
        self.assertEqual((rewritten.get("where"), rewritten.get("ref"), rewritten.get("later_key")),
                         (False, "refs/beads/team", 1), f"init --tracked dropped hand-set keys: {rewritten}")
        self.assertNotIn("where", last, "the `where: false` opt-out was dropped: the event records branch and commit")

    def test_g04_reinit_does_not_move_the_checkout_to_another_ref(self):
        """G04: re-init naming the same remote drops `"ref"`: this checkout moves to refs/beads/journal while its
        teammates stay on the committed ref, and two machines both hold an online-confirmed claim on one issue."""
        a, b = support.Machine(self.box, "g04-a"), support.Machine(self.box, "g04-b")
        made = a.run("init", "--prefix", "ex", "--tracked", "--remote", str(self.box.origin), "--json")
        self.assertEqual(made.returncode, 0, made.stderr)
        config_path = a.checkout / ".beadroll.json"
        config = json.loads(config_path.read_text())
        config["ref"] = "refs/beads/team"
        config_path.write_text(json.dumps(config, indent=1) + "\n")
        _g04_commit_config(a)
        support.git("pull", "-q", "origin", "main", cwd=b.checkout)
        key = a.bead("create", "shared work")["id"]
        a.bead("sync")
        b.bead("sync")
        again = a.run("init", "--tracked", "--remote", str(self.box.origin), "--json")
        self.assertEqual(again.returncode, 0, again.stderr)
        claims = [m.run("claim", key, "--json", thread=f"{m.name}-agent") for m in (a, b)]
        self.assertEqual(sorted(p.returncode for p in claims), [0, 1],
                         "both machines hold a confirmed claim on the same issue: "
                         + " | ".join(_g04_outcome(p) for p in claims))
        self.assertEqual(a.bead("info")["ref"], "refs/beads/team")

    def test_g04_reinit_does_not_write_back_a_hand_set_password(self):
        """G04: re-running `init --tracked` keeps `"where": false` and, while keeping the existing keys, never
        writes a hand-set password back into the tracked file (guard for the fix; bead never stores one)."""
        m = support.Machine(self.box, "g04-pw")
        made = m.run("init", "--prefix", "ex", "--tracked", "--json")
        self.assertEqual(made.returncode, 0, made.stderr)
        config_path = m.checkout / ".beadroll.json"
        config = json.loads(config_path.read_text())
        config.update({"remote": "https://alice:supersecret@example.invalid/x.git", "where": False})
        config_path.write_text(json.dumps(config, indent=1) + "\n")
        again = m.run("init", "--tracked", "--json")
        self.assertEqual(again.returncode, 0, again.stderr)
        rewritten = json.loads(config_path.read_text())
        self.assertNotIn("supersecret", json.dumps(rewritten))
        self.assertIs(rewritten.get("where"), False, f"init --tracked dropped `where`: {rewritten}")


def _g05_fake_ssh(box):
    """An ssh stand-in for git (GIT_SSH_COMMAND): it runs the remote command in $FAKE_SSH_ROOT, locally."""
    path = box.root / "fake-ssh"
    path.write_text("#!/bin/sh\n"
                    "[ \"$1\" = \"-G\" ] && exit 0\n"
                    "for last; do :; done\n"
                    "PATH=\"$(git --exec-path):$PATH\"\n"
                    "cd \"$FAKE_SSH_ROOT\" && exec sh -c \"$last\"\n")
    path.chmod(0o755)
    return path

def _g05_git_env(box, text=""):
    """A scratch global git config for this sandbox (never the developer's own)."""
    path = box.root / f"gitconfig-{len(list(box.root.glob('gitconfig-*')))}"
    path.write_text(text)
    return {"GIT_CONFIG_GLOBAL": str(path), "GIT_CONFIG_NOSYSTEM": "1"}

def _g05_info(test, machine, cwd=None):
    proc = machine.run("info", "--json", cwd=cwd)
    test.assertEqual(proc.returncode, 0, proc.stderr)
    return json.loads(proc.stdout)

class G05_OriginsGitReadsAsRemotes(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_origin_shapes_git_reads_as_remotes_are_one_workspace(self):
        """G05: an origin git reads as ssh or a remote helper (host:path without user@, an ssh alias, an
        insteadOf shorthand such as gh:org/repo, bracketed IPv6 scp, an upper-case scheme, transport::address)
        is not a path under the checkout: every clone, worktree and subdirectory gets one workspace and the
        remote is the URL itself."""
        one, two = support.Machine(self.box, "one"), support.Machine(self.box, "two")
        worktree = self.box.root / "one-wt"
        support.git("worktree", "add", "-q", str(worktree), cwd=one.checkout)
        (one.checkout / "sub").mkdir()
        for url in ("myhost:org/repo.git", "host.example:org/repo.git", "gh:org/repo", "[::1]:org/repo.git",
                    "HTTPS://host.example/org/repo.git", "codecommit::us-east-1://my-repo"):
            with self.subTest(origin=url):
                for machine in (one, two):
                    support.git("remote", "set-url", "origin", url, cwd=machine.checkout)
                seen = {"clone": _g05_info(self, one), "worktree": _g05_info(self, one, cwd=worktree),
                        "subdirectory": _g05_info(self, one, cwd=one.checkout / "sub"), "other clone": _g05_info(self, two)}
                ids = {where: info["workspace"] for where, info in seen.items()}
                self.assertEqual(len(set(ids.values())), 1,
                                 f"git reads {url!r} as a remote, yet each checkout got its own tracker: {ids}")
                for where, info in seen.items():
                    self.assertEqual(info["remote"], url, f"{where}: the remote became a path under the checkout")

    def test_upper_case_scheme_is_the_same_remote(self):
        """G05: HTTPS://Host/org/repo and https://host/org/repo are one remote, so one workspace."""
        machine = support.Machine(self.box, "case")
        ids = []
        for url in ("https://host.example/org/repo.git", "HTTPS://host.example/org/repo.git"):
            support.git("remote", "set-url", "origin", url, cwd=machine.checkout)
            ids.append(_g05_info(self, machine)["workspace"])
        self.assertEqual(ids[0], ids[1], "an upper-case scheme gave the same remote a different workspace")

    def test_scp_and_shorthand_origins_sync(self):
        """G05: `myhost:origin.git` (an ssh alias, no user@) and `gh:origin.git` (an insteadOf shorthand) are
        remotes git reaches; bead must sync through them and a second clone must see the issue."""
        ssh = _g05_fake_ssh(self.box)
        shapes = {
            "ssh-alias": ("myhost:{name}.git", {**_g05_git_env(self.box), "GIT_SSH_COMMAND": str(ssh),
                                                "FAKE_SSH_ROOT": str(self.box.root)}),
            "insteadof": ("gh:{name}.git", _g05_git_env(self.box, f'[url "{self.box.root}/"]\n\tinsteadOf = gh:\n')),
        }
        for shape, (template, env) in shapes.items():
            with self.subTest(shape=shape):
                url = template.format(name=f"tracker-{shape}")
                self.box.bare(f"tracker-{shape}.git")
                a, b = support.Machine(self.box, f"{shape}-a"), support.Machine(self.box, f"{shape}-b")
                for machine in (a, b):
                    support.git("remote", "set-url", "origin", url, cwd=machine.checkout)
                    machine.extra.update(env)
                reached = subprocess.run(["git", "ls-remote", "origin"], cwd=a.checkout, capture_output=True, text=True,
                                         env={**{k: v for k, v in os.environ.items() if not k.startswith("GIT_")}, **env})
                self.assertEqual(reached.returncode, 0, f"git itself cannot reach {url}: {reached.stderr}")
                self.assertEqual(a.run("init", "--prefix", "ex").returncode, 0)
                key = a.bead("create", "shared through the remote")["id"]
                synced = a.run("sync", "--json")
                self.assertEqual(synced.returncode, 0, f"bead could not sync through {url}: {synced.stderr}")
                pulled = b.run("sync", "--json")
                self.assertEqual(pulled.returncode, 0, pulled.stderr)
                self.assertEqual(b.show(key)["title"], "shared through the remote")


class G06_HttpsTokenInTheUserPosition(unittest.TestCase):
    TOKEN = "ghp_G06TESTTOKEN0123"

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_token_user_part_is_not_written_to_tracked_config_or_meta(self):
        """G06: https://TOKEN:x-oauth-basic@host and https://TOKEN:@host carry the token as the user part; bead
        must not write it to .beadroll.json or meta.json (it says the password was dropped), and the value it
        stores must be the value it uses."""
        for shape, url in (("x-oauth-basic", f"https://{self.TOKEN}:x-oauth-basic@example.invalid/org/repo.git"),
                           ("empty-password", f"https://{self.TOKEN}:@example.invalid/org/repo.git")):
            with self.subTest(shape=shape):
                tracked = support.Machine(self.box, f"{shape}-tracked")
                made = tracked.run("init", "--prefix", "ex", "--tracked", "--remote", url, "--json")
                self.assertEqual(made.returncode, 0, made.stderr)
                config = (tracked.checkout / ".beadroll.json").read_text()
                self.assertNotIn(self.TOKEN, config, "the token is in .beadroll.json, which bead says to commit")
                plain = support.Machine(self.box, f"{shape}-meta")
                made = plain.run("init", "--prefix", "ex", "--remote", url, "--json")
                self.assertEqual(made.returncode, 0, made.stderr)
                meta = json.loads(next(plain.home.glob("*/meta.json")).read_text())
                self.assertNotIn(self.TOKEN, meta["remote"], "the token is stored in meta.json")
                info = plain.run("info", "--json")
                self.assertEqual(info.returncode, 0, info.stderr)
                self.assertNotIn(self.TOKEN, info.stdout)
                self.assertEqual(meta["remote"], json.loads(info.stdout)["remote"],
                                 "bead stores one remote and uses another")

    def test_token_user_part_of_origin_is_not_shown(self):
        """G06: a checkout whose origin is http://TOKEN:x-oauth-basic@host must not have bead print the token
        in info or in a sync error (git itself redacts it), nor store it in meta.json."""
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]  # closed again: connections are refused, nothing leaves the host
        machine = support.Machine(self.box, "origin-token")
        url = f"http://{self.TOKEN}:x-oauth-basic@127.0.0.1:{port}/org/repo.git"
        support.git("remote", "set-url", "origin", url, cwd=machine.checkout)
        self.assertEqual(machine.run("init", "--prefix", "ex", "--json").returncode, 0)
        info = machine.run("info", "--json")
        self.assertEqual(info.returncode, 0, info.stderr)
        synced = machine.run("sync", "--json", GIT_TERMINAL_PROMPT="0")
        self.assertNotEqual(synced.returncode, 0)  # nothing answers; only what bead prints matters
        metas = "".join(p.read_text() for p in machine.home.glob("*/meta.json"))
        self.assertNotIn(self.TOKEN, info.stdout + synced.stdout + synced.stderr + metas)


def _g07_machine(box, name, init=True):
    machine = support.Machine(box, name)
    machine.bead(*(["init", "--prefix", "ex"] if init else ["sync"]))
    return machine

class G07_ReceiptSurvivesLaterLocalWrites(unittest.TestCase):
    """Only a coordinated command drops an actor's receipts. Later local writes by that actor (close,
    reopen, update without --claim, comments) go through without dropping them, so a later deliberate
    repeat of the same command is answered from the old receipt and does nothing, exit 0."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_new_claim_after_close_and_reopen_claims(self):
        """G07: after the actor's own close, reopen and edit went through, `claim X` claims X; it is not
        answered "Claimed (confirmed)" from the old receipt while nobody holds X."""
        a = _g07_machine(self.box, "g07-again")
        x = a.bead("create", "X")["id"]
        a.bead("sync")
        self.assertEqual(a.run("claim", x, BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
        self.assertEqual(a.run("sync").returncode, 0)  # the agent finds it holds X and works on it
        self.assertEqual(a.run("close", x).returncode, 0)
        self.assertEqual(a.run("reopen", x).returncode, 0)
        self.assertEqual(a.run("update", x, "--title", "X again").returncode, 0)
        claim = a.run("claim", x)
        self.assertEqual(claim.returncode, 0, claim.stderr)
        self.assertIsNotNone(a.show(x).get("claim"), f"'{claim.stdout.strip()}' but nobody holds {x}")

    def test_new_claiming_close_after_reopen_closes(self):
        """G07: after the actor's own reopen went through, `update X --claim --status closed` closes X
        again; it is not answered "Updated" from the old receipt while X stays open."""
        a = _g07_machine(self.box, "g07-again-close")
        x = a.bead("create", "X")["id"]
        a.bead("sync")
        crashed = a.run("update", x, "--claim", "--status", "closed", BEAD_TEST_CRASH_AFTER_PUSH="1")
        self.assertEqual(crashed.returncode, 99)
        self.assertEqual(a.run("sync").returncode, 0)
        self.assertEqual(a.run("reopen", x).returncode, 0)
        again = a.run("update", x, "--claim", "--status", "closed")
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(a.show(x)["status"], "closed", f"'{again.stdout.strip()}' but {x} is still open")

    def test_write_made_while_the_attempt_was_pending_also_moves_the_actor_on(self):
        """G07: a comment made before anyone synced (the attempt still pending) went through after the
        crash, so once sync has settled the attempt and the claim has ended, `claim X` claims X."""
        a = _g07_machine(self.box, "g07-pending-write")
        x = a.bead("create", "X")["id"]
        a.bead("sync")
        self.assertEqual(a.run("claim", x, BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
        self.assertEqual(a.run("comments", "add", x, "work continues").returncode, 0)
        self.assertEqual(a.run("sync").returncode, 0)  # settles the attempt: a holds X
        b = _g07_machine(self.box, "g07-pending-write-b", init=False)
        self.assertEqual(b.run("takeover", x, "--reason", "a went quiet").returncode, 0)
        self.assertEqual(b.run("release", x).returncode, 0)
        self.assertEqual(a.run("sync").returncode, 0)
        self.assertIsNone(a.show(x).get("claim"))
        claim = a.run("claim", x)
        self.assertEqual(claim.returncode, 0, claim.stderr)
        self.assertIsNotNone(a.show(x).get("claim"), f"'{claim.stdout.strip()}' but nobody holds {x}")


def _g08_machine(box, name, init=True):
    machine = support.Machine(box, name)
    machine.bead(*(["init", "--prefix", "ex"] if init else ["sync"]))
    return machine

class G08_StaleReceiptsDroppedBeforeTheCommandGoesThrough(unittest.TestCase):
    """coordinated drops the actor's settled receipts as soon as a later command's build succeeds
    (bead 1910), before its push. When that later command then fails, the retry of the earlier,
    accepted command is no longer recognised, although REVIEWING section 2 says a receipt survives a
    failed later command by the same actor."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_crash_before_push_of_a_later_command_keeps_the_accepted_close(self):
        """G08: a later `claim Y` that crashed before its push did not go through; the retry of the
        accepted claiming close is still recognised (not VALIDATION "is closed")."""
        a = _g08_machine(self.box, "g08-crash")
        x = a.bead("create", "X")["id"]
        y = a.bead("create", "Y")["id"]
        a.bead("sync")
        first = a.run("update", x, "--claim", "--status", "closed", "--json", BEAD_TEST_CRASH_AFTER_PUSH="1")
        self.assertEqual(first.returncode, 99)
        later = a.run("claim", y, "--json", BEAD_TEST_CRASH_BEFORE_PUSH="1")
        self.assertEqual(later.returncode, 98)  # the later command failed: it never reached the remote
        retry = a.run("update", x, "--claim", "--status", "closed", "--json")
        self.assertEqual(retry.returncode, 0, f"retry of the accepted close not recognised: {retry.stderr}")

    def test_unresolved_later_command_keeps_the_accepted_release(self):
        """G08: a later `claim Y` that ended UNRESOLVED (and never landed) did not go through; the retry
        of the accepted release is still recognised (not NOT_OWNER)."""
        a = _g08_machine(self.box, "g08-unresolved")
        x = a.bead("create", "X")["id"]
        y = a.bead("create", "Y")["id"]
        a.bead("claim", x)
        a.bead("sync")
        self.assertEqual(a.run("release", x, "--json", BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
        gate = self.box.root / "gate"
        gate.mkdir()
        hook = self.box.origin / "hooks" / "pre-receive"
        hook.write_text(f"#!/bin/sh\nwhile [ -d {gate} ]; do sleep 0.05; done\nexit 1\n")
        hook.chmod(0o755)
        self.addCleanup(hook.unlink, missing_ok=True)
        later = a.run("claim", y, "--json", BEAD_NET_TIMEOUT="1")
        gate.rmdir()
        time.sleep(1)  # the held pushes are refused once the gate is gone
        hook.unlink()
        self.assertEqual(later.returncode, 1, later.stderr)
        self.assertEqual(support.code(later), "UNRESOLVED")
        retry = a.run("release", x, "--json")
        self.assertEqual(retry.returncode, 0, f"retry of the accepted release not recognised: {retry.stderr}")

    def test_failed_later_command_does_not_make_the_retry_reapply_edits(self):
        """G08: after a later command of the same actor failed before its push, the retry of an accepted
        `update --claim --title` must not apply the title a second time over another machine's newer one."""
        a = _g08_machine(self.box, "g08-overwrite")
        x = a.bead("create", "X")["id"]
        y = a.bead("create", "Y")["id"]
        a.bead("sync")
        b = _g08_machine(self.box, "g08-overwrite-b", init=False)
        crashed = a.run("update", x, "--claim", "--title", "from-A", "--json", BEAD_TEST_CRASH_AFTER_PUSH="1")
        self.assertEqual(crashed.returncode, 99)
        b.bead("sync")
        b.bead("update", x, "--title", "from-B")
        b.bead("sync")
        self.assertEqual(a.run("claim", y, "--json", BEAD_TEST_CRASH_BEFORE_PUSH="1").returncode, 98)
        retry = a.run("update", x, "--claim", "--title", "from-A", "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        a.bead("sync")
        b.bead("sync")
        self.assertEqual(b.show(x)["title"], "from-B",
                         "the accepted update was applied a second time, over the other machine's later title")


def _g09_machine(box, name, init=True):
    machine = support.Machine(box, name)
    machine.bead(*(["init", "--prefix", "ex"] if init else ["sync"]))
    return machine

class G09_ReceiptLoopVariableShadowsTheInFlightReceipt(unittest.TestCase):
    """`for receipt in stale:` (bead 1910) rebinds `receipt`, the in-flight (events, result) tuple, to
    a Path. When the push is then refused because another machine published first, the next round
    evaluates `receipt[0]` on a PosixPath: TypeError, a traceback instead of another round."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_ordinary_contention_after_a_settled_receipt_retries_the_claim(self):
        """G09: with a settled receipt left by an earlier lost acknowledgement, a claim whose first push
        loses an ordinary race retries and succeeds instead of crashing with TypeError."""
        a = _g09_machine(self.box, "g09-contend")
        x = a.bead("create", "X")["id"]
        y = a.bead("create", "Y")["id"]
        a.bead("sync")
        b = _g09_machine(self.box, "g09-contend-b", init=False)
        self.assertEqual(a.run("claim", x, BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
        self.assertEqual(a.run("sync").returncode, 0)  # the accepted claim is found; its receipt is settled
        self.assertEqual(len(list((a.store() / "pending").glob("*.command"))), 1)
        with mock.patch.dict(os.environ, a.env(), clear=True), contextlib.chdir(a.checkout):
            bp = support.load_module()
            real = bp.Link.publish
            claims = []

            def contended(link, tip, note="sync", pending=(), fence=False):
                if note == "claim" and not claims:
                    claims.append(tip)
                    b.bead("create", "B's unrelated work")
                    b.bead("sync")  # machine B publishes between A's fetch and A's push
                return real(link, tip, note=note, pending=pending, fence=fence)

            with open(os.devnull, "w", encoding="utf-8") as sink, contextlib.redirect_stdout(sink), \
                    mock.patch.object(bp.Link, "publish", contended):
                try:
                    rc = bp.run(["claim", y, "--json"])
                except TypeError as error:
                    self.fail(f"claim crashed after an ordinary refused push: TypeError: {error}")
        self.assertEqual(len(claims), 1, "the contention was not injected")
        self.assertEqual(rc, 0)
        self.assertEqual(a.show(y)["claim"]["thread"], "g09-contend-thread-0001")


def _g10_machine(box, name, init=True):
    machine = support.Machine(box, name)
    machine.bead(*(["init", "--prefix", "ex"] if init else ["sync"]))
    return machine

class G10_RecognisedRetryReportsAClaimThatMovedOn(unittest.TestCase):
    """A recognised retry (settle_commands -> done) returns the original mode without looking at the
    claim as it is now: `claim X` answers "Claimed X (confirmed)", exit 0, while another agent holds
    X; `update X --claim` answers "Updated"; `takeover X` crashes with KeyError when the claim has
    since ended."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_claim_retry_after_a_takeover_does_not_say_claimed(self):
        """G10: a retried `claim X` whose accepted claim was since taken over by B does not answer
        "Claimed X (confirmed)" with exit 0."""
        a = _g10_machine(self.box, "g10-moved")
        x = a.bead("create", "X")["id"]
        a.bead("sync")
        b = _g10_machine(self.box, "g10-moved-b", init=False)
        self.assertEqual(a.run("claim", x, BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
        self.assertEqual(b.run("takeover", x, "--reason", "a looked dead").returncode, 0)
        retry = a.run("claim", x)
        owner = a.show(x)["claim"]["owner"]
        self.assertTrue(owner.startswith("codex:g10-moved-b-thread"), owner)
        self.assertNotEqual((retry.returncode, retry.stdout.strip()[:7]), (0, "Claimed"),
                            f"told the agent it holds {x}, which {owner} holds: {retry.stdout.strip()}")

    def test_claiming_update_retry_after_a_takeover_does_not_succeed(self):
        """G10: a retried `update X --claim --add-label mine` whose claim was since taken over by B does
        not exit 0 as if A held X."""
        a = _g10_machine(self.box, "g10-update")
        x = a.bead("create", "X")["id"]
        a.bead("sync")
        b = _g10_machine(self.box, "g10-update-b", init=False)
        crashed = a.run("update", x, "--claim", "--add-label", "mine", BEAD_TEST_CRASH_AFTER_PUSH="1")
        self.assertEqual(crashed.returncode, 99)
        self.assertEqual(b.run("takeover", x, "--reason", "a looked dead").returncode, 0)
        retry = a.run("update", x, "--claim", "--add-label", "mine")
        owner = a.show(x)["claim"]["owner"]
        self.assertTrue(owner.startswith("codex:g10-update-b-thread"), owner)
        self.assertNotEqual(retry.returncode, 0,
                            f"claiming update exited 0 while {owner} holds {x}: {retry.stdout.strip()}")

    def test_takeover_retry_after_the_claim_ended_is_not_a_traceback(self):
        """G10: a retried `takeover X` whose accepted takeover has since ended (B closed X with --force)
        answers with an error, not a KeyError traceback."""
        a = _g10_machine(self.box, "g10-ended")
        x = a.bead("create", "X")["id"]
        a.bead("sync")
        b = _g10_machine(self.box, "g10-ended-b", init=False)
        b.bead("claim", x)
        a.bead("sync")
        self.assertEqual(a.run("takeover", x, "--reason", "idle", BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
        b.bead("sync")
        self.assertEqual(b.run("close", x, "--force").returncode, 0)
        retry = a.run("takeover", x, "--reason", "idle")
        self.assertNotIn("Traceback", retry.stderr)
        self.assertNotEqual(retry.returncode, 0, retry.stdout)


def _g11_install_post_receive(box, gate):
    """post-receive runs after the ref update and after receive-pack sent its status report. On its first run it
    prints a line that looks like a refusal (the client relays it as `remote: ...`) and kills receive-pack, so the
    client's push exits 1 although the remote applied the update."""
    hook = box.origin / "hooks" / "post-receive"
    hook.write_text("#!/bin/sh\ncat >/dev/null\n"
                    f"if mkdir '{gate}' 2>/dev/null; then\n"
                    "  echo 'mirror: ! [rejected]  refs/beads/journal -> refs/beads/journal (fetch first)' >&2\n"
                    "  sleep 0.2\n  kill -9 $PPID\nfi\nexit 0\n")
    hook.chmod(0o755)

def _g11_remote_head(box):
    return subprocess.run(["git", "--git-dir", str(box.origin), "rev-parse", "refs/beads/journal"],
                          capture_output=True, text=True).stdout.strip()

class G11_ServerHookTextDoesNotDecideThePushOutcome(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "g11")
        self.a.bead("init", "--prefix", "ex")
        self.key = self.a.bead("create", "one")["id"]
        self.a.bead("sync")

    def test_applied_release_is_not_reported_not_owner_because_a_hook_printed_rejected(self):
        """G11: Link.publish classifies a failed push by searching all of git's stderr, including the `remote:`
        lines a server hook prints. A push the remote applied, whose connection then drops after a post-receive
        hook printed '[rejected] ... (fetch first)', is taken as refused: the attempt and its receipt are dropped,
        the command is rebuilt, and the accepted release answers NOT_OWNER."""
        self.a.bead("claim", self.key)
        gate = self.box.root / "g11-hook-fired"
        _g11_install_post_receive(self.box, gate)
        before = _g11_remote_head(self.box)
        released = self.a.run("release", self.key, "--json")
        self.assertTrue(gate.exists(), "the post-receive hook did not run")
        self.assertNotEqual(_g11_remote_head(self.box), before, "the release did not reach the remote")
        self.assertNotIn("claim", self.a.show(self.key), "the release was not applied")
        self.assertEqual(released.returncode, 0,
                         "the remote applied the release, but bead reported: " + released.stderr.strip())

    def test_applied_claiming_edit_is_not_rebuilt_over_a_later_observed_edit(self):
        """G11: the same misclassification loses an update. The accepted `update --claim --title` is dropped as
        refused and rebuilt after the retry sleep; meanwhile another machine saw the accepted title and changed
        it, and the rebuilt attempt sets the old title again on top of that later edit."""
        b = support.Machine(self.box, "g11b")
        b.bead("sync")
        env = mock.patch.dict(os.environ, self.a.env(BEAD_WORKSPACE=self.a.store().name), clear=True)
        env.start()
        self.addCleanup(env.stop)
        bp = support.load_module()
        ws = bp.Workspace(self.a.store().name, None, str(self.box.origin), {})
        actor = {"agent": "codex", "thread": "g11-thread-0001"}
        gate = self.box.root / "g11-hook-fired"
        _g11_install_post_receive(self.box, gate)
        later = []

        def between_retries(_delay):
            if not later:
                later.append(True)
                b.bead("sync")
                self.assertEqual(b.show(self.key)["title"], "accepted first", "b did not see the accepted edit")
                b.bead("update", self.key, "--title", "later observed edit")
                b.bead("sync")

        args = bp.build_parser().parse_args(["update", self.key, "--claim", "--title", "accepted first"])
        with mock.patch.object(bp.time, "sleep", between_retries):
            try:
                bp.cmd_update(args, ws, actor)
            except bp.BpError as error:
                self.fail(f"the remote accepted the update, but bead reported {error.code}: {error}")
        self.assertTrue(gate.exists(), "the post-receive hook did not run")
        b.bead("sync")
        self.a.bead("sync")
        sets = [e for f in sorted((self.a.store() / "events").rglob("*.json*"))
                for e in map(json.loads, filter(str.strip, f.read_text().splitlines()))
                if e["op"] == "set" and e.get("data", {}).get("fields", {}).get("title") == "accepted first"]
        self.assertEqual(len(sets), 1, "the accepted edit was dropped as refused and emitted a second time")
        expected = "later observed edit" if later else "accepted first"
        self.assertEqual(self.a.show(self.key)["title"], expected,
                         "the rebuilt edit overwrote a later edit that had seen the accepted one")


def _g12_push_before_script():
    """A driver that runs bead and exits 98 right before its first git push (the sweep's `push-before`
    fault): the command's writes are recorded locally and nothing has been published."""
    return r'''
import os, sys, runpy
ns = runpy.run_path(sys.argv[1])  # the launcher imports the package; every module holding run_git is patched
mods = [m for n, m in list(sys.modules.items()) if n.startswith("beadroll") and hasattr(m, "run_git")]
real_run_git = mods[0].run_git
def run_git(args, **kw):
    if args and args[0] == "push":
        os._exit(98)
    return real_run_git(args, **kw)
for m in mods:
    m.run_git = run_git
raise SystemExit(ns["main"](sys.argv[2:]))
'''

def _g12_wrapped(machine, wrapper, *argv):
    """Run bead under the push-before driver."""
    return subprocess.run([sys.executable, str(wrapper), support.BEAD, *argv], cwd=machine.checkout,
                          env=machine.env(), capture_output=True, text=True)

def _g12_seen(machine, key):
    """(status, owner) of an issue as a machine sees it."""
    issue = machine.show(key)
    return issue["status"], (issue.get("claim") or {}).get("owner")

class G12_RetriedEndOfClaimIsPublished(unittest.TestCase):
    """G12: the owner's close/delete records the end of its claim locally, then publishes it at once
    (publish_soon). If the process dies before that push, the retry finds no claim held any more,
    so `ended` stays False: it answers success and never publishes; other machines keep seeing the
    issue claimed until some later, unrelated sync."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a, self.b = support.Machine(self.box, "a"), support.Machine(self.box, "b")
        self.a.bead("init", "--prefix", "gc")
        self.key = self.a.bead("create", "work")["id"]
        self.a.bead("sync")
        self.b.bead("sync")
        self.a.bead("claim", self.key)
        self.b.bead("sync")
        self.assertIn("claim", self.b.show(self.key))
        self.wrapper = self.box.root / "fault.py"
        self.wrapper.write_text(_g12_push_before_script())

    def crash_then_retry(self, *argv):
        crashed = _g12_wrapped(self.a, self.wrapper, *argv, "--json")
        self.assertEqual(crashed.returncode, 98, f"the wrapper did not stop before the push: {crashed.stderr}")
        self.b.bead("sync")
        self.assertEqual(_g12_seen(self.b, self.key)[0], "in_progress", "the crashed run published after all")
        retry = self.a.run(*argv, "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.b.bead("sync")
        return _g12_seen(self.b, self.key)

    def test_g12_retried_close_publishes_the_end_of_the_claim(self):
        """G12: the owner's close died before its publish; the retry answers success, so it must publish."""
        seen = self.crash_then_retry("close", self.key, "--reason", "done")
        self.assertEqual(seen, ("closed", None),
                         "the retried close answered success, yet the other machine still sees the issue "
                         f"{seen[0]} and claimed by {seen[1]}: the end of the claim was never published")

    def test_g12_retried_delete_publishes_the_end_of_the_claim(self):
        """G12: the owner's delete died before its publish; the retry answers success, so it must publish."""
        seen = self.crash_then_retry("delete", self.key, "--reason", "gone")
        self.assertEqual(seen, ("tombstone", None),
                         "the retried delete answered success, yet the other machine still sees the issue "
                         f"{seen[0]} and claimed by {seen[1]}: the end of the claim was never published")

    def test_g12_retried_update_status_closed_publishes_the_end_of_the_claim(self):
        """G12: the same for `update --status closed`, which makes the transition `close` makes."""
        seen = self.crash_then_retry("update", self.key, "--status", "closed")
        self.assertEqual(seen, ("closed", None),
                         "the retried update --status closed answered success, yet the other machine still sees "
                         "the issue "
                         f"{seen[0]} and claimed by {seen[1]}: the end of the claim was never published")


def _g13_locale(name):
    return dict(LC_ALL=name, LANG=name, PYTHONUTF8="0", PYTHONCOERCECLOCALE="0")

def _g13_text_encoding(machine, env):
    """The encoding the interpreter that runs bead (python3 on bead's PATH) uses for text files under env."""
    probe = subprocess.run(["python3", "-c", "import locale; print(locale.getpreferredencoding(False))"],
                           env=machine.env(**env), capture_output=True, text=True)
    return probe.stdout.strip().lower().replace("-", "").replace("_", "") if probe.returncode == 0 else ""

class G13_TrackedConfigIsUtf8WhateverTheLocale(unittest.TestCase):
    """The same committed .beadroll.json bytes must name one coordination ref under every locale."""

    LATIN1 = _g13_locale("en_US.ISO8859-1")
    ASCII = _g13_locale("en_US.US-ASCII")

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        config = json.dumps({"workspace": "ws-g13-0001", "ref": "refs/beads/équipe"}, ensure_ascii=False).encode()
        self.a, self.b = support.Machine(self.box, "g13-a"), support.Machine(self.box, "g13-b")
        for machine in (self.a, self.b):
            (machine.checkout / ".beadroll.json").write_bytes(config)  # the same committed bytes in both clones
        self.a.bead("init", "--prefix", "ex")
        self.key = self.a.bead("create", "shared work")["id"]
        self.a.bead("sync")
        self.b.bead("sync")

    def _needs(self, env, encoding):
        found = _g13_text_encoding(self.b, env)
        if found != encoding:
            self.skipTest(f"python3 does not decode text as {encoding} under {env['LC_ALL']} here (got {found!r})")

    def test_latin1_session_resolves_the_committed_ref(self):
        """G13: under an ISO-8859-1 locale bead decodes the UTF-8 .beadroll.json as Latin-1 and uses another ref."""
        self._needs(self.LATIN1, "iso88591")
        info = self.b.run("info", "--json", **self.LATIN1)
        self.assertEqual(info.returncode, 0, info.stderr)
        self.assertEqual(json.loads(info.stdout)["ref"], "refs/beads/équipe",
                         "the committed ref was decoded in the locale's encoding")

    def test_latin1_claim_excludes_a_utf8_claim(self):
        """G13: a claim made under ISO-8859-1 lands on a second ref, so another agent's UTF-8 claim also succeeds."""
        self._needs(self.LATIN1, "iso88591")
        first = self.b.run("claim", self.key, "--json", thread="g13-b-agent", **self.LATIN1)
        self.assertEqual(first.returncode, 0, first.stderr)
        second = self.a.run("claim", self.key, "--json", thread="g13-a-agent")
        refs = support.git("for-each-ref", "--format=%(refname)", "refs/beads", cwd=self.box.origin).split()
        self.assertNotEqual(second.returncode, 0,
                            f"two agents both hold a confirmed claim on {self.key}; the remote has refs {refs}")
        self.assertEqual(refs, ["refs/beads/équipe"], "one tracker, one coordination ref")

    def test_ascii_locale_reads_a_valid_utf8_config(self):
        """G13: under a US-ASCII locale a valid UTF-8 .beadroll.json is rejected as 'not valid JSON'."""
        self._needs(self.ASCII, "usascii")
        info = self.b.run("info", "--json", **self.ASCII)
        self.assertEqual(info.returncode, 0, f"a valid UTF-8 config was refused under US-ASCII: {info.stderr}")
        self.assertEqual(json.loads(info.stdout)["ref"], "refs/beads/équipe")


def _g14_record(key, *depends_on):
    rec = {"id": key, "title": key.upper(), "status": "open", "priority": 2, "issue_type": "task",
           "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}
    if depends_on:
        rec["dependencies"] = [{"issue_id": key, "depends_on_id": d, "type": "blocks"} for d in depends_on]
    return rec

def _g14_write(box, name, *records):
    path = box.root / f"{name}.jsonl"
    path.write_text("".join(json.dumps(r) + "\n" for r in records))
    return str(path)

def _g14_edges(machine, key):
    return sorted((d["id"], d["dependency_type"]) for d in machine.show(key).get("dependencies", []))

class G14_KeyRetractionLostToConcurrentTarget(unittest.TestCase):
    """A key-based dep_remove (the target is not an issue on the remover's machine) only filters
    `dangling`. When another machine's concurrent import of the target has the earlier event id, replay
    has already turned the entry into an edge, so the retraction is lost and the edge comes back."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def _g14_pair(self):
        m1 = support.Machine(self.box, "m1")
        m1.bead("init", "--prefix", "ex")
        m1.bead("import", _g14_write(self.box, "a", _g14_record("ex-a", "ex-b")))
        m1.bead("sync")
        m2 = support.Machine(self.box, "m2")
        m2.bead("sync")
        return m1, m2

    def _g14_settle(self, *machines):
        for machine in (*machines, *machines):
            machine.bead("sync")
        self.assertEqual(len({m.export() for m in machines}), 1, "machines did not converge")

    def test_dep_remove_of_a_dangling_key_survives_a_concurrent_target_import(self):
        """G14: `dep remove ex-a ex-b` of an imported dependency on a key that is not an issue yet stays
        removed after sync even when another machine's concurrent import of ex-b has the earlier id."""
        m1, m2 = self._g14_pair()
        m2.bead("import", _g14_write(self.box, "b", _g14_record("ex-b", "ex-a")))  # earlier id: sorts first
        m1.bead("dep", "remove", "ex-a", "ex-b")  # m1 has not seen ex-b: the removal is key-based
        self.assertEqual(_g14_edges(m1, "ex-a"), [])
        self._g14_settle(m1, m2)
        self.assertEqual(_g14_edges(m1, "ex-a"), [],
                         "the dependency m1 took back came back once the concurrent import of ex-b was merged")
        self.assertIn("ex-a", m1.ready())
        self.assertEqual(m1.bead("dep", "cycles")["count"], 0)

    def test_reimport_that_drops_the_dependency_survives_a_concurrent_target_import(self):
        """G14: a re-import that no longer lists the dangling dependency (a key-based retraction) stays
        removed after sync even when a concurrent import of the target sorts first."""
        m1, m2 = self._g14_pair()
        m2.bead("import", _g14_write(self.box, "b", _g14_record("ex-b")))
        m1.bead("import", _g14_write(self.box, "cleared", _g14_record("ex-a")))
        self._g14_settle(m1, m2)
        self.assertEqual(_g14_edges(m2, "ex-a"), [],
                         "the re-import's retraction was undone by the concurrent import of its target")

    def test_replay_does_not_depend_on_the_tie_break_between_the_two_concurrent_events(self):
        """G14: replay of {import ex-a (dangling ex-b), key dep_remove, concurrent create of ex-b} gives
        the same edges whichever of the two concurrent events has the smaller id; a concurrent explicit
        `dep add` that had not seen the removal still keeps its edge."""
        bead = support.load_module()
        init, a = "01900000-0000-7000-8000-000000000000", "01900000-0000-7000-8000-000000000001"
        early, late = "01900000-0000-7000-8000-000000000002", "01900000-0000-7000-8000-000000000003"
        fields = {"title": "t", "status": "open", "priority": 2, "issue_type": "task"}

        def project(remove_id, create_b_id):
            journal = bead.Journal()
            for event in (
                {"v": 1, "id": init, "replica": "m1", "deps": [], "op": "init", "data": {"prefix": "ex", "name": "ex"}},
                {"v": 1, "id": a, "replica": "m1", "deps": [init], "op": "create", "issue": a,
                 "data": {"key": "ex-a", "fields": fields, "imported": True,
                          "dangling": [{"depends_on_id": "ex-b", "type": "blocks"}]}},
                {"v": 1, "id": remove_id, "replica": "m1", "deps": [a], "op": "dep_remove", "issue": a,
                 "data": {"key": "ex-b", "type": None}},
                {"v": 1, "id": create_b_id, "replica": "m2", "deps": [a], "op": "create", "issue": create_b_id,
                 "data": {"key": "ex-b", "fields": fields, "imported": True}},
            ):
                journal.add(event, event["replica"])
            state = bead.replay(journal)
            return sorted(state.issues[a]["deps"]), bead.is_blocked(state, state.issues[a])

        self.assertEqual(project(early, late), ([], False))
        self.assertEqual(project(late, early), ([], False),
                         "the key-based removal lost because the target's create had the smaller id")
        # Only the import's own tag is taken back: a concurrent explicit `dep add ex-a ex-b` by a writer
        # that had not seen the removal keeps its edge (add-wins).
        add = "01900000-0000-7000-8000-000000000004"
        journal = bead.Journal()
        for event in (
            {"v": 1, "id": init, "replica": "m1", "deps": [], "op": "init", "data": {"prefix": "ex", "name": "ex"}},
            {"v": 1, "id": a, "replica": "m1", "deps": [init], "op": "create", "issue": a,
             "data": {"key": "ex-a", "fields": fields, "imported": True,
                      "dangling": [{"depends_on_id": "ex-b", "type": "blocks"}]}},
            {"v": 1, "id": early, "replica": "m2", "deps": [a], "op": "create", "issue": early,
             "data": {"key": "ex-b", "fields": fields, "imported": True}},
            {"v": 1, "id": add, "replica": "m2", "deps": [early], "op": "dep_add", "issue": a,
             "data": {"to": early, "type": "blocks"}},
            {"v": 1, "id": late, "replica": "m1", "deps": [a], "op": "dep_remove", "issue": a,
             "data": {"key": "ex-b", "type": None}},
        ):
            journal.add(event, event["replica"])
        self.assertEqual(sorted(bead.replay(journal).issues[a]["deps"]), [(early, "blocks")],
                         "a concurrent explicit dep add was removed by a key-based retraction it had not seen")


def _g15_record(key, title, deps=(), kind="task"):
    return json.dumps({"id": key, "title": title, "status": "open", "priority": 2, "issue_type": kind,
                       "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
                       "created_by": "seed",
                       "dependencies": [{"issue_id": key, "depends_on_id": d, "type": t} for d, t in deps]})

def _g15_titles(machine, key):
    return [d["title"] for d in machine.show(key)["dependencies"]]

class G15_NewKeyCapturesPendingImportedDependency(unittest.TestCase):
    """mint_key (create --parent) and State.free_key (the rename of a key two machines minted
    concurrently) avoid only keys that are issues, not keys an imported dependency is still waiting
    for. A new or renamed issue can take such a key; the imported dependency then binds to that
    unrelated issue, and the genuine record imported later is skipped as already known."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_local_child_key_does_not_capture_an_imported_dependency(self):
        """G15: `create --parent ex-p` does not mint ex-p.2 while an imported dependency waits for ex-p.2;
        the genuine ex-p.2 imported next is imported and is what ex-r depends on."""
        a = support.Machine(self.box, "a")
        a.bead("init", "--prefix", "ex")
        first, second = self.box.root / "first.jsonl", self.box.root / "second.jsonl"
        first.write_text("\n".join([_g15_record("ex-p", "parent", kind="epic"),
                                    _g15_record("ex-p.1", "imported child one", [("ex-p", "parent-child")]),
                                    _g15_record("ex-r", "waits for ex-p.2", [("ex-p.2", "blocks")])]) + "\n")
        second.write_text(_g15_record("ex-p.2", "imported child two", [("ex-p", "parent-child")]) + "\n")
        a.bead("import", str(first))
        local = a.bead("create", "unrelated local child", "--parent", "ex-p")["id"]
        self.assertNotEqual(local, "ex-p.2", "create --parent minted ex-p.2, the key an imported dependency waits for")
        self.assertEqual(_g15_titles(a, "ex-r"), [])
        self.assertEqual(a.bead("import", str(second))["imported"], 1, "the genuine ex-p.2 record was skipped")
        self.assertEqual(_g15_titles(a, "ex-r"), ["imported child two"])
        a.bead("close", local, "--reason", "done")
        self.assertNotIn("ex-r", a.ready())

    def test_merge_rename_does_not_capture_an_imported_dependency(self):
        """G15: when two machines both mint ex-p.1 and replay renames one of them, the rename skips ex-p.2,
        which an imported dependency (already applied in replay order) is waiting for."""
        m1, m2, m3 = (support.Machine(self.box, name) for name in ("m1", "m2", "m3"))
        m1.bead("init", "--prefix", "ex")
        epic = self.box.root / "epic.jsonl"
        epic.write_text(_g15_record("ex-p", "parent epic", kind="epic") + "\n")
        m1.bead("import", str(epic))
        for machine in (m1, m2, m3):
            machine.bead("sync")
        waits = self.box.root / "waits.jsonl"
        waits.write_text(_g15_record("ex-r", "waits for ex-p.2", [("ex-p.2", "blocks")]) + "\n")
        m3.bead("import", str(waits))  # first by clock: applied before both children in replay order
        self.assertEqual(m1.bead("create", "machine 1 child", "--parent", "ex-p")["id"], "ex-p.1")
        self.assertEqual(m2.bead("create", "machine 2 child", "--parent", "ex-p")["id"], "ex-p.1")
        for machine in (m3, m1, m2, m3, m1, m2):
            machine.bead("sync")
        self.assertEqual(len({m.export() for m in (m1, m2, m3)}), 1, "machines did not converge")
        for machine in (m1, m2, m3):
            self.assertEqual(_g15_titles(machine, "ex-r"), [],
                             f"{machine.name}: the renamed child took ex-p.2 and captured ex-r's imported dependency")
            self.assertFalse(machine.show("ex-r")["blocked"], f"{machine.name}: ex-r is blocked by an unrelated child")


class G16_ConcurrentProcessesShareNoFetchDestination(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_concurrent_syncs_and_claims_on_one_machine_do_not_fail_on_the_shared_remote_ref(self):
        """G16: every bead process of a store fetches into the one refs/beads/remote, and git fetch updates it
        only if it still holds the value that fetch read first. Concurrent syncs and claims on one machine (agents
        in several worktrees share one store) fail with REMOTE_UNREACHABLE 'cannot reach the coordination remote
        ... cannot lock ref refs/beads/remote', although the remote was reachable and nothing contended on it."""
        a, b = support.Machine(self.box, "g16a"), support.Machine(self.box, "g16b")
        a.bead("init", "--prefix", "ex")
        keys = [a.bead("create", f"issue {n}")["id"] for n in range(6)]
        a.bead("sync")
        b.bead("sync")
        failures = []
        for round_no in range(3):
            b.bead("create", f"round {round_no}")
            b.bead("sync")  # the remote moved: every fetch below updates the local copy of the remote head
            argvs = [["sync"]] * 5 + [["claim", keys[2 * round_no]], ["claim", keys[2 * round_no + 1]]]
            procs = [subprocess.Popen([support.BEAD, *argv, "--json"], cwd=a.checkout, env=a.env(thread=f"g16-t{i}"),
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                     for i, argv in enumerate(argvs)]
            for argv, proc in zip(argvs, procs):
                _, err = proc.communicate(timeout=60)
                if proc.returncode:
                    failures.append(f"{argv[0]}: {json.loads(err)['error']['message']}")
        self.assertEqual(failures, [], f"{len(failures)} of 21 concurrent commands on one machine failed")
        self.assertEqual(a.export(), (b.bead("sync"), b.export())[1])


def _g17_translated_locale(box):
    """A locale in which this git prints a translated "couldn't find remote ref", or None."""
    for locale in ("de_DE.UTF-8", "fr_FR.UTF-8"):
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(LANG=locale, LC_ALL=locale, LANGUAGE="", GIT_CONFIG_NOSYSTEM="1",
                   GIT_CONFIG_GLOBAL=os.devnull)
        probe = subprocess.run(["git", "--git-dir", str(box.origin), "fetch", str(box.origin), "refs/beads/none"],
                               cwd=box.root, env=env, capture_output=True, text=True)
        if probe.returncode and probe.stderr.strip() and "find remote ref" not in probe.stderr:
            return locale
    return None

class G17_TranslatedGit(unittest.TestCase):
    def test_first_sync_and_claim_work_under_a_translated_git(self):
        """G17: Link.fetch recognises a not-yet-published journal by git's English "couldn't find remote ref";
        under a translated git (LANG/LC_ALL=de_DE.UTF-8 or fr_FR.UTF-8) the first sync of every new tracker
        fails with REMOTE_UNREACHABLE, and so does every claim, until some other machine has published."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        locale = _g17_translated_locale(box)
        if not locale:
            self.skipTest("this git has no German or French messages")
        a = support.Machine(box, "a")
        a.extra.update(LANG=locale, LC_ALL=locale, LANGUAGE="")
        a.bead("init", "--prefix", "lo", "--offline")  # kept local, so the sync below is the push under this locale
        key = a.bead("create", "x", "--offline")["id"]
        first = a.run("sync", "--json")
        self.assertEqual(first.returncode, 0, f"first sync under {locale}: {first.stderr}")
        self.assertGreater(json.loads(first.stdout)["pushed"], 0, first.stdout)
        claim = a.run("claim", key, "--json")
        self.assertEqual(claim.returncode, 0, f"claim under {locale}: {claim.stderr}")


def _g18_read_only_ssh():
    """A transport that serves fetches and refuses every push the way a read-only deploy key does."""
    return ("#!/bin/sh\n"
            "if [ \"$1\" = \"-G\" ]; then exit 1; fi\n"
            "for last; do :; done\n"
            "case \"$last\" in\n"
            "  git-receive-pack*) echo 'ERROR: The key you are authenticating with has been marked as read only.' >&2;"
            " exit 1 ;;\n"
            "  git-upload-pack*) eval \"exec $last\" ;;\n"
            "esac\nexit 2\n")

def _g18_error(proc):
    return json.loads(proc.stderr)["error"]

class G18_DefiniteRefusalIsNeitherARaceNorNoAnswer(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.config = self.box.root / "g18-empty.gitconfig"
        self.config.write_text("")

    def test_policy_refusal_is_reported_at_once_with_the_remotes_reason(self):
        """G18: a push the remote refuses by policy (a declining pre-receive hook, '[remote rejected]') is
        treated as a lost race: twelve rounds, about twenty seconds, then SYNC_CONFLICT 'the remote journal
        kept moving' without the remote's reason."""
        a = support.Machine(self.box, "g18p")
        a.extra.update(GIT_CONFIG_GLOBAL=str(self.config), GIT_CONFIG_NOSYSTEM="1")
        a.bead("init", "--prefix", "po")
        key = a.bead("create", "x", "--offline")["id"]  # local: the sync below is the push the hook refuses
        hook = self.box.origin / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\necho 'refs/beads/* may not be pushed here' >&2\nexit 1\n")
        hook.chmod(0o755)
        for argv in (["sync"], ["claim", key]):
            started = time.monotonic()
            proc = a.run(*argv, "--json")
            elapsed = time.monotonic() - started
            self.assertEqual(proc.returncode, 1, proc.stdout)
            error = _g18_error(proc)
            self.assertNotEqual(error["code"], "SYNC_CONFLICT",
                                f"{argv[0]}: a refusal by the remote was retried as a race: {error} after {elapsed:.1f}s")
            self.assertIn("may not be pushed here", error["message"], f"{argv[0]} hid the remote's reason")
            self.assertLess(elapsed, 5, f"{argv[0]} kept retrying a refusal")
        self.assertNotIn("claim", a.show(key))
        hook.unlink()
        self.assertEqual(a.run("claim", key, "--json").returncode, 0, "the refused claim left the issue unclaimable")

    def test_transport_refusal_reports_gits_reason_not_no_answer(self):
        """G18: a push refused before any ref status (here a read-only ssh key; also HTTP 403, a pre-push hook)
        is reported as 'the push got no answer' / 'the remote is not answering', and git's reason is thrown
        away."""
        writer = support.Machine(self.box, "g18w")
        writer.extra.update(GIT_CONFIG_GLOBAL=str(self.config), GIT_CONFIG_NOSYSTEM="1")
        writer.bead("init", "--prefix", "ex")
        key = writer.bead("create", "one")["id"]
        writer.bead("sync")
        ssh = self.box.root / "g18-read-only-ssh.sh"
        ssh.write_text(_g18_read_only_ssh())
        ssh.chmod(0o755)
        reader = support.Machine(self.box, "g18r")
        reader.extra.update(GIT_CONFIG_GLOBAL=str(self.config), GIT_CONFIG_NOSYSTEM="1", GIT_SSH_COMMAND=str(ssh),
                            BEAD_REMOTE=f"ssh://fakehost{self.box.origin}")
        reader.bead("sync")  # reading works: only pushing is refused
        reader.bead("comments", "add", key, "a note from a read-only clone")
        for argv in (["sync"], ["claim", key]):
            started = time.monotonic()
            proc = reader.run(*argv, "--json")
            self.assertEqual(proc.returncode, 1, proc.stdout)
            self.assertLess(time.monotonic() - started, 10)
            message = _g18_error(proc)["message"]
            self.assertIn("marked as read only", message, f"{argv[0]} hid git's reason: {message}")
            self.assertNotRegex(message, "no answer|not answering", f"{argv[0]} says the remote did not answer")
        self.assertNotIn("claim", writer.show(key))


def _g19_git(*args, cwd=None, env=None):
    proc = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
                          cwd=cwd, env=env, capture_output=True, text=True)
    if proc.returncode:
        raise AssertionError(f"git {' '.join(args)}: {proc.stderr}")
    return proc.stdout.strip()

class G19_ClientHooksOnPrivatePushes(unittest.TestCase):
    """G19: bead's private journal pushes run the user's client hooks: `git init` copies init.templateDir's
    hooks into $BEAD_HOME/<ws>/sync.git (where they stay), and a global core.hooksPath or a config-based
    hook applies to the push. A hook that fails there makes every push 'uncertain', so every claim ends
    UNRESOLVED and every sync REMOTE_UNREACHABLE ('the push got no answer'), the hook's message is never
    shown, and nothing changes until the hook is found in a directory the user does not know about."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.log = self.box.root / "hook.log"
        self.config = self.box.root / "global.gitconfig"  # a scratch global config, never the developer's
        self.config.write_text("")
        self.a = support.Machine(self.box, "a")
        self.a.extra.update(GIT_CONFIG_GLOBAL=str(self.config), GIT_CONFIG_NOSYSTEM="1")

    def hook(self, path, body):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"#!/bin/sh\necho \"$0 ran in $PWD\" >> '{self.log}'\n{body}\n")
        path.chmod(0o755)
        return path

    def ran(self):
        return self.log.read_text() if self.log.exists() else ""

    def assert_claim_and_sync_work(self, issue):
        claim = self.a.run("claim", issue, "--json")
        self.assertEqual(claim.returncode, 0, f"claim: {claim.stderr} (hook log: {self.ran()!r})")
        sync = self.a.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, f"sync: {sync.stderr} (hook log: {self.ran()!r})")
        self.assertEqual(list((self.a.store() / "pending").glob("*.json")), [], "an attempt stayed unsettled")
        self.assertEqual(self.ran(), "", "a client hook ran on bead's private push")

    def test_template_hook_is_not_copied_into_sync_git(self):
        """G19: a pre-push hook from init.templateDir lands in sync.git at first contact and keeps blocking
        every push after the user has dropped the setting."""
        template = self.box.root / "template"
        self.hook(template / "hooks" / "pre-push", "echo 'scanner: push blocked' >&2\nexit 1")
        self.config.write_text(f"[init]\n\ttemplateDir = {template}\n")
        self.a.bead("init", "--prefix", "hk")
        issue = self.a.bead("create", "one")["id"]
        self.a.run("sync", "--json")  # the first contact creates sync.git under the user's config
        self.config.write_text("")  # the user drops the template; a copy in sync.git would stay behind
        copied = self.a.store() / "sync.git" / "hooks" / "pre-push"
        self.assertFalse(copied.exists(), f"the user's template hook was copied to {copied}")
        self.assert_claim_and_sync_work(issue)

    def test_global_hook_written_for_code_pushes_does_not_block_claims(self):
        """G19: a global core.hooksPath pre-push hook that passes for the user's own pushes fails on bead's
        (it runs against the bare sync.git), and every claim then ends UNRESOLVED."""
        hooks = self.box.root / "hooks"
        self.hook(hooks / "pre-push", "git rev-parse --show-toplevel >/dev/null || exit 1")
        self.a.bead("init", "--prefix", "hk")
        issue = self.a.bead("create", "one")["id"]
        self.a.bead("sync")
        self.config.write_text(f"[core]\n\thooksPath = {hooks}\n")
        env = self.a.env()
        _g19_git("commit", "-q", "--allow-empty", "-m", "work", cwd=self.a.checkout, env=env)
        _g19_git("push", "-q", "origin", "HEAD:refs/heads/topic", cwd=self.a.checkout, env=env)  # passes the hook
        self.assertIn("ran in", self.ran(), "the hook did not run on the user's own push")
        self.log.unlink()
        self.assert_claim_and_sync_work(issue)

    def test_hook_already_in_an_existing_sync_git_is_not_run(self):
        """G19: a pre-push hook an earlier bead copied into the store blocks every later push."""
        self.a.bead("init", "--prefix", "hk")
        issue = self.a.bead("create", "one")["id"]
        self.a.bead("sync")
        self.hook(self.a.store() / "sync.git" / "hooks" / "pre-push", "echo 'policy: refused' >&2\nexit 1")
        self.assert_claim_and_sync_work(issue)

    def test_config_based_global_hook_does_not_block_claims(self):
        """G19: the same for a git >= 2.54 config-based hook (`[hook "x"] event = pre-push`), which
        core.hooksPath does not govern."""
        script = self.hook(self.box.root / "policy-hook", "echo 'policy: refused' >&2\nexit 1")
        text = f"[hook \"policy\"]\n\tevent = pre-push\n\tcommand = {script}\n"
        probe_config = self.box.root / "probe.gitconfig"
        probe_config.write_text(text)
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(GIT_CONFIG_GLOBAL=str(probe_config), GIT_CONFIG_NOSYSTEM="1")
        listed = subprocess.run(["git", "--git-dir", str(self.box.origin), "hook", "list", "pre-push"],
                                cwd=self.box.root, env=env, capture_output=True, text=True)
        if "policy" not in listed.stdout:
            self.skipTest("this git has no config-based hooks")
        self.a.bead("init", "--prefix", "hk")
        issue = self.a.bead("create", "one")["id"]
        self.a.bead("sync")
        self.config.write_text(text)
        self.assert_claim_and_sync_work(issue)


def _g20_config(box, text=""):
    """A scratch global git config for this sandbox (never the developer's)."""
    path = box.root / "global.gitconfig"
    path.write_text(text)
    return {"GIT_CONFIG_GLOBAL": str(path), "GIT_CONFIG_NOSYSTEM": "1"}

def _g20_git(*args, cwd=None, env=None):
    full = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    full.update(env or {})
    proc = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
                          cwd=cwd, env=full, capture_output=True, text=True)
    if proc.returncode:
        raise AssertionError(f"git {' '.join(args)}: {proc.stderr}")
    return proc.stdout.strip()

def _g20_serve(test, root, token):
    """Smart HTTP (git http-backend) for the repositories under `root` on 127.0.0.1. Every request must carry
    the Authorization header `token` exactly once and the X-Guard header exactly once; otherwise 401."""

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            self.serve()

        def do_POST(self):
            self.serve()

        def serve(self):
            if (self.headers.get_all("Authorization") or []) != [token] or len(self.headers.get_all("X-Guard") or []) != 1:
                self.send_response(401)
                self.send_header("WWW-Authenticate", 'Basic realm="g20"')
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
                body = b""
                while True:
                    size = int(self.rfile.readline().strip(), 16)
                    if not size:
                        self.rfile.readline()
                        break
                    body += self.rfile.read(size)
                    self.rfile.readline()
            else:
                body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            path, _, query = self.path.partition("?")
            env = {"PATH": os.environ["PATH"], "GIT_PROJECT_ROOT": str(root), "GIT_HTTP_EXPORT_ALL": "1",
                   "REMOTE_USER": "ci", "REQUEST_METHOD": self.command, "PATH_INFO": path, "QUERY_STRING": query,
                   "CONTENT_TYPE": self.headers.get("Content-Type", ""), "CONTENT_LENGTH": str(len(body)),
                   "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
            for header, name in (("Git-Protocol", "HTTP_GIT_PROTOCOL"), ("Content-Encoding", "HTTP_CONTENT_ENCODING")):
                if self.headers.get(header):
                    env[name] = self.headers[header]
            out = subprocess.run(["git", "http-backend"], input=body, env=env, capture_output=True).stdout
            head, _, payload = out.partition(b"\r\n\r\n")
            status, headers = 200, []
            for line in head.split(b"\r\n"):
                name, _, value = line.decode().partition(":")
                if name.lower() == "status":
                    status = int(value.split()[0])
                elif name and name.lower() != "content-length":
                    headers.append((name, value.strip()))
            self.send_response(status)
            for name, value in headers:
                self.send_header(name, value)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    test.addCleanup(server.server_close)
    test.addCleanup(server.shutdown)
    return f"http://127.0.0.1:{server.server_port}"

def _g20_fake_ssh(box):
    """`ssh` on PATH refuses (nothing leaves the machine); `user-ssh` runs the remote command locally."""
    folder = box.root / "bin"
    folder.mkdir(exist_ok=True)
    (folder / "ssh").write_text("#!/bin/sh\necho 'plain ssh: blocked in this test' >&2\nexit 255\n")
    (folder / "user-ssh").write_text('#!/bin/sh\nfor last; do :; done\nexec sh -c "$last"\n')
    for path in folder.iterdir():
        path.chmod(0o755)
    return {"PATH": f"{folder}:{os.environ['PATH']}"}

class G20_TransportSettings(unittest.TestCase):
    """G20: sync.git lives under BEAD_HOME and bead passes it only the remote URL, so the settings that let
    git reach the remote from the checkout do not apply to bead's fetch and push: the checkout's own config
    (actions/checkout keeps its token there as http.<url>.extraheader), global includes conditioned on
    `gitdir:` or `hasconfig:`, and GIT_SSH or core.sshCommand, which bead's own GIT_SSH_COMMAND default
    overrides. sync and every online claim then fail with REMOTE_UNREACHABLE where git itself works."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def assert_bead_syncs(self, machine):
        machine.bead("init", "--prefix", "tr")
        issue = machine.bead("create", "one")["id"]
        sync = machine.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, f"git reaches the remote from this checkout, bead does not: {sync.stderr}")
        claim = machine.run("claim", issue, "--json")
        self.assertEqual(claim.returncode, 0, claim.stderr)

    def test_actions_checkout_token_in_the_repository_config(self):
        """G20: a token in the checkout's own config (actions/checkout's http.<url>.extraheader) never reaches
        bead's fetch; a header the global config already sends to both is still sent once."""
        token = "Basic eC1hY2Nlc3MtdG9rZW46Z2hzX3Rlc3R0b2tlbg=="
        base = _g20_serve(self, self.box.root, token)
        guard = self.box.root / "guard.gitconfig"  # an include that applies to the checkout and to sync.git
        guard.write_text(f"[http \"{base}/\"]\n\textraheader = X-Guard: 1\n")
        genv = _g20_config(self.box, f"[includeIf \"gitdir:{self.box.root}/\"]\n\tpath = {guard}\n")
        env = dict(genv, GIT_TERMINAL_PROMPT="0")
        checkout = self.box.root / "ci"
        _g20_git("init", "-q", str(checkout), env=env)  # what actions/checkout does
        _g20_git("remote", "add", "origin", f"{base}/origin.git", cwd=checkout, env=env)
        _g20_git("config", "--local", f"http.{base}/.extraheader", f"AUTHORIZATION: {token}", cwd=checkout, env=env)
        _g20_git("fetch", "-q", "origin", "main", cwd=checkout, env=env)
        _g20_git("checkout", "-q", "-b", "main", "FETCH_HEAD", cwd=checkout, env=env)
        _g20_git("push", "-q", "--dry-run", "origin", "HEAD:refs/heads/probe", cwd=checkout, env=env)
        m = support.Machine(self.box, "ci")
        m.checkout = checkout
        m.extra.update(genv)
        self.assert_bead_syncs(m)

    def test_git_ssh_program(self):
        """G20: GIT_SSH is overridden by bead's GIT_SSH_COMMAND default."""
        genv = _g20_config(self.box)
        a = support.Machine(self.box, "a")
        a.extra.update(genv, **_g20_fake_ssh(self.box), GIT_SSH=str(self.box.root / "bin" / "user-ssh"),
                       GIT_SSH_VARIANT="simple")
        _g20_git("remote", "set-url", "origin", f"ssh://fakehost{self.box.origin}", cwd=a.checkout, env=a.env())
        _g20_git("fetch", "-q", "origin", cwd=a.checkout, env=a.env())  # git reaches the remote with GIT_SSH
        self.assert_bead_syncs(a)

    def test_core_ssh_command_in_the_repository_config(self):
        """G20: the checkout's core.sshCommand is invisible to sync.git and overridden by bead's default."""
        genv = _g20_config(self.box)
        a = support.Machine(self.box, "a")
        a.extra.update(genv, **_g20_fake_ssh(self.box), GIT_SSH_VARIANT="simple")
        _g20_git("remote", "set-url", "origin", f"ssh://fakehost{self.box.origin}", cwd=a.checkout, env=a.env())
        _g20_git("config", "--local", "core.sshCommand", str(self.box.root / "bin" / "user-ssh"), cwd=a.checkout,
                 env=a.env())
        _g20_git("fetch", "-q", "origin", cwd=a.checkout, env=a.env())
        self.assert_bead_syncs(a)

    def test_include_if_scoped_url_rewrite(self):
        """G20: a url rewrite in a global include scoped with includeIf "gitdir:" (the usual multi-account
        setup) does not apply to sync.git."""
        ssh = _g20_fake_ssh(self.box)
        work = self.box.root / "work"
        work.mkdir()
        include = self.box.root / "work.gitconfig"
        include.write_text(f"[url \"{self.box.origin}\"]\n\tinsteadOf = git@github.com:corp/proj.git\n")
        genv = _g20_config(self.box, f"[includeIf \"gitdir:{work}/\"]\n\tpath = {include}\n")
        checkout = work / "proj"
        _g20_git("clone", "-q", "git@github.com:corp/proj.git", str(checkout), env=dict(genv, **ssh))
        m = support.Machine(self.box, "w")
        m.checkout = checkout
        m.extra.update(genv, **ssh)
        self.assert_bead_syncs(m)


def _g21_nested_repository(path):
    """A repository inside the checkout (vendored code, a submodule-like directory)."""
    support.git("init", "-q", "-b", "main", str(path))
    support.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "--allow-empty", "-m", "v",
                cwd=path)

def _g21_beads_refs(path):
    return subprocess.run(["git", "for-each-ref", "refs/beads"], cwd=path, capture_output=True, text=True).stdout.strip()

class G21_RemoteResolvedAgainstTheCwd(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_relative_tracked_remote_works_from_a_subdirectory(self):
        """G21: a relative remote path in .beadroll.json is read from the top of the checkout, as git reads a
        relative remote URL, so sync works from a subdirectory too."""
        self.box.bare("tracker.git")
        machine = support.Machine(self.box, "rel")
        (machine.checkout / "src").mkdir()
        made = machine.run("init", "--prefix", "ex", "--tracked", "--remote", "../tracker.git", "--json")
        self.assertEqual(made.returncode, 0, made.stderr)
        top = machine.run("sync", "--json")
        self.assertEqual(top.returncode, 0, top.stderr)
        sub = machine.run("sync", "--json", cwd=machine.checkout / "src")
        self.assertEqual(sub.returncode, 0, f"the same tracked remote fails from src/: {sub.stderr}")

    def test_relative_remote_given_to_init_in_a_subdirectory_works_from_the_top(self):
        """G21: `init --remote ../../tracker.git` typed in src/ names a tracker beside the checkout; bead must
        store it so that sync finds that tracker from the top as well (tracked and untracked)."""
        for tracked in (True, False):
            with self.subTest(tracked=tracked):
                tracker = self.box.bare(f"tracker-{tracked}.git")
                machine = support.Machine(self.box, f"typed-{tracked}")
                (machine.checkout / "src").mkdir()
                made = machine.run("init", "--prefix", "ex", "--remote", f"../../{tracker.name}", "--json",
                                   *(["--tracked"] if tracked else []), cwd=machine.checkout / "src")
                self.assertEqual(made.returncode, 0, made.stderr)
                info = machine.run("info", "--json")
                self.assertEqual(info.returncode, 0, info.stderr)
                self.assertEqual(json.loads(info.stdout)["remote"], str(tracker))
                top = machine.run("sync", "--json")
                self.assertEqual(top.returncode, 0, f"the remote given in src/ is lost at the top: {top.stderr}")

    def test_named_remote_with_a_relative_url_works_from_a_subdirectory(self):
        """G21: a configured remote whose URL is relative (../tracker.git) is reached from a subdirectory, as git
        reaches it (`git ls-remote tr` works there)."""
        self.box.bare("tracker.git")
        machine = support.Machine(self.box, "named-rel")
        (machine.checkout / "src").mkdir()
        support.git("remote", "add", "tr", "../tracker.git", cwd=machine.checkout)
        support.git("ls-remote", "tr", cwd=machine.checkout / "src")  # git itself reaches it from src/
        self.assertEqual(machine.run("init", "--prefix", "ex").returncode, 0)
        sub = machine.run("sync", "--remote", "tr", "--json", cwd=machine.checkout / "src")
        self.assertEqual(sub.returncode, 0, sub.stderr)

    def test_configured_remote_name_wins_over_a_same_named_directory(self):
        """G21: like git, a configured remote name is that remote even when the cwd holds a directory of the
        same name; bead must not publish the journal into a repository inside the checkout."""
        machine = support.Machine(self.box, "shadow")
        team = self.box.bare("team.git")
        support.git("remote", "add", "upstream", str(team), cwd=machine.checkout)
        (machine.checkout / ".beadroll.json").write_text(json.dumps({"workspace": "named-ws", "remote": "upstream"}) + "\n")
        vendored = machine.checkout / "upstream"
        _g21_nested_repository(vendored)
        self.assertEqual(machine.run("init", "--prefix", "ex").returncode, 0)
        info = machine.run("info", "--json")
        self.assertEqual(info.returncode, 0, info.stderr)
        self.assertEqual(json.loads(info.stdout)["remote"], str(team))
        synced = machine.run("sync", "--json")
        self.assertEqual(synced.returncode, 0, synced.stderr)
        self.assertEqual(_g21_beads_refs(vendored), "", "the journal was pushed into a repository inside the checkout")
        self.assertIn("refs/beads/journal", _g21_beads_refs(team))

    def test_a_directory_named_like_the_remote_does_not_let_two_machines_claim_one_issue(self):
        """G21: with the remote shadowed at the top of one checkout, that machine's claim lands in the nested
        repository and a second machine is also granted the claim against the real remote."""
        team = self.box.bare("team.git")
        config = json.dumps({"workspace": "named-ws", "remote": "upstream"}) + "\n"
        a, b = support.Machine(self.box, "a"), support.Machine(self.box, "b")
        for machine in (a, b):
            support.git("remote", "add", "upstream", str(team), cwd=machine.checkout)
            (machine.checkout / ".beadroll.json").write_text(config)
        _g21_nested_repository(a.checkout / "upstream")
        src = a.checkout / "src"
        src.mkdir()
        self.assertEqual(a.run("init", "--prefix", "ex", cwd=src).returncode, 0)
        key = a.bead("create", "shared work", cwd=src)["id"]
        a.bead("sync", cwd=src)
        self.assertEqual(b.run("init", "--prefix", "ex").returncode, 0)
        b.bead("sync")
        first = a.run("claim", key, "--json")  # at the top of A's checkout
        self.assertEqual(first.returncode, 0, first.stderr)
        second = b.run("claim", key, "--json")
        self.assertNotEqual(second.returncode, 0,
                            "B was granted a claim A already holds: " + second.stdout.strip()[:200])
        self.assertEqual(support.code(second), "ALREADY_CLAIMED", second.stderr)


def _g22_has_journal(repo):
    return subprocess.run(["git", "--git-dir", str(repo), "rev-parse", "-q", "--verify", "refs/beads/journal"],
                          capture_output=True).returncode == 0

class G22_OriginIsNotWhereGitPushes(unittest.TestCase):
    """G22: find_workspace takes origin from `git config --get remote.origin.url`, which is the LAST url
    value and never the pushurl. bead then derives the workspace id from it and coordinates through it,
    while `git push origin` goes to the pushurl (or the first url) and `git fetch` reads the first url."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def _mirror_machine(self, name):
        """A machine that fetches from a mirror and pushes to the primary (url=mirror, pushurl=primary)."""
        mirror = self.box.root / "mirror.git"
        if not mirror.exists():
            support.git("clone", "-q", "--mirror", str(self.box.origin), str(mirror))
        m = support.Machine(self.box, name)
        support.git("remote", "set-url", "origin", str(mirror), cwd=m.checkout)
        support.git("remote", "set-url", "--push", "origin", str(self.box.origin), cwd=m.checkout)
        self.assertEqual(support.git("remote", "get-url", "--push", "origin", cwd=m.checkout).strip(),
                         str(self.box.origin))
        return m, mirror

    def test_pushurl_receives_the_tracker(self):
        """G22: with remote.origin.pushurl set, bead publishes to the fetch url (a mirror), not where
        `git push origin` goes."""
        m, mirror = self._mirror_machine("m")
        m.bead("init", "--prefix", "pu")
        m.bead("create", "one")
        sync = m.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, sync.stderr)
        self.assertTrue(_g22_has_journal(self.box.origin),
                        "the tracker was not published where `git push origin` goes (the pushurl)")
        self.assertFalse(_g22_has_journal(mirror), "the tracker was published to the fetch-only mirror")

    def test_mirror_fetching_machine_joins_the_crews_tracker(self):
        """G22: a machine with url=mirror and pushurl=primary derives another workspace id than its crew's
        direct clones and cannot join the tracker they publish on the primary."""
        a = support.Machine(self.box, "a")
        a.bead("init", "--prefix", "pu")
        issue = a.bead("create", "one")["id"]
        a.bead("sync")
        m, _ = self._mirror_machine("m")
        sync = m.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, sync.stderr)
        self.assertEqual(m.show(issue)["title"], "one")
        added = m.bead("create", "two")["id"]
        m.bead("sync")
        a.bead("sync")
        self.assertEqual(a.show(added)["title"], "two")

    def test_second_url_does_not_move_the_tracker(self):
        """G22: after `git remote set-url --add origin <backup>` (git still fetches from the first url and
        pushes code to both), bead coordinates through the last url: NOT_INITIALIZED, and following its
        advice would start a second tracker on the backup."""
        a = support.Machine(self.box, "a")
        a.bead("init", "--prefix", "pu")
        issue = a.bead("create", "one")["id"]
        a.bead("sync")
        backup = self.box.root / "backup.git"
        support.git("clone", "-q", "--bare", str(self.box.origin), str(backup))  # branches only: no tracker
        c = support.Machine(self.box, "c")
        support.git("remote", "set-url", "--add", "origin", str(backup), cwd=c.checkout)
        self.assertEqual(support.git("remote", "get-url", "origin", cwd=c.checkout).strip(), str(self.box.origin))
        self.assertEqual(c.bead("info")["remote"], str(self.box.origin))
        sync = c.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, sync.stderr)
        self.assertEqual(c.show(issue)["title"], "one")
        self.assertFalse(_g22_has_journal(backup))


def _g23_commit_and_push(checkout):
    support.git("add", ".beadroll.json", cwd=checkout)
    support.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "track bead",
                cwd=checkout)
    support.git("push", "-q", "origin", "HEAD:main", cwd=checkout)

class G23_TrackedConfigReadFromWorkingTreeOnly(unittest.TestCase):
    """G23: find_workspace reads .beadroll.json from the working tree only. A checkout whose tree predates
    the commit that added it (a worktree on an older branch, a bisect, a bare clone) falls back to the
    URL-derived workspace: NOT_INITIALIZED with 'run `bead init`', and doing so starts a second tracker."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "a")
        support.git("branch", "release-1", cwd=self.a.checkout)  # cut before bead was tracked
        support.git("push", "-q", "origin", "release-1", cwd=self.a.checkout)

    def test_worktree_on_an_older_branch_uses_the_repositorys_tracker(self):
        """G23: a worktree on a branch cut before .beadroll.json was committed, and a bare clone, resolve
        to another workspace than the repository's tracked one."""
        self.a.bead("init", "--tracked")
        _g23_commit_and_push(self.a.checkout)
        issue = self.a.bead("create", "fix the release build")["id"]
        self.a.bead("sync")
        tracked = self.a.bead("info")["workspace"]
        tree = self.box.root / "release-1"
        support.git("worktree", "add", "-q", str(tree), "release-1", cwd=self.a.checkout)
        self.assertFalse((tree / ".beadroll.json").exists())
        self.assertEqual(self.a.bead("info", cwd=tree)["workspace"], tracked,
                         "a worktree of the same repository resolves to another workspace")
        show = self.a.run("show", issue, "--json", cwd=tree)
        self.assertEqual(show.returncode, 0, show.stderr)
        claim = self.a.run("claim", issue, "--json", cwd=tree)
        self.assertEqual(claim.returncode, 0, claim.stderr)
        bare = self.box.root / "bare.git"
        support.git("clone", "-q", "--bare", str(self.box.origin), str(bare))
        self.assertEqual(self.a.bead("info", cwd=bare)["workspace"], tracked, "a bare clone resolves elsewhere")

    def test_older_branch_checkout_keeps_the_tracked_coordination_remote(self):
        """G23: with `init --tracked --remote <coordination>`, a clone on an older branch also loses the
        remote: it is told to run `bead init`, and doing so publishes a second tracker into the code origin."""
        coordination = self.box.bare("coordination.git")
        self.a.bead("init", "--prefix", "tr", "--tracked", "--remote", str(coordination))
        _g23_commit_and_push(self.a.checkout)
        issue = self.a.bead("create", "shared issue")["id"]
        self.a.bead("sync")
        c = support.Machine(self.box, "c")
        support.git("checkout", "-q", "release-1", cwd=c.checkout)
        sync = c.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, sync.stderr)
        self.assertEqual(json.loads(sync.stdout)["remote"], str(coordination.resolve()))
        self.assertEqual(c.show(issue)["title"], "shared issue")
        refs = support.git("--git-dir", str(self.box.origin), "for-each-ref", "--format=%(refname)", "refs/beads")
        self.assertEqual(refs, "", "bead published to the code origin")


class G24_RenamedRepositoryLosesItsTracker(unittest.TestCase):
    """G24: without .beadroll.json the workspace id is a hash of the origin URL (find_workspace). When the
    repository is renamed or moved, a machine that updates origin and every fresh clone under the new URL
    derive another id: the tracker their own origin holds is refused with WORKSPACE_MISMATCH, which names
    no remedy, and no command adopts it. A fix must still refuse a foreign tracker named with --remote."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "a")
        self.a.bead("init", "--prefix", "rn")
        self.issue = self.a.bead("create", "one")["id"]
        self.a.bead("sync")
        renamed = self.box.root / "renamed.git"
        self.box.origin.rename(renamed)  # the repository moves; new clones use the new URL
        self.box.origin = renamed

    def test_machine_that_updates_origin_keeps_its_tracker(self):
        """G24: after `git remote set-url origin <new URL>`, sync fails with WORKSPACE_MISMATCH against the
        machine's own origin and the machine's issues vanish from view."""
        support.git("remote", "set-url", "origin", str(self.box.origin), cwd=self.a.checkout)
        sync = self.a.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, sync.stderr)
        self.assertEqual(self.a.show(self.issue)["title"], "one")
        self.a.bead("claim", self.issue)

    def test_fresh_clone_under_the_new_url_uses_the_published_tracker(self):
        """G24: a fresh clone under the new URL cannot join the tracker its origin holds; a foreign tracker
        named with --remote must still be refused, and the refusal must name the way forward."""
        c = support.Machine(self.box, "c")
        sync = c.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, sync.stderr)
        self.assertEqual(c.show(self.issue)["title"], "one")
        self.assertEqual(c.bead("list")["total"], 1)  # later commands find it without another sync
        # guard: adoption is for this checkout's own origin only
        unrelated = self.box.clone("unrelated", self.box.bare("unrelated.git"))
        d = support.Machine(self.box, "d")
        foreign = d.run("sync", "--remote", str(self.box.origin), "--json", cwd=unrelated)
        self.assertEqual(support.code(foreign), "WORKSPACE_MISMATCH", foreign.stdout)
        self.assertIn("BEAD_WORKSPACE=", json.loads(foreign.stderr)["error"]["message"])
        self.assertEqual(support.code(d.run("list", "--json", cwd=unrelated)), "NOT_INITIALIZED")


def _g25_track(machine):
    """`init --tracked`, committed and pushed, so every clone shares the workspace id whatever its URL."""
    machine.bead("init", "--prefix", "fz", "--tracked")
    support.git("add", ".beadroll.json", cwd=machine.checkout)
    support.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "track bead",
                cwd=machine.checkout)
    support.git("push", "-q", "origin", "HEAD:main", cwd=machine.checkout)

class G25_FirstSyncRemoteIsFrozen(unittest.TestCase):
    """G25: synchronise and coordinated save every remote they used to meta.json, including origin
    itself, and Workspace.remote ranks that 'last used' value above origin. After `git remote set-url
    origin <new>` git follows the change and bead keeps using the old URL."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_set_url_is_followed(self):
        """G25: after the repository moves and origin is re-pointed, git fetch works and bead sync and
        claim fail against the URL the first sync used."""
        a = support.Machine(self.box, "a")
        _g25_track(a)
        issue = a.bead("create", "one")["id"]
        a.bead("sync")
        moved = self.box.root / "moved.git"
        shutil.move(str(self.box.origin), str(moved))
        support.git("remote", "set-url", "origin", str(moved), cwd=a.checkout)
        support.git("fetch", "-q", "origin", cwd=a.checkout)  # git follows the change
        sync = a.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, sync.stderr)
        self.assertEqual(a.bead("info")["remote"], str(moved))
        claim = a.run("claim", issue, "--json")
        self.assertEqual(claim.returncode, 0, claim.stderr)

    def test_moved_repository_does_not_split_the_crew(self):
        """G25: the repository migrates and the old one stays writable. The machine that re-pointed origin
        keeps coordinating through the old URL, fresh clones use the new one, and both claims of one
        issue are confirmed."""
        a = support.Machine(self.box, "a")
        _g25_track(a)
        issue = a.bead("create", "contested")["id"]
        a.bead("sync")
        new_home = self.box.root / "new-home.git"
        support.git("clone", "-q", "--mirror", str(self.box.origin), str(new_home))  # tracker included
        support.git("remote", "set-url", "origin", str(new_home), cwd=a.checkout)
        self.box.origin = new_home
        b = support.Machine(self.box, "b")  # a fresh clone of the new home
        b.bead("sync")
        b.bead("claim", issue)
        claim = a.run("claim", issue, "--json")
        self.assertNotEqual(claim.returncode, 0,
                            "two exclusive claims of one issue were both confirmed: " + claim.stdout[:120])
        self.assertEqual(support.code(claim), "ALREADY_CLAIMED")

    def test_origin_remembered_by_an_older_version_is_not_preferred(self):
        """G25: a meta.json remote that is origin in another spelling (https saved, origin since switched to
        ssh after a token expired) still outranks origin."""
        bp = support.load_module()
        state = self.box.root / "home-inprocess"
        env = {k: v for k, v in os.environ.items() if k != "BEAD_REMOTE"}
        env["BEAD_HOME"] = str(state)
        with mock.patch.dict(os.environ, env, clear=True):
            ws = bp.Workspace("legacy-ws", "repo", "git@example.invalid:org/repo.git", {})
            ws.dir.mkdir(parents=True)
            (ws.dir / "meta.json").write_text(json.dumps({"remote": "https://example.invalid/org/repo.git"}))
            self.assertEqual(ws.remote(), "git@example.invalid:org/repo.git",
                             "the remembered spelling of origin outranks origin itself")
            ws.save_meta(remote="git@example.invalid:org/repo.git", last_sync="2026-01-01T00:00:00Z")
            self.assertNotIn("remote", json.loads((ws.dir / "meta.json").read_text()))


def _g26_config(box, name, text=""):
    """A scratch global git config for this sandbox (never the developer's)."""
    path = box.root / name
    path.write_text(text)
    return {"GIT_CONFIG_GLOBAL": str(path), "GIT_CONFIG_NOSYSTEM": "1"}

def _g26_git(*args, cwd=None, env=None):
    full = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    full.update(env or {})
    proc = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
                          cwd=cwd, env=full, capture_output=True, text=True)
    if proc.returncode:
        raise AssertionError(f"git {' '.join(args)}: {proc.stderr}")
    return proc.stdout.strip()

class G26_ObjectFormat(unittest.TestCase):
    """G26: bead creates sync.git with a plain `git init --bare`, so its hash follows this machine's default
    (init.defaultObjectFormat, GIT_DEFAULT_HASH, Git 3's announced sha256 default), not the project's. A
    SHA-256 project, or a SHA-256 default against a SHA-1 project, can never publish: every push is
    'uncertain' (sync: REMOTE_UNREACHABLE 'the push got no answer'; claim: UNRESOLVED), and a sync.git once
    made in the wrong format stays wrong after the setting is gone."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.plain = _g26_config(self.box, "plain.gitconfig")

    def machines(self, genv):
        a, b = support.Machine(self.box, "a"), support.Machine(self.box, "b")
        a.extra.update(genv)
        b.extra.update(genv)
        return a, b

    def sync_git_format(self, machine):
        return _g26_git("--git-dir", str(machine.store() / "sync.git"), "rev-parse", "--show-object-format",
                        env=self.plain)

    def assert_published(self, a, b, issue, why):
        sync = a.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, f"{why}: {sync.stderr}")
        claim = a.run("claim", issue, "--json")
        self.assertEqual(claim.returncode, 0, f"{why}, claim: {claim.stderr}")
        b.bead("sync")
        self.assertEqual(b.show(issue)["status"], "in_progress", f"{why}: the other machine does not see the claim")

    def test_sha256_project_syncs(self):
        """G26: a SHA-256 repository gets a SHA-1 sync.git and can never publish."""
        origin = self.box.root / "origin256.git"
        _g26_git("init", "-q", "--bare", "--object-format=sha256", "-b", "main", str(origin), env=self.plain)
        a, b = self.machines(self.plain)
        for m in (a, b):
            m.checkout = self.box.root / f"{m.name}-256"
        _g26_git("clone", "-q", str(origin), str(a.checkout), env=self.plain)
        _g26_git("commit", "-q", "--allow-empty", "-m", "start", cwd=a.checkout, env=self.plain)
        _g26_git("push", "-q", "origin", "HEAD:main", cwd=a.checkout, env=self.plain)
        _g26_git("clone", "-q", str(origin), str(b.checkout), env=self.plain)
        a.bead("init", "--prefix", "of")
        issue = a.bead("create", "one")["id"]
        self.assert_published(a, b, issue, "SHA-256 project")

    def test_sha256_default_with_a_sha1_project_syncs(self):
        """G26: a machine whose default is SHA-256 (init.defaultObjectFormat) cannot publish to a SHA-1
        project, although the user's own git works there."""
        a, b = self.machines(_g26_config(self.box, "sha256.gitconfig", "[init]\n\tdefaultObjectFormat = sha256\n"))
        _g26_git("fetch", "-q", "origin", cwd=a.checkout, env=a.extra)  # the user's own git works here
        a.bead("init", "--prefix", "of")
        issue = a.bead("create", "one")["id"]
        self.assert_published(a, b, issue, "SHA-256 default, SHA-1 project")

    def poison(self, machine):
        """What a run under GIT_DEFAULT_HASH=sha256 or a SHA-256 default left: the store's sync.git in SHA-256."""
        path = machine.store() / "sync.git"
        shutil.rmtree(path, ignore_errors=True)
        _g26_git("init", "-q", "--bare", "--object-format=sha256", str(path), env=self.plain)

    def test_sync_git_made_in_the_wrong_format_is_made_again(self):
        """G26: a SHA-256 sync.git keeps a SHA-1 project's machine from publishing after the setting that
        made it is gone, before and after a journal exists on the remote."""
        a, b = self.machines(self.plain)
        a.bead("init", "--prefix", "of")
        issue = a.bead("create", "one")["id"]
        self.poison(a)
        first = a.run("sync", "--json")
        self.assertEqual(first.returncode, 0, f"no journal published yet: {first.stderr}")
        self.assertEqual(self.sync_git_format(a), "sha1")
        b.bead("sync")
        self.poison(b)
        a.bead("create", "two")
        a.bead("sync")
        sync = b.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, f"against a published journal: {sync.stderr}")
        self.assertEqual(self.sync_git_format(b), "sha1")
        self.assert_published(a, b, issue, "after a SHA-256 sync.git was left behind")


class G27_UnfinishedSyncGit(unittest.TestCase):
    """G27: Link.fetch treats sync.git as ready whenever the directory exists, and creates it with `git init`
    in the caller's raw environment. GIT_WORK_TREE at first contact makes that init fail after mkdir, an
    init killed midway leaves the same empty directory, and GIT_OBJECT_DIRECTORY sends sync.git's objects
    to the caller's directory. Every later sync and claim then fails with REMOTE_UNREACHABLE
    ('not a git repository'), documented as 'retry later', until the user deletes $BEAD_HOME/<ws>/sync.git,
    although it holds nothing that is not also in events/ or on the remote."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        config = self.box.root / "global.gitconfig"  # a scratch global config, never the developer's
        config.write_text("")
        self.a, self.b = support.Machine(self.box, "a"), support.Machine(self.box, "b")
        for m in (self.a, self.b):
            m.extra.update(GIT_CONFIG_GLOBAL=str(config), GIT_CONFIG_NOSYSTEM="1")
        self.a.bead("init", "--prefix", "wd")
        self.issue = self.a.bead("create", "one")["id"]
        self.a.bead("sync")

    def assert_b_works(self, after):
        sync = self.b.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, f"sync after {after}: {sync.stderr}")
        claim = self.b.run("claim", self.issue, "--json")
        self.assertEqual(claim.returncode, 0, f"claim after {after}: {claim.stderr}")
        self.a.bead("sync")
        self.assertEqual(self.a.show(self.issue)["status"], "in_progress", "the claim was not published")

    def test_git_work_tree_at_first_contact(self):
        """G27: GIT_WORK_TREE in the environment at first contact (Link.env strips it for every later git
        call, but not for the init) leaves an empty sync.git that wedges the store."""
        first = self.b.run("sync", "--json", GIT_WORK_TREE=str(self.b.checkout))
        self.assertEqual(first.returncode, 0, f"first sync with GIT_WORK_TREE set: {first.stderr}")
        self.assert_b_works("a first contact with GIT_WORK_TREE set")

    def test_sync_git_left_empty_by_an_interrupted_init_is_completed(self):
        """G27: an empty sync.git (what an init killed or failing after mkdir leaves) is never completed."""
        self.b.bead("sync")
        sync_git = self.b.store() / "sync.git"
        shutil.rmtree(sync_git)
        sync_git.mkdir()
        self.assert_b_works("finding sync.git empty")

    def test_caller_object_directory_never_reaches_sync_git(self):
        """G27: GIT_OBJECT_DIRECTORY at first contact puts sync.git's objects in the caller's directory and
        leaves sync.git without objects/, which every later command then fails on."""
        objects = self.box.root / "caller-objects"
        objects.mkdir()
        first = self.b.run("sync", "--json", GIT_OBJECT_DIRECTORY=str(objects))
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual([p.name for p in objects.rglob("*") if p.is_file()], [],
                         "bead wrote sync.git's objects into the caller's object directory")
        self.assert_b_works("a first contact with GIT_OBJECT_DIRECTORY set")

    def test_sync_git_without_objects_is_completed(self):
        """G27: a sync.git with HEAD but no objects/ (what an earlier bead left after a first contact with
        GIT_OBJECT_DIRECTORY set) is never completed."""
        self.b.bead("sync")
        shutil.rmtree(self.b.store() / "sync.git" / "objects")
        self.assert_b_works("finding sync.git without objects/")


def _g28_unprivileged():
    return not (hasattr(os, "geteuid") and os.geteuid() == 0)

class G28_OnlyTheLockHolderSweepsPending(unittest.TestCase):
    """G28: load_pending removes a pending attempt and its receipt whenever read_attempt returns [],
    which it also does for an OSError, and view()/info call it without the store lock. So a plain
    reader (show, list, ready, prime, export, info) deletes an accepted attempt it cannot read
    (EACCES), or one a locked sync confirmed between the reader's glob of pending/ and its read;
    the retry of the accepted command is then no longer recognised. On a read-only store, the same
    sweep makes every read fail with IO as soon as one junk pending file exists."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "a")
        self.a.bead("init", "--prefix", "gp")
        self.key = self.a.bead("create", "x")["id"]
        self.a.bead("sync")

    def lose_ack(self, argv):
        """Run a command the remote accepts, dying before it hears so: its attempt and receipt stay."""
        crashed = self.a.run(*argv, "--json", BEAD_TEST_CRASH_AFTER_PUSH="1")
        self.assertEqual(crashed.returncode, 99, crashed.stderr)
        attempts = list((self.a.store() / "pending").glob("*.json"))
        self.assertEqual(len(attempts), 1)
        self.assertTrue(attempts[0].with_suffix(".command").exists())
        return attempts[0]

    def release_with_lost_ack(self):
        key = self.a.bead("create", "held")["id"]
        self.a.bead("claim", key)
        argv = ["release", key]
        return argv, self.lose_ack(argv)

    def test_g28_reader_keeps_an_unreadable_accepted_attempt_and_its_receipt(self):
        """G28: `show` (no lock) deletes an accepted attempt it cannot read (EACCES) and its receipt."""
        if not _g28_unprivileged():
            self.skipTest("root reads a mode-000 file")
        for name in ("update --claim --status closed", "release"):
            with self.subTest(command=name):
                if name == "release":
                    argv, attempt = self.release_with_lost_ack()
                else:
                    argv = ["update", self.key, "--claim", "--status", "closed"]
                    attempt = self.lose_ack(argv)
                receipt = attempt.with_suffix(".command")
                attempt.chmod(0)
                try:
                    shown = self.a.run("show", self.key, "--json")
                finally:
                    if attempt.exists():
                        attempt.chmod(0o600)
                self.assertEqual(shown.returncode, 0, shown.stderr)
                self.assertEqual((attempt.exists(), receipt.exists()), (True, True),
                                 "a read-only command deleted an accepted attempt it could not read, and its receipt")
                retry = self.a.run(*argv, "--json")
                self.assertEqual(retry.returncode, 0, retry.stderr)

    def test_g28_reader_racing_a_sync_keeps_the_receipt_the_sync_kept(self):
        """G28: a reader between its glob of pending/ and its read, while a locked sync confirms the
        attempt (removing the pending file, keeping the receipt), deletes that receipt."""
        argv, attempt = self.release_with_lost_ack()
        receipt = attempt.with_suffix(".command")
        paused = []
        with mock.patch.dict(os.environ, self.a.env(thread="reader"), clear=True), \
                contextlib.chdir(self.a.checkout):
            bp = support.load_module()
            real = bp.pending_files

            def pending_files(ws):
                found = real(ws)
                if found and not paused:  # listed, not yet read: a locked writer runs now
                    synced = self.a.run("sync", "--json")
                    paused.append((synced.returncode, synced.stderr, attempt.exists(), receipt.exists()))
                return found

            with tempfile.TemporaryFile("w+") as out:
                with support.patched("pending_files", pending_files), contextlib.redirect_stdout(out):
                    shown = bp.run(["show", argv[1], "--json"])
                out.seek(0)
                printed = out.read()
        self.assertEqual(shown, 0, printed)
        self.assertEqual(len(paused), 1, "the reader never reached pending/")
        self.assertEqual(paused[0], (0, "", False, True), "the sync confirms the attempt and keeps its receipt")
        self.assertTrue(receipt.exists(), "a reader (show, no lock) deleted the receipt the locked sync kept")
        retry = self.a.run(*argv, "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)

    def test_g28_readers_work_on_a_read_only_store_with_a_junk_pending_file(self):
        """G28: readers try to delete a junk pending file, so every read fails with IO on a read-only store."""
        if not _g28_unprivileged():
            self.skipTest("root writes a read-only store")
        store = self.a.store()
        (store / "pending").mkdir(exist_ok=True)
        (store / "pending" / "01900000-0000-7000-8000-0000000000aa.json").write_text("{")
        subprocess.run(["chmod", "-R", "a-w", str(store)], check=True)
        try:
            results = {argv[0]: self.a.run(*argv, "--json")
                       for argv in (["show", self.key], ["list"], ["ready"], ["export"], ["info"])}
        finally:
            subprocess.run(["chmod", "-R", "u+w", str(store)], check=True)
        for name, proc in results.items():
            with self.subTest(command=name):
                self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_g28_locked_sweep_does_not_delete_an_unreadable_attempt(self):
        """G28: under the lock too, an attempt that cannot be read is not junk: `sync` must not sweep it
        (and its receipt) away; it reports IO, and once the mode is fixed the retry is recognised."""
        if not _g28_unprivileged():
            self.skipTest("root reads a mode-000 file")
        argv, attempt = self.release_with_lost_ack()
        receipt = attempt.with_suffix(".command")
        attempt.chmod(0)
        try:
            synced = self.a.run("sync", "--json")
        finally:
            if attempt.exists():
                attempt.chmod(0o600)
        self.assertEqual((attempt.exists(), receipt.exists()), (True, True),
                         f"sync (exit {synced.returncode}) swept away an accepted attempt it could not read, "
                         "and its receipt")
        self.assertEqual(json.loads(synced.stderr)["error"]["code"], "IO", synced.stderr)
        retry = self.a.run(*argv, "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)


def _g29_full_volume_script():
    """A driver that runs bead with write_atomic failing with ENOSPC, as a full volume fails it: for
    meta.json (G29_FULL=meta), or for a segment of another machine as a pull brings it in (G29_FULL=foreign)."""
    return r'''
import errno, os, sys, runpy
from pathlib import Path
ns = runpy.run_path(sys.argv[1])  # the launcher imports the package; every module holding write_atomic is patched
mods = [m for n, m in list(sys.modules.items()) if n.startswith("beadroll") and hasattr(m, "write_atomic")]
which = os.environ.pop("G29_FULL")
real_write = mods[0].write_atomic
replica_id = sys.modules["beadroll.machine"].replica_id
def write_atomic(path, data, private=True):
    path = Path(path)
    own = replica_id(create=False)
    if (which == "meta" and path.name == "meta.json") or \
            (which == "foreign" and path.suffix == ".jsonl" and path.parent.name != own):
        raise OSError(errno.ENOSPC, os.strerror(errno.ENOSPC), str(path))
    return real_write(path, data, private)
for m in mods:
    m.write_atomic = write_atomic
raise SystemExit(ns["main"](sys.argv[2:]))
'''

def _g29_seen(machine, key):
    """(status, owner) of an issue as a machine sees it."""
    issue = machine.show(key)
    return issue["status"], (issue.get("claim") or {}).get("owner")

class G29_FailureAfterAcceptanceIsNotNothingHalfDone(unittest.TestCase):
    """G29: coordinated() drops the attempt and its receipt, then writes meta.json; publish_soon
    catches BpError only. So an OSError (a full volume) after the remote accepted a command is
    reported as IO, which `bead prime` documents as "nothing half-done": the release or close has
    happened, and the receipt that would let a retry recognise it is already gone."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a, self.obs = support.Machine(self.box, "a"), support.Machine(self.box, "obs")
        self.a.bead("init", "--prefix", "gf")
        self.wrapper = self.box.root / "full_volume.py"
        self.wrapper.write_text(_g29_full_volume_script())

    def full(self, which, *argv):
        return subprocess.run([sys.executable, str(self.wrapper), support.BEAD, *argv], cwd=self.a.checkout,
                              env=self.a.env(G29_FULL=which), capture_output=True, text=True)

    def remote(self, key):
        self.obs.bead("sync")
        return _g29_seen(self.obs, key)

    def test_g29_accepted_command_is_not_reported_as_io_when_meta_cannot_be_written(self):
        """G29: the remote accepted a release / claiming close, then meta.json hit ENOSPC: not an IO failure."""
        released, closed = (self.a.bead("create", name)["id"] for name in ("released", "closed"))
        self.a.bead("sync")
        self.a.bead("claim", released)
        for name, argv, expected in (("release", ["release", released], "open"),
                                     ("update --claim --status closed",
                                      ["update", closed, "--claim", "--status", "closed"], "closed")):
            with self.subTest(command=name):
                first = self.full("meta", *argv, "--json")
                key = argv[1]
                self.assertEqual(self.remote(key), (expected, None), f"precondition: the remote has the {name}")
                retry = self.a.run(*argv, "--json") if first.returncode else None
                self.assertEqual(first.returncode, 0,
                                 f"{name} took effect on the remote, yet bead answered {first.stderr.strip()}; "
                                 f"its retry then answered {retry and (retry.returncode, retry.stderr.strip())}")

    def test_g29_recognised_retry_is_not_reported_as_io_when_meta_cannot_be_written(self):
        """G29: a retry recognises its accepted release (the receipt), then meta.json hits ENOSPC: not IO."""
        key = self.a.bead("create", "x")["id"]
        self.a.bead("sync")
        self.a.bead("claim", key)
        self.assertEqual(self.a.run("release", key, "--json", BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
        retry = self.full("meta", "release", key, "--json")
        again = self.a.run("release", key, "--json") if retry.returncode else None
        self.assertEqual(retry.returncode, 0,
                         f"the retry recognised the accepted release, yet answered {retry.stderr.strip()}; "
                         f"the next retry then answered {again and (again.returncode, again.stderr.strip())}")

    def test_g29_owner_close_whose_publish_hits_a_full_volume_is_recorded_not_failed(self):
        """G29: the owner's close is recorded; its publish hits ENOSPC pulling another machine's segment:
        the answer is "recorded locally, not yet published", not exit 1 IO."""
        key = self.a.bead("create", "x")["id"]
        self.a.bead("sync")
        self.a.bead("claim", key)
        b = support.Machine(self.box, "b")
        b.bead("sync")
        b.bead("create", "growth on b")
        b.bead("sync")
        closed = self.full("foreign", "close", key, "--reason", "done")
        self.assertEqual(_g29_seen(self.a, key), ("closed", None), "precondition: the close is recorded here")
        self.assertEqual((closed.returncode, "not yet published" in closed.stdout), (0, True),
                         f"the close was recorded, yet bead answered: {closed.stderr.strip() or closed.stdout.strip()}")


def _g30_common():
    """A Beads record's required fields."""
    return {"status": "open", "priority": 2, "issue_type": "task", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}

def _g30_foreign():
    """Nested deeper than any Python's json parser reaches."""
    return "[" * 1_000_000 + "]" * 1_000_000

def _g30_record(key, depth, field="x_state"):
    """A Beads record whose unknown field nests `depth` arrays (the record itself is one more level)."""
    head = json.dumps({"id": key, "title": "deep", **_g30_common()})[:-1]
    return head + f', "{field}": ' + "[" * depth + "]" * depth + "}\n"

def _g30_tail(text):
    lines = [line for line in text.strip().split("\n") if line.strip()]
    return lines[-1] if lines else ""

def _g30_git(*args, env, data=None):
    """Plumbing on the sandbox's bare remote (GIT_DIR), run from inside it rather than the caller's cwd."""
    return subprocess.run(["git", *args], cwd=env["GIT_DIR"], env=env, input=data, capture_output=True,
                          check=True).stdout.strip().decode()

def _g30_parses(python, depth):
    """Whether `python` can json-decode `depth` nested arrays (None when it cannot be run)."""
    code = f"import json; json.loads('[' * {depth} + ']' * {depth})"
    try:
        return subprocess.run([python, "-c", code], capture_output=True, timeout=60).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return None

def _g30_older_python():
    """A Python 3.12 or 3.13 (README: 3.12+), whose json stops near 10,000 levels, or None."""
    found = [shutil.which("python3.12"), shutil.which("python3.13")]
    found += sorted(str(p) for p in (Path.home() / ".local/share/uv/python").glob("cpython-3.1[23]*/bin/python3.1[23]"))
    for path in filter(None, found):
        if _g30_parses(path, 1000) and _g30_parses(path, 12_000) is False:
            return path
    return None

class G30_DeepJsonIsBoundedAndScoped(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.m = support.Machine(self.box, "g30")
        self.m.bead("init", "--prefix", "ex")
        self.m.bead("create", "ordinary work")

    def assert_no_traceback(self, proc, what):
        if "Traceback" in proc.stderr:
            self.fail(f"{what} crashed: {_g30_tail(proc.stderr)}")

    def assert_works(self, proc, what):
        self.assert_no_traceback(proc, what)
        self.assertEqual(proc.returncode, 0, f"{what}: {proc.stderr[-400:]}")

    def test_line_too_deep_for_json_is_one_integrity_problem(self):
        """G30: one journal line nested deeper than json can parse (foreign segment or attempt file) is
        reported as an unreadable line, not a RecursionError that takes down every command."""
        folder = self.m.store() / "events" / "fffffff0"
        (folder / "claims").mkdir(parents=True)
        (folder / "000001.jsonl").write_text(_g30_foreign() + "\n")
        for args in (("list",), ("ready",), ("info",)):
            self.assert_works(self.m.run(*args, "--json"), f"bead {args[0]} with a deep segment line")
        integrity = self.m.bead("info")["integrity"]
        self.assertTrue(any(str(row.get("origin", "")).endswith("000001.jsonl:1") for row in integrity), integrity)
        (folder / "claims" / "01a1179c-0000-7000-8000-000000000001.json").write_text(_g30_foreign() + "\n")
        self.assert_works(self.m.run("list", "--json"), "bead list with a deep attempt file")
        # an unterminated deep final line is a torn tail to repair, also on sync
        (self.m.store() / "events" / "fffffff1").mkdir()
        (self.m.store() / "events" / "fffffff1" / "000001.jsonl").write_text(_g30_foreign())
        self.assert_works(self.m.run("sync", "--json"), "bead sync with a deep unterminated tail")

    def test_deep_tracked_config_or_manifest_is_a_structured_error(self):
        """G30: a committed .beadroll.json or a published manifest.json nested too deep for json gives
        CONFIG / MANIFEST_MISSING (and help still works), not a traceback."""
        self.m.bead("sync")
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(GIT_DIR=str(self.box.origin), GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
                   GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")
        tip = _g30_git("rev-parse", "refs/beads/journal", env=env)
        blob = _g30_git("hash-object", "-w", "--stdin", env=env, data=_g30_foreign().encode())
        tree = _g30_git("mktree", env=env, data=f"100644 blob {blob}\tmanifest.json\n".encode())
        commit = _g30_git("commit-tree", tree, "-p", tip, "-m", "deep manifest", env=env)
        _g30_git("update-ref", "refs/beads/journal", commit, tip, env=env)
        sync = self.m.run("sync", "--json")
        self.assert_no_traceback(sync, "sync against a deep manifest")
        self.assertEqual(support.code(sync), "MANIFEST_MISSING")

        (self.m.checkout / ".beadroll.json").write_text('{"workspace": "ws-g30-0001", "x": ' + _g30_foreign() + "}")
        listed = self.m.run("list", "--json")
        self.assert_no_traceback(listed, "list beside a deep .beadroll.json")
        self.assertEqual(support.code(listed), "CONFIG")
        self.assert_works(self.m.run("help"), "bead help beside a deep .beadroll.json")

    def test_import_refuses_nesting_that_not_every_machine_can_read(self):
        """G30: import accepts a record nested 20,000 deep (Python 3.14 parses it, 3.12/3.13 cannot), then
        every machine replays the event; a bounded depth must be refused whole with VALIDATION."""
        for name, text in (("deep-20000", _g30_record("ex-deep", 20_000)),
                           ("deep-200", _g30_record("ex-deep200", 200)),
                           ("deep-dep-metadata", json.dumps({"id": "ex-depmeta", "title": "m", **_g30_common()})[:-1]
                            + ', "dependencies": [{"issue_id": "ex-depmeta", "depends_on_id": "ex-elsewhere", '
                            '"type": "blocks", "metadata": ' + "[" * 200 + "]" * 200 + "}]}\n")):
            with self.subTest(name):
                path = self.box.root / f"{name}.jsonl"
                path.write_text(text)
                proc = self.m.run("import", str(path), "--json")
                self.assert_no_traceback(proc, f"import of {name}")
                self.assertNotEqual(proc.returncode, 0, f"import of {name} was accepted: {proc.stdout}")
                self.assertEqual(support.code(proc), "VALIDATION")
        self.assertEqual({i["title"] for i in self.m.bead("list", "--all", "--limit", "0")["issues"]}, {"ordinary work"})
        ordinary = self.box.root / "nested-50.jsonl"
        ordinary.write_text(_g30_record("ex-nested", 50, "custom"))
        self.assertEqual(self.m.bead("import", str(ordinary))["imported"], 1)
        exported = next(json.loads(row) for row in self.m.export().split("\n") if '"ex-nested"' in row)
        self.assertEqual(json.dumps(exported["custom"]), "[" * 50 + "]" * 50)

    def test_peer_on_python_312_keeps_working_after_a_deep_import_elsewhere(self):
        """G30: machine a (python3 on PATH, 3.14) imports a 12,000-deep field and syncs; machine b runs
        bead on Python 3.12/3.13 and must keep working and see what a sees."""
        old = _g30_older_python()
        if old is None:
            self.skipTest("no Python 3.12/3.13 whose json stops below 12,000 levels")
        if not _g30_parses("python3", 12_000):
            self.skipTest("python3 on PATH (bead's shebang) parses no deeper than 3.12/3.13")
        a = self.m
        a.bead("sync")
        b = support.Machine(self.box, "g30b")

        def b_run(*args):
            return subprocess.run([old, support.BEAD, *args, "--json"], cwd=b.checkout, env=b.env(),
                                  capture_output=True, text=True)

        self.assert_works(b_run("sync"), f"{old}: first sync")
        path = self.box.root / "deep.jsonl"
        path.write_text(_g30_record("ex-deep", 12_000))
        a.run("import", str(path), "--json")  # accepted or refused: what matters is b afterwards
        a.bead("create", "made after the import")
        a.bead("sync")
        self.assert_works(b_run("sync"), f"{old}: sync after a's import")
        listed = b_run("list", "--all", "--limit", "0")
        self.assert_works(listed, f"{old}: list")
        self.assertEqual({i["id"] for i in json.loads(listed.stdout)["issues"]},
                         {i["id"] for i in a.bead("list", "--all", "--limit", "0")["issues"]})


def _g31_titles():
    """Titles holding U+2028, U+2029 and U+0085 (escapes only: raw ones would be split by line-based tools)."""
    return ["line\u2028sep", "para\u2029sep", "nel\u0085x"]

def _g31_tail(text):
    lines = [line for line in text.strip().split("\n") if line.strip()]
    return lines[-1] if lines else ""

class G31_ExportWithUnicodeLineSeparatorsImportsAgain(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_own_export_with_unicode_line_separators_imports_again(self):
        """G31: `export` writes U+2028/U+2029/U+0085 raw inside JSON strings (valid JSONL), but `import`
        splits the file with str.splitlines(), so bead cannot import its own export."""
        src = support.Machine(self.box, "g31src")
        src.bead("init", "--prefix", "ex")
        keys = {}
        for title in _g31_titles():
            keys[title] = src.bead("create", title, "-d", f"d:{title}")["id"]
            src.bead("comments", "add", keys[title], f"c:{title}")
        out = self.box.root / "export.jsonl"
        proc = src.run("export", "-o", str(out), "--json")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        raw = out.read_bytes()
        self.assertEqual(raw.count(b"\n"), 3)  # three records, three physical lines
        self.assertIn("\u2028".encode(), raw)  # written raw, which is valid JSON
        dst = support.Machine(self.box, "g31dst")
        dst.bead("init", "--prefix", "zz", "--offline")  # its own tracker: not a second machine of the source's
        proc = dst.run("import", str(out), "--offline", "--json")
        self.assertEqual(proc.returncode, 0, f"bead could not import its own export: {_g31_tail(proc.stderr)}")
        self.assertEqual(json.loads(proc.stdout)["imported"], 3)  # (--offline: the result also counts the unpublished)
        for title, key in keys.items():
            issue = dst.show(key)
            self.assertEqual(issue["title"], title)
            self.assertEqual(issue["description"], f"d:{title}")
            self.assertEqual([c["text"] for c in issue["comments"]], [f"c:{title}"])

    def test_init_import_of_a_br_export_with_a_raw_line_separator(self):
        """G31: `init --import` of a br-written record whose title holds a raw U+2028 fails as invalid JSONL.
        CRLF line ends and blank lines must keep working."""
        record = {"id": "bx-o5n", "title": "line\u2028sep", "status": "open", "priority": 2, "issue_type": "task",
                  "created_at": "2026-10-07T18:24:00Z", "updated_at": "2026-10-07T18:24:00Z"}
        other = {**record, "id": "bx-crlf", "title": "plain"}
        path = self.box.root / "issues.jsonl"
        path.write_bytes(json.dumps(record, ensure_ascii=False).encode() + b"\r\n\n"
                         + json.dumps(other).encode() + b"\r\n")
        machine = support.Machine(self.box, "g31br")
        proc = machine.run("init", "--prefix", "bx", "--import", str(path), "--json")
        self.assertEqual(proc.returncode, 0, f"init --import refused a valid br export: {_g31_tail(proc.stderr)}")
        self.assertEqual(json.loads(proc.stdout)["imported"], 2)
        self.assertEqual(machine.show("bx-o5n")["title"], "line\u2028sep")


def _g32_common():
    """A Beads record's required fields."""
    return {"status": "open", "priority": 2, "issue_type": "task", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}

def _g32_tail(text):
    lines = [line for line in text.strip().split("\n") if line.strip()]
    return lines[-1] if lines else ""

class G32_IntegerFieldsStayBeadsReadable(unittest.TestCase):
    def assert_refused(self, proc, what):
        if "Traceback" in proc.stderr:
            self.fail(f"{what} crashed: {_g32_tail(proc.stderr)}")
        self.assertNotEqual(proc.returncode, 0, f"{what} was accepted: {proc.stdout.strip()}")
        self.assertEqual(support.code(proc), "VALIDATION")

    def test_writes_refuse_booleans_and_estimates_beads_cannot_read(self):
        """G32: bead accepts a JSON boolean for priority/estimated_minutes (bool is an int) and an
        estimated_minutes of any sign or size, then exports records br refuses (`expected i32`,
        `cannot be negative`). New writes must refuse them; histories that already hold them still replay."""
        # Guard first: the stricter rule is for writes only. Events already written with such values (by
        # e9eed92 or by hand) keep their issue and their attempt, or upgraded machines diverge from old ones.
        bp = support.load_module()
        journal = bp.Journal()
        base = "01a00000-0000-7000-8000-00000000000"
        for n, fields in enumerate(({"priority": True}, {"estimated_minutes": -7}, {"estimated_minutes": 2**40})):
            eid = base + str(n)
            journal.add({"v": bp.FORMAT, "id": eid, "replica": "a", "deps": [base + str(n - 1)] if n else [],
                         "actor": {"agent": "t"}, "op": "create", "issue": eid,
                         "data": {"key": f"ex-{n}", "fields": {"title": f"t{n}", **fields}}}, "a")
        st = bp.replay(journal)
        self.assertEqual(sorted(i["id"] for i in st.issues.values()), ["ex-0", "ex-1", "ex-2"])
        self.assertEqual(st.integrity, [])
        with tempfile.TemporaryDirectory() as folder:
            attempt = Path(folder) / "01a00000-0000-7000-8000-000000000009.json"
            claim = {"v": bp.FORMAT, "id": "01a00000-0000-7000-8000-000000000009", "replica": "a", "deps": [base + "2"],
                     "actor": {"agent": "t", "thread": "t1"}, "op": "claim", "issue": base + "1", "data": {"generation": 1}}
            edit = {**claim, "id": "01a00000-0000-7000-8000-00000000000a", "deps": [claim["id"]], "op": "set",
                    "data": {"fields": {"estimated_minutes": -7}}}
            attempt.write_text(json.dumps(claim) + "\n" + json.dumps(edit) + "\n")
            problems = []
            self.assertEqual(len(bp.read_attempt(attempt, problems)), 2, problems)

        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        m = support.Machine(box, "g32")
        m.bead("init", "--prefix", "ex")
        cases = (("prio-true", {"priority": True}), ("prio-false", {"priority": False}),
                 ("est-bool", {"estimated_minutes": False}), ("est-neg", {"estimated_minutes": -7}),
                 ("est-big", {"estimated_minutes": 2**31}))
        for name, fields in cases:
            with self.subTest(import_record=name):
                path = box.root / f"{name}.jsonl"
                path.write_text(json.dumps({"id": f"ex-{name}", "title": name, **_g32_common(), **fields}) + "\n")
                self.assert_refused(m.run("import", str(path), "--json"), f"import of {fields}")
        for value in ("-7", str(2**31)):
            with self.subTest(create_estimate=value):
                self.assert_refused(m.run("create", "e", f"--estimate={value}", "--json"), f"create --estimate={value}")
        key = m.bead("create", "ok", "--estimate=0")["id"]
        m.bead("sync")
        for args in (("update", key, "--estimate=-7"), ("update", key, "--claim", "--estimate=-7")):
            with self.subTest(args=args):
                self.assert_refused(m.run(*args, "--json"), " ".join(args))
        issue = m.show(key)
        self.assertEqual((issue.get("estimated_minutes"), issue["status"]), (0, "open"))  # refused whole, no claim
        m.bead("create", "edge", "--estimate=2147483647")  # the largest value Beads reads is still accepted
        self.assertNotIn('"priority":true', m.export())


def _g33_common():
    """A Beads record's required fields."""
    return {"status": "open", "priority": 2, "issue_type": "task", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}

def _g33_tail(text):
    lines = [line for line in text.strip().split("\n") if line.strip()]
    return lines[-1] if lines else ""

class G33_DuplicateIdsInOneImportFailWhole(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.m = support.Machine(self.box, "g33")
        self.m.bead("init", "--prefix", "ex")

    def write(self, name, records):
        path = self.box.root / name
        path.write_text("".join(json.dumps(r) + "\n" for r in records))
        return path

    def test_duplicate_ids_in_one_import_fail_whole(self):
        """G33: two records with one id in an import are minted as two issues (the second renamed, and the
        first record's blockers moved onto it) instead of failing whole with VALIDATION, as br does."""
        path = self.write("dup.jsonl", [{"id": "ex-a", "title": "first", **_g33_common()},
                                        {"id": "ex-a", "title": "second", **_g33_common()}])
        proc = self.m.run("import", str(path), "--json")
        if "Traceback" in proc.stderr:
            self.fail(f"import crashed: {_g33_tail(proc.stderr)}")
        self.assertNotEqual(proc.returncode, 0, f"import with a repeated id was accepted: {proc.stdout.strip()}")
        self.assertEqual(support.code(proc), "VALIDATION")
        self.assertEqual(self.m.bead("list", "--all", "--limit", "0")["total"], 0)

    def test_concatenated_export_does_not_double_issues_or_unblock_them(self):
        """G33: the same export twice in one file (a union merge of issues.jsonl) gives two claimable copies
        of each issue and leaves a blocked issue ready; it must be refused (or mean the same as once)."""
        one = [{"id": "ex-x", "title": "x", **_g33_common()},
               {"id": "ex-y", "title": "y", **_g33_common(), "dependencies": [{"depends_on_id": "ex-x", "type": "blocks"}]}]
        proc = self.m.run("import", str(self.write("twice.jsonl", one + one)), "--json")
        if "Traceback" in proc.stderr:
            self.fail(f"import crashed: {_g33_tail(proc.stderr)}")
        keys = sorted(i["id"] for i in self.m.bead("list", "--all", "--limit", "0")["issues"])
        self.assertIn(keys, ([], ["ex-x", "ex-y"]), f"import rc {proc.returncode} {proc.stdout.strip()}")
        self.assertNotIn("ex-y", self.m.ready())
        # the same file imported once per run stays idempotent
        self.assertEqual(self.m.bead("import", str(self.write("once.jsonl", one)))["imported"], 0 if keys else 2)
        self.assertEqual(sorted(i["id"] for i in self.m.bead("list", "--all", "--limit", "0")["issues"]), ["ex-x", "ex-y"])


def _g34_tail(text):
    lines = [line for line in text.strip().split("\n") if line.strip()]
    return lines[-1] if lines else ""

def _g34_git(*args, cwd, env=None, data=None):
    """git with byte arguments allowed; returns stdout bytes."""
    return subprocess.run([b"git", *[a if isinstance(a, bytes) else a.encode() for a in args]], cwd=cwd, env=env,
                          input=data, capture_output=True, check=True).stdout.strip()

def _g34_add_packed_ref(checkout, ref, sha):
    """A ref whose name is not UTF-8. APFS refuses such file names, so it goes into packed-refs."""
    path = Path(checkout, ".git", "packed-refs")
    rows = [r for r in (path.read_bytes().split(b"\n") if path.exists() else []) if r and not r.startswith(b"#")]
    rows.append(sha + b" " + ref)
    rows.sort(key=lambda r: r.split(b" ", 1)[1])
    path.write_bytes(b"# pack-refs with: peeled fully-peeled sorted \n" + b"\n".join(rows) + b"\n")

class G34_NonUtf8GitAndPsOutputIsNotATraceback(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def assert_no_traceback(self, proc, what):
        if "Traceback" in proc.stderr:
            self.fail(f"{what} crashed: {_g34_tail(proc.stderr)}")

    def test_create_on_a_branch_whose_name_is_not_utf8(self):
        """G34: capture_where strictly decodes `rev-parse --abbrev-ref HEAD`, so `create` on a branch named
        with Latin-1 bytes is a UnicodeDecodeError traceback; it must work (the branch is not recorded)."""
        m = support.Machine(self.box, "g34a")
        m.bead("init", "--prefix", "ex")
        sha = _g34_git("rev-parse", "HEAD", cwd=m.checkout)
        _g34_add_packed_ref(m.checkout, b"refs/heads/caf\xe9", sha)
        _g34_git("symbolic-ref", "HEAD", b"refs/heads/caf\xe9", cwd=m.checkout)
        self.assertEqual(_g34_git("rev-parse", "--abbrev-ref", "HEAD", cwd=m.checkout), b"caf\xe9")
        proc = m.run("create", "on a latin-1 branch", "--json")
        self.assert_no_traceback(proc, "create on branch caf\\xe9")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertNotIn("branch", m.show(json.loads(proc.stdout)["id"]))

    def test_every_command_with_an_origin_url_that_is_not_utf8(self):
        """G34: find_workspace strictly decodes remote.origin.url, and help/prime catch only BpError, so a
        Latin-1 byte in the origin URL makes every command in the checkout a traceback, help included."""
        m = support.Machine(self.box, "g34b")
        _g34_git("config", "remote.origin.url", b"https://example.invalid/caf\xe9.git", cwd=m.checkout)
        for args in (("help",), ("prime",), ("init", "--prefix", "ex"), ("create", "x"), ("info",)):
            with self.subTest(args=args):
                proc = m.run(*args, "--json")
                self.assert_no_traceback(proc, f"bead {args[0]}")
                self.assertEqual(proc.returncode, 0, proc.stderr)
        info = m.bead("info")
        self.assertEqual((info["workspace_source"], info["name"]), ("origin url", "caf\ufffd"))

    def test_show_closed_issue_when_origin_head_names_a_latin1_branch(self):
        """G34: merged_into_default strictly decodes the origin/HEAD target, so `show` of a closed issue
        is a traceback when origin/HEAD names a branch that is not UTF-8."""
        m = support.Machine(self.box, "g34c")
        m.bead("init", "--prefix", "ex")
        key = m.bead("create", "x")["id"]
        m.bead("close", key)
        _g34_git("symbolic-ref", "refs/remotes/origin/HEAD", b"refs/remotes/origin/caf\xe9", cwd=m.checkout)
        proc = m.run("show", key, "--json")
        self.assert_no_traceback(proc, "show of a closed issue")
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def _plant_foreign_folder(self):
        """Add events/caf\\xe9/1.jsonl to the published coordination ref on the shared bare remote."""
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(GIT_DIR=str(self.box.origin), GIT_AUTHOR_NAME="x", GIT_AUTHOR_EMAIL="x@example.invalid",
                   GIT_COMMITTER_NAME="x", GIT_COMMITTER_EMAIL="x@example.invalid")
        root = self.box.root
        tip = _g34_git("rev-parse", "refs/beads/journal", cwd=root, env=env)
        top = _g34_git("ls-tree", "-z", tip + b"^{tree}", cwd=root, env=env).split(b"\0")
        events_sha = next(r.split(b"\t")[0].split()[2] for r in top if r.endswith(b"\tevents"))
        blob = _g34_git("hash-object", "-w", "--stdin", cwd=root, env=env, data=b"")
        sub = _g34_git("mktree", "-z", cwd=root, env=env, data=b"100644 blob " + blob + b"\t1.jsonl\0")
        old = [r for r in _g34_git("ls-tree", "-z", events_sha, cwd=root, env=env).split(b"\0") if r]
        events = _g34_git("mktree", "-z", cwd=root, env=env,
                          data=b"".join(r + b"\0" for r in old) + b"040000 tree " + sub + b"\tcaf\xe9\0")
        rows = [r for r in top if r and not r.endswith(b"\tevents")]
        tree = _g34_git("mktree", "-z", cwd=root, env=env,
                        data=b"".join(r + b"\0" for r in rows) + b"040000 tree " + events + b"\tevents\0")
        commit = _g34_git("commit-tree", tree, "-p", tip, "-m", "foreign", cwd=root, env=env)
        _g34_git("update-ref", "refs/beads/journal", commit, tip, cwd=root, env=env)

    def test_sync_with_a_foreign_folder_named_in_latin1_and_quotepath_false(self):
        """G34: Link.merge decodes `ls-tree -r` strictly (git_text); with core.quotePath=false git prints
        a non-UTF-8 folder name raw and `sync` is a traceback. What merge sees must not depend on quotePath."""
        m = support.Machine(self.box, "g34d")
        m.bead("init", "--prefix", "ex")
        m.bead("create", "x")
        m.bead("sync")
        self._plant_foreign_folder()
        quote = self.box.root / "quote.gitconfig"
        quote.write_text("[core]\n\tquotePath = false\n")
        empty = self.box.root / "empty.gitconfig"
        empty.write_text("")
        # quotePath=false first: a default-config sync would skip the quoted path and publish a tree without it
        proc = m.run("sync", "--json", GIT_CONFIG_GLOBAL=str(quote), GIT_CONFIG_NOSYSTEM="1")
        self.assert_no_traceback(proc, "sync with core.quotePath=false")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        control = m.run("sync", "--json", GIT_CONFIG_GLOBAL=str(empty), GIT_CONFIG_NOSYSTEM="1")
        self.assert_no_traceback(control, "sync with the default quotePath")
        self.assertEqual(control.returncode, 0, control.stderr)

    def test_nested_harness_whose_parent_argv_is_not_utf8(self):
        """G34: nearest_harness reads `ps` with text=True (strict decode); with both CLAUDE_CODE_SESSION_ID and
        CODEX_THREAD_ID set, a parent process whose argv holds Latin-1 bytes makes even `help` a traceback."""
        m = support.Machine(self.box, "g34e")
        m.bead("init", "--prefix", "ex")
        env = m.env()
        env.pop("BEAD_AGENT")
        env.pop("BEAD_THREAD")
        env.update(CLAUDE_CODE_SESSION_ID="claude-session-0001", CODEX_THREAD_ID="codex-thread-0001")
        script = b'"$0" "$1" --json; :'  # `; :` stops the shell from exec-ing bead: the shell stays its parent
        for argument in (b"resume.txt", b"r\xe9sum\xe9.txt"):  # the ASCII control first
            proc = subprocess.run([b"sh", b"-c", script, support.BEAD.encode(), b"help", argument],
                                  cwd=m.checkout, env=env, capture_output=True, text=True)
            self.assert_no_traceback(proc, f"help under a parent whose argv holds {argument!r}")
            self.assertEqual(proc.returncode, 0, proc.stderr)


def _g35_untracked(checkout):
    return subprocess.run(["git", "status", "--porcelain", "--untracked-files=all"], cwd=checkout,
                          capture_output=True, text=True).stdout.splitlines()

class G35_StoreNeverLandsInTheCheckout(unittest.TestCase):
    """G35: BEAD_HOME is used verbatim: an unexpanded `~/...` or a relative path puts the store inside the
    current directory, i.e. inside the checkout, and each subdirectory gets a different store."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.m = support.Machine(self.box, "g35")
        (self.m.checkout / "src").mkdir()

    def test_g35_tilde_bead_home_means_the_users_home(self):
        """G35: BEAD_HOME=~/beads (as settings files and service definitions pass it) creates a directory named
        `~` in the checkout holding the journal and the machine identity; from src/ the tracker is missing."""
        fake = self.box.root / "fakehome"
        fake.mkdir()
        env = {"BEAD_HOME": "~/beads", "HOME": str(fake)}
        made = self.m.run("init", "--prefix", "ex", "--json", **env)
        self.assertEqual(made.returncode, 0, made.stderr)
        created = self.m.run("create", "from top", "--json", **env)
        self.assertEqual(created.returncode, 0, created.stderr)
        listed = self.m.run("list", "--json", cwd=self.m.checkout / "src", **env)
        self.assertEqual(_g35_untracked(self.m.checkout), [], "the store was written inside the checkout")
        self.assertEqual(listed.returncode, 0, f"a subdirectory sees another store: {listed.stderr}")
        self.assertEqual([i["title"] for i in json.loads(listed.stdout)["issues"]], ["from top"])
        self.assertTrue((fake / "beads" / "replica.json").exists(), "the store is not under the user's home")

    def test_g35_relative_bead_home_never_writes_into_the_checkout(self):
        """G35: BEAD_HOME=state/beads writes the store (journal, lock, meta, machine identity) under the
        current directory of each command."""
        made = self.m.run("init", "--prefix", "ex", "--json", BEAD_HOME="state/beads")
        self.assertEqual(_g35_untracked(self.m.checkout), [],
                         f"the store was written inside the checkout: {made.stdout or made.stderr}")
        if made.returncode:
            self.assertEqual(json.loads(made.stderr)["error"]["code"], "CONFIG", made.stderr)


def _g36_hung_up_terminal(test):
    """The slave side of a pseudo-terminal whose master is already closed: every write to it fails with EIO,
    as when the terminal window or the ssh session that owned it went away."""
    master, slave = os.openpty()
    os.close(master)
    test.addCleanup(os.close, slave)
    return slave

def _g36_noise(proc):
    return f"exit {proc.returncode}, stderr ends: " + repr((proc.stderr or b"").decode(errors="replace")[-300:])

class G36_HungUpTerminalAndClosedDescriptors(unittest.TestCase):
    longMessage = False  # the messages below quote the end of bead's stderr

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.machine = support.Machine(self.box, "tty")
        self.machine.bead("init", "--prefix", "ex")

    def run_bead(self, *args, stdout=subprocess.PIPE, stderr=subprocess.PIPE):
        return subprocess.run([support.BEAD, *args], cwd=self.machine.checkout, env=self.machine.env(),
                              stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, timeout=30)

    def run_with_closed(self, redirection, *args):
        """Run bead as `bead ARGS >&-` (or `2>&-`): started with that descriptor closed."""
        return subprocess.run(["/bin/sh", "-c", f'exec "$0" "$@" {redirection}', support.BEAD, *args],
                              cwd=self.machine.checkout, env=self.machine.env(), stdin=subprocess.DEVNULL,
                              capture_output=True, timeout=30)

    def titles(self):
        return [i["title"] for i in self.machine.bead("list", "--all", "--limit", "0")["issues"]]

    def test_committed_create_on_a_hung_up_terminal_exits_zero_quietly(self):
        """G36: a create that committed exits 120 with an EIO traceback when its terminal hung up, so a wrapper
        that retries on failure creates the issue twice; EPIPE on a closed pipe already exits 0 quietly."""
        for args in (["create", "made while the terminal was gone"], ["create", "made as json", "--json"]):
            with self.subTest(args=args):
                proc = self.run_bead(*args, stdout=_g36_hung_up_terminal(self))
                self.assertIn(args[1], self.titles())  # the write committed
                self.assertNotIn(b"Traceback", proc.stderr, _g36_noise(proc))
                self.assertEqual(proc.returncode, 0, _g36_noise(proc))

    def test_read_commands_on_a_hung_up_terminal_exit_zero_quietly(self):
        """G36: list, list --json, the overview, export and COMMAND --help exit 120 with Python noise when the
        terminal hung up, instead of exiting 0 quietly as on a closed pipe."""
        self.machine.bead("create", "something to list")
        for args in (["list"], ["list", "--json"], [], ["export"], ["create", "--help"]):
            with self.subTest(args=args):
                proc = self.run_bead(*args, stdout=_g36_hung_up_terminal(self))
                self.assertNotIn(b"Traceback", proc.stderr, _g36_noise(proc))
                self.assertNotIn(b"Exception ignored", proc.stderr, _g36_noise(proc))
                self.assertEqual(proc.returncode, 0, _g36_noise(proc))

    def test_failed_command_on_a_hung_up_terminal_keeps_its_status(self):
        """G36: a failed command whose terminal hung up exits 120, not its own status (1, or 2 for USAGE), which F06
        decided for a closed stderr."""
        for args, status in ((["show", "ex-nope"], 1), (["no-such-command"], 2)):
            with self.subTest(args=args):
                slave = _g36_hung_up_terminal(self)
                proc = self.run_bead(*args, stdout=slave, stderr=slave)
                self.assertEqual(proc.returncode, status, f"`bead {' '.join(args)}` on a hung-up terminal exited "
                                 f"{proc.returncode}, not its own status {status}")

    def test_closed_descriptors_do_not_turn_a_committed_create_into_a_traceback(self):
        """G36: started with stdout closed (`>&-`), a committed create exits 1 with an AttributeError traceback;
        started with stderr closed, a usage error exits 1 instead of 2 and prints its message on stdout."""
        made = self.run_with_closed(">&-", "create", "made with stdout closed")
        self.assertEqual(self.titles(), ["made with stdout closed"])  # the write committed
        self.assertNotIn(b"Traceback", made.stderr, _g36_noise(made))
        self.assertEqual(made.returncode, 0, _g36_noise(made))
        usage = self.run_with_closed("2>&-", "no-such-command")
        self.assertEqual((usage.returncode, usage.stdout), (2, b""),
                         f"a usage error with stderr closed exited {usage.returncode} (USAGE is 2) and wrote "
                         f"{usage.stdout!r} on stdout")


class G37_JsonSaysWhenTheEndOfAClaimWasNotPublished(unittest.TestCase):
    """G37: close, delete and `update --status closed` by the owner publish at once (publish_soon); when
    that publish fails, text mode prints "recorded locally, not yet published (CODE)", but the --json
    answer is the issue list alone, identical to a published one. An agent that reads --json (the
    mode `bead prime` tells it to use) believes the other machines know the claim has ended."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "a")
        self.a.bead("init", "--prefix", "gj")
        self.keys = [self.a.bead("create", name)["id"] for name in ("closed", "deleted", "updated", "control", "text")]
        self.a.bead("sync")
        for key in self.keys:
            self.a.bead("claim", key)

    def test_g37_json_close_delete_update_say_they_were_not_published(self):
        """G37: with --json, an owner's close/delete/update --status closed whose publish failed must say so."""
        control = self.a.run("close", self.keys[3], "--reason", "done", "--json")  # the remote answers
        self.assertEqual(control.returncode, 0, control.stderr)
        self.assertNotIn("unpublished", json.loads(control.stdout)[0], "a published close is not marked")
        away = self.box.root / "origin-away.git"
        self.box.origin.rename(away)  # the remote is unreachable from here on
        try:
            runs = {"close": self.a.run("close", self.keys[0], "--reason", "done", "--json"),
                    "delete": self.a.run("delete", self.keys[1], "--reason", "gone", "--json"),
                    "update": self.a.run("update", self.keys[2], "--status", "closed", "--json"),
                    "close (text)": self.a.run("close", self.keys[4], "--reason", "done")}
        finally:
            away.rename(self.box.origin)
        self.assertIn("not yet published", runs.pop("close (text)").stdout, "text mode is told")
        for name, proc in runs.items():
            with self.subTest(command=name):
                self.assertEqual(proc.returncode, 0, proc.stderr)
                answer = json.loads(proc.stdout)
                self.assertIsInstance(answer, list, "the --json answer keeps its shape: a list of issues")
                self.assertEqual(len(answer), 1)
                self.assertIn("not yet published", answer[0].get("unpublished", ""),
                              f"{name} --json: the publish failed, yet the answer carries no sign of it "
                              f"(stdout {proc.stdout.strip()[:160]!r}, stderr {proc.stderr.strip()!r})")


def _g38_common():
    """A Beads record's required fields."""
    return {"status": "open", "priority": 2, "issue_type": "task", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}

def _g38_shapes():
    """`comments` values that are not a list of objects."""
    return (("list of strings", ["text"]), ("object", {"id": 1}), ("number", 5), ("string", "abc"),
            ("true", True), ("list holding null", [None]))

def _g38_tail(text):
    lines = [line for line in text.strip().split("\n") if line.strip()]
    return lines[-1] if lines else ""

class G38_ImportCommentsOfTheWrongShapeAreValidation(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_wrong_comment_shapes_fail_the_import_with_validation(self):
        """G38: an imported record whose `comments` is not a list of objects crashes import_records with
        AttributeError/TypeError before validation runs, instead of failing whole with VALIDATION."""
        m = support.Machine(self.box, "g38")
        m.bead("init", "--prefix", "ex")
        good = {"id": "ex-good", "title": "good", **_g38_common()}
        for name, comments in _g38_shapes():
            with self.subTest(comments=name):
                path = self.box.root / f"{name.replace(' ', '-')}.jsonl"
                path.write_text(json.dumps(good) + "\n"
                                + json.dumps({"id": "ex-bad", "title": name, **_g38_common(), "comments": comments}) + "\n")
                proc = m.run("import", str(path), "--json")
                if "Traceback" in proc.stderr:
                    self.fail(f"import with comments as {name} crashed: {_g38_tail(proc.stderr)}")
                self.assertEqual(proc.returncode, 1, proc.stdout)
                self.assertEqual(support.code(proc), "VALIDATION")
        self.assertEqual(m.bead("list", "--all", "--limit", "0")["total"], 0)  # failed whole: not even ex-good
        ok = self.box.root / "ok.jsonl"
        ok.write_text(json.dumps({**good, "comments": [{"id": 7, "author": "a", "text": "kept",
                                                        "created_at": "2026-01-01T00:00:00Z"}]}) + "\n")
        self.assertEqual(m.bead("import", str(ok))["imported"], 1)
        self.assertEqual([c["text"] for c in m.show("ex-good")["comments"]], ["kept"])

    def test_init_import_with_wrong_comment_shape_is_validation(self):
        """G38: `init --import` of such a record also tracebacks instead of a VALIDATION error."""
        path = self.box.root / "init.jsonl"
        path.write_text(json.dumps({"id": "bx-bad", "title": "t", **_g38_common(), "comments": ["x"]}) + "\n")
        m = support.Machine(self.box, "g38init")
        proc = m.run("init", "--prefix", "bx", "--import", str(path), "--json")
        if "Traceback" in proc.stderr:
            self.fail(f"init --import with comments as a list of strings crashed: {_g38_tail(proc.stderr)}")
        self.assertEqual(support.code(proc), "VALIDATION")


def _g39_recording_server(test):
    """A local HTTP server that answers 401 until it is sent credentials, then 404. Returns its port and the
    list of Authorization headers it has seen."""
    seen = []

    class Recorder(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            auth = self.headers.get("Authorization")
            seen.append(auth)
            self.send_response(404 if auth else 401)
            if not auth:
                self.send_header("WWW-Authenticate", 'Basic realm="x"')
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *args):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Recorder)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    test.addCleanup(server.server_close)
    test.addCleanup(server.shutdown)
    return server.server_address[1], seen

class G39_PasswordInATransportUrl(unittest.TestCase):
    PLAIN = "https://alice:supersecret@example.invalid/org/repo.git"
    HELPER = "https::" + PLAIN  # git's `<transport>::<address>` form: git-remote-https gets the address as is

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        gitconfig = self.box.root / "empty-gitconfig"
        gitconfig.write_text("")  # no credential helper or askpass from the developer's own config
        self.git_env = {"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": str(gitconfig), "GIT_ASKPASS": ""}
        self.count = 0

    def initialised(self, url, *more):
        """A new machine after `init --remote URL`, and what init printed."""
        self.count += 1
        machine = support.Machine(self.box, f"m{self.count}")
        machine.extra.update(self.git_env)
        made = machine.run("init", "--prefix", "ex", "--remote", url, *more)
        self.assertEqual(made.returncode, 0, made.stderr)
        return machine, made.stdout + made.stderr

    def test_password_in_transport_url_is_not_stored_or_shown(self):
        """G39: the password of `https::https://user:pw@host/...` must be dropped exactly as for
        `https://user:pw@host/...`: not kept in meta.json, not shown by info, with the `https::` prefix kept."""
        stored, shown, said = {}, {}, {}
        for name, url in (("helper", self.HELPER), ("plain", self.PLAIN)):
            machine, said[name] = self.initialised(url)
            stored[name] = json.loads(next(machine.home.glob("*/meta.json")).read_text())["remote"]
            info = machine.run("info", "--json")
            self.assertEqual(info.returncode, 0, info.stderr)
            shown[name] = json.loads(info.stdout)["remote"]
        self.assertNotIn("supersecret", stored["helper"] + shown["helper"], "the password was kept")
        self.assertEqual(stored["helper"], "https::" + stored["plain"])
        self.assertEqual(shown["helper"], "https::" + shown["plain"])
        self.assertEqual("password was dropped" in said["helper"], "password was dropped" in said["plain"])

    def test_password_in_transport_url_is_not_committed(self):
        """G39: `init --tracked --remote https::https://user:pw@host/...` must not write the password into
        .beadroll.json, the file bead tells the user to commit."""
        configs = {}
        for name, url in (("helper", self.HELPER), ("plain", self.PLAIN)):
            machine, _ = self.initialised(url, "--tracked", "--json")
            configs[name] = json.loads((machine.checkout / ".beadroll.json").read_text())["remote"]
        self.assertNotIn("supersecret", configs["helper"], "the password is in .beadroll.json")
        self.assertEqual(configs["helper"], "https::" + configs["plain"])

    def test_password_in_transport_url_is_not_handed_to_git(self):
        """G39: bead must not hand git the password of `http::http://user:pw@host/...` (git sends it as Basic
        auth), nor print it when the sync fails."""
        port, seen = _g39_recording_server(self)
        machine, _ = self.initialised(f"http::http://alice:secret-g39@127.0.0.1:{port}/org/repo.git", "--json")
        synced = machine.run("sync")
        self.assertNotEqual(synced.returncode, 0)  # the server never accepts; only what it was sent matters
        self.assertTrue(seen, "git never reached the recording server")
        self.assertEqual([auth for auth in seen if auth], [], "git was handed the password and sent it to the server")
        self.assertNotIn("secret-g39", synced.stdout + synced.stderr)


def _g40_init(test, machine, url):
    """`init --remote URL` on a new machine: (what init printed, meta.json's remote, info's remote)."""
    made = machine.run("init", "--prefix", "ex", "--remote", url)
    test.assertEqual(made.returncode, 0, made.stderr)
    info = machine.run("info", "--json")
    test.assertEqual(info.returncode, 0, info.stderr)
    meta = json.loads(next(machine.home.glob("*/meta.json")).read_text())
    return made.stdout + made.stderr, meta.get("remote"), json.loads(info.stdout)["remote"]

class G40_AtSignAfterTheAuthority(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_at_sign_in_path_query_or_port_url_is_not_user_info(self):
        """G40: an '@' after the authority (in the path or query, after a port or a bracketed IPv6 host) is not
        user-info: bead must store and use the URL as given, not a different host, and not claim it dropped
        a password."""
        for number, url in enumerate(("https://host.example/org/repo@v2.git", "ssh://host.example:2222/srv/a@b.git",
                                      "https://[::1]:8443/a@b", "file:///tmp/a:b@c/r.git",
                                      "https://host.example/r?x=a@b")):
            with self.subTest(url=url):
                said, stored, used = _g40_init(self, support.Machine(self.box, f"at{number}"), url)
                self.assertEqual(stored, url, "meta.json holds a different remote")
                self.assertEqual(used, url, "bead would sync with a different remote")
                self.assertNotIn("password was dropped", said)

    def test_password_is_dropped_but_host_and_path_are_kept(self):
        """G40: in https://u:p@h/r@x the user-info ends at the first '/': git sends u:p to host h for path /r@x,
        so bead must drop the password and keep host h and path /r@x."""
        said, stored, used = _g40_init(self, support.Machine(self.box, "both"),
                                       "https://u:p4ss@h.example/r@x")
        for value in (stored, used):
            self.assertNotIn("p4ss", value)
            self.assertTrue(value.startswith("https://") and value.endswith("h.example/r@x"),
                            f"the host became {value!r}")
        self.assertIn("password was dropped", said)

    def test_origin_with_at_sign_in_its_path_syncs(self):
        """G40: a file:// origin whose path holds ':' and '@' is the repository git fetches; bead must sync with
        it rather than with a path it made up by reading 'a:b' as user:password."""
        tracker = self.box.bare("a:b@c-tracker.git")
        machine = support.Machine(self.box, "path")
        url = f"file://{tracker}"
        support.git("remote", "set-url", "origin", url, cwd=machine.checkout)
        support.git("ls-remote", "origin", cwd=machine.checkout)  # git itself reaches it
        self.assertEqual(machine.run("init", "--prefix", "ex").returncode, 0)
        machine.bead("create", "through a path with an at sign")
        self.assertEqual(machine.bead("info")["remote"], url)
        synced = machine.run("sync", "--json")
        self.assertEqual(synced.returncode, 0, synced.stderr)
        refs = subprocess.run(["git", "for-each-ref", "refs/beads"], cwd=tracker, capture_output=True, text=True)
        self.assertIn("refs/beads/journal", refs.stdout)


class G41_DamagedRemoteIsNotAnUnreachableOne(unittest.TestCase):
    def test_remote_ref_naming_lost_objects_is_reported_as_damage_naming_the_ref(self):
        """G41: when the remote's refs/beads/journal names a commit or tree the remote no longer has (what a
        `git gc --prune=now` racing a push leaves), every fetch fails and bead reports REMOTE_UNREACHABLE
        'cannot reach the coordination remote', which PRIME defines as 'nothing is lost; retry later'. The remote
        answered, no retry can succeed, and the message never names the damaged ref."""
        for lost in ("commit", "tree"):
            with self.subTest(lost=lost):
                box = support.Sandbox()
                self.addCleanup(box.cleanup)
                a, b = support.Machine(box, f"g41a-{lost}"), support.Machine(box, f"g41b-{lost}")
                a.bead("init", "--prefix", "ex")
                a.bead("sync")
                b.bead("sync")
                key = a.bead("create", "work")["id"]
                a.bead("sync")  # b has not fetched this commit
                spec = "refs/beads/journal" + ("" if lost == "commit" else "^{tree}")
                sha = support.git("--git-dir", str(box.origin), "rev-parse", spec).strip()
                loose = box.origin / "objects" / sha[:2] / sha[2:]
                self.assertTrue(loose.exists(), "a small push arrives as loose objects")
                loose.unlink()  # what a concurrent `git gc --prune=now` did to a just-pushed object in this review's soak
                for argv in (["sync"], ["claim", key]):
                    proc = b.run(*argv, "--json")
                    self.assertEqual(proc.returncode, 1, proc.stdout)
                    error = json.loads(proc.stderr)["error"]
                    self.assertNotEqual(error["code"], "REMOTE_UNREACHABLE",
                                        f"{argv[0]}: a remote that answered is reported unreachable "
                                        f"('nothing is lost; retry later'): {error['message']}")
                    self.assertIn("refs/beads/journal", error["message"], f"{argv[0]} does not name the damaged ref")


def _g42_record(key, title, depends_on=None, kind="blocks"):
    rec = {"id": key, "title": title, "status": "open", "priority": 2, "issue_type": "task",
           "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}
    if depends_on:
        rec["dependencies"] = [{"issue_id": key, "depends_on_id": depends_on, "type": kind}]
    return rec

class G42_UidRetractionMissesTwinOfImportedKey(unittest.TestCase):
    """An imported dependency on a key that did not exist yet binds, after a merge, to whichever
    concurrent copy of that key sorts first. A writer who took the dependency back named the copy it saw
    (dep_remove by uid); when the other copy keeps the key, the retraction misses it and the dependency
    is live again on every machine."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def _g42_machines(self, kind):
        (self.box.root / "a.jsonl").write_text(json.dumps(_g42_record("ex-a", "A", "ex-b", kind)) + "\n")
        (self.box.root / "b.jsonl").write_text(json.dumps(_g42_record("ex-b", "B")) + "\n")
        m1 = support.Machine(self.box, "m1")
        m1.bead("init", "--prefix", "ex")
        m1.bead("import", str(self.box.root / "a.jsonl"))
        q = m1.bead("create", "Q")["id"]
        m1.bead("sync")
        m2, m3 = support.Machine(self.box, "m2"), support.Machine(self.box, "m3")
        m2.bead("sync")
        m3.bead("sync")
        m3.bead("import", str(self.box.root / "b.jsonl"))  # earlier id: this copy keeps ex-b after the merge
        m2.bead("import", str(self.box.root / "b.jsonl"))  # renamed after the merge
        self.assertEqual([(d["id"], d["dependency_type"]) for d in m2.show("ex-a")["dependencies"]], [("ex-b", kind)])
        return m1, m2, m3, q

    def _g42_settle(self, *machines):
        for machine in machines + machines:
            machine.bead("sync")
        self.assertEqual(len({m.export() for m in machines}), 1, "machines did not converge")

    def test_dep_remove_of_an_imported_dependency_survives_the_merge(self):
        """G42: `dep remove ex-a ex-b` of an imported dependency stays removed after sync even when a
        concurrent import of ex-b on another machine sorts first and keeps the key."""
        m1, m2, m3, _ = self._g42_machines("blocks")
        m2.bead("dep", "remove", "ex-a", "ex-b")  # by uid: m2's own copy of ex-b
        self.assertEqual(m2.show("ex-a")["dependencies"], [])
        self._g42_settle(m1, m2, m3)
        self.assertEqual(len(m2.bead("list", "--all", "--limit", "0")["issues"]), 4)  # ex-a, Q, ex-b, renamed twin
        self.assertEqual(m2.show("ex-a")["dependencies"], [],
                         "the imported dependency m2 took back is live again, bound to the other copy of ex-b")
        self.assertIn("ex-a", m2.ready())

    def test_reparenting_away_from_an_imported_parent_survives_the_merge(self):
        """G42: `update --parent Q` that moves ex-a away from an imported parent-child dependency keeps it
        moved after sync even when a concurrent import of the old parent's key sorts first."""
        m1, m2, m3, q = self._g42_machines("parent-child")
        m2.bead("update", "ex-a", "--parent", q)  # removes the imported parent edge by uid
        self.assertEqual([d["id"] for d in m2.show("ex-a")["dependencies"]], [q])
        self._g42_settle(m1, m2, m3)
        self.assertEqual([d["id"] for d in m2.show("ex-a")["dependencies"]], [q],
                         "the imported parent m2 moved ex-a away from is a parent again after the merge")


class G43_RenamedRecordExportsForeignIssueId(unittest.TestCase):
    """When a record imported on two machines is renamed in the merge (ex-a -> ex-aX), the renamed copy's
    still-unresolved imported dependency is exported with issue_id 'ex-a', the other record's id; br
    refuses the whole file ('dependency issue_id ... does not match import issue ...')."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_unresolved_dependency_is_exported_under_the_records_own_id(self):
        """G43: every exported dependency names its own record (issue_id == id), including a still-dangling
        imported dependency of a record that was renamed because two machines imported the same key."""
        dep = {"issue_id": "ex-a", "depends_on_id": "external:other:ex-9", "type": "blocks",
               "created_at": "2026-01-01T00:00:00Z", "created_by": "seed", "metadata": "{}", "thread_id": ""}
        rec = {"id": "ex-a", "title": "A", "status": "open", "priority": 2, "issue_type": "task",
               "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed",
               "dependencies": [dep]}
        path = self.box.root / "ext.jsonl"
        path.write_text(json.dumps(rec) + "\n")
        m1 = support.Machine(self.box, "m1")
        m1.bead("init", "--prefix", "ex")
        m1.bead("sync")
        m2 = support.Machine(self.box, "m2")
        m2.bead("sync")
        m1.bead("import", str(path))
        m2.bead("import", str(path))
        for machine in (m1, m2, m1, m2):
            machine.bead("sync")
        records = [json.loads(line) for line in m1.export().splitlines()]
        self.assertEqual(len(records), 2)
        self.assertEqual(len({r["id"] for r in records}), 2)
        for exported in records:
            self.assertEqual([d["issue_id"] for d in exported["dependencies"]], [exported["id"]],
                             f"record {exported['id']} exports a dependency whose issue_id names another record")


def _g44_record(key, *depends_on):
    rec = {"id": key, "title": key.upper(), "status": "open", "priority": 2, "issue_type": "task",
           "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}
    if depends_on:  # no created_at / created_by on the dependency, as in the shipped incremental-import test
        rec["dependencies"] = [{"issue_id": key, "depends_on_id": d, "type": "blocks"} for d in depends_on]
    return rec

def _g44_dependency(machine, key):
    rec = next(r for r in map(json.loads, machine.export().splitlines()) if r["id"] == key)
    return rec["dependencies"][0]

class G44_MaterialisedDependencyExportsNullMeta(unittest.TestCase):
    """A dependency without created_at/created_by: imported in one batch with its target, the dep_add
    stamps it; when the target arrives later (another import or another machine), materialise_dangling
    copies only the keys present and export writes "created_at": null, "created_by": null, which br
    refuses ('invalid type: null, expected an RFC 3339 formatted date and time string')."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        a, b = _g44_record("ex-a", "ex-b"), _g44_record("ex-b")
        for name, recs in (("a", [a]), ("b", [b]), ("ab", [a, b])):
            (self.box.root / f"{name}.jsonl").write_text("".join(json.dumps(r) + "\n" for r in recs))

    def test_incremental_import_stamps_the_dependency_like_one_shot(self):
        """G44: importing the dependent and its target in two batches exports the dependency with a string
        created_at and created_by, as a one-shot import of the same records does."""
        split = support.Machine(self.box, "split")
        split.bead("init", "--prefix", "ex")
        split.bead("import", str(self.box.root / "a.jsonl"))
        split.bead("import", str(self.box.root / "b.jsonl"))
        one = support.Machine(self.box, "one")
        one.bead("init", "--prefix", "ex")
        one.bead("import", str(self.box.root / "ab.jsonl"))
        self.assertIsInstance(_g44_dependency(one, "ex-a")["created_at"], str)
        self.assertIsInstance(_g44_dependency(one, "ex-a")["created_by"], str)
        dep = _g44_dependency(split, "ex-a")
        self.assertIsInstance(dep["created_at"], str, f"incremental import exported a null created_at: {dep}")
        self.assertIsInstance(dep["created_by"], str, f"incremental import exported a null created_by: {dep}")

    def test_target_imported_on_another_machine_stamps_the_dependency(self):
        """G44: when the dependent and its target are imported on two machines and meet by sync, both
        machines export the dependency with a string created_at and created_by."""
        x, y = support.Machine(self.box, "x"), support.Machine(self.box, "y")
        x.bead("init", "--prefix", "ex")
        x.bead("sync")
        y.bead("sync")
        x.bead("import", str(self.box.root / "a.jsonl"))
        y.bead("import", str(self.box.root / "b.jsonl"))
        for machine in (x, y, x, y):
            machine.bead("sync")
        self.assertEqual(x.export(), y.export())
        dep = _g44_dependency(x, "ex-a")
        self.assertEqual(dep["depends_on_id"], "ex-b")
        self.assertIsInstance(dep["created_at"], str, f"the synced dependency exported a null created_at: {dep}")
        self.assertIsInstance(dep["created_by"], str, f"the synced dependency exported a null created_by: {dep}")


class G45_UnresolvedImportedDependencyInvisible(unittest.TestCase):
    """An imported dependency on a key that is not an issue yet appears only in export: show (JSON and
    human) and dep list omit it, although `dep remove` accepts it and it blocks the issue as soon as the
    key is imported."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        rec = {"id": "ex-a", "title": "A", "status": "open", "priority": 2, "issue_type": "task",
               "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed",
               "dependencies": [{"issue_id": "ex-a", "depends_on_id": "ex-b", "type": "blocks"},
                                {"issue_id": "ex-a", "depends_on_id": "ex-b", "type": None}]}
        (self.box.root / "a.jsonl").write_text(json.dumps(rec) + "\n")
        self.machine = support.Machine(self.box, "vis")
        self.machine.bead("init", "--prefix", "ex")
        self.machine.bead("import", str(self.box.root / "a.jsonl"))

    def test_show_and_dep_list_report_an_unresolved_imported_dependency(self):
        """G45: show --json, human show, dep list --json and human dep list name an imported dependency
        on a key that is not an issue yet (once, although the record lists it twice)."""
        machine = self.machine
        self.assertIn("ex-b", machine.export())
        self.assertIn("ex-a", machine.ready())  # ready now; it becomes blocked the moment ex-b is imported
        shown = machine.show("ex-a")
        self.assertIn("ex-b", json.dumps(shown), "show --json hides the pending imported dependency: "
                      + json.dumps(shown))
        listed = machine.bead("dep", "list", "ex-a")
        self.assertIn("ex-b", json.dumps(listed), "dep list --json hides it: " + json.dumps(listed))
        human = machine.run("show", "ex-a")
        self.assertEqual(human.returncode, 0, human.stderr)
        self.assertEqual(human.stdout.count("ex-b"), 1, "human show hides it (or lists it twice): " + human.stdout)
        human_list = machine.run("dep", "list", "ex-a")
        self.assertIn("ex-b", human_list.stdout, "human dep list hides it: " + human_list.stdout)
        self.assertEqual(machine.bead("dep", "remove", "ex-a", "ex-b")["depends_on_id"], "ex-b")
        self.assertNotIn("ex-b", json.dumps(machine.show("ex-a")), "the retracted dependency is still shown")


def _g46_record(key, *depends_on):
    rec = {"id": key, "title": key.upper(), "status": "open", "priority": 2, "issue_type": "task",
           "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}
    if depends_on:
        rec["dependencies"] = [{"issue_id": key, "depends_on_id": d, "type": kind} for d, kind in depends_on]
    return rec

class G46_ImportAcceptsSelfDependency(unittest.TestCase):
    """A record whose dependencies name its own id is imported as a self-edge: the issue is blocked by
    itself and never ready, `dep cycles` reports it, while `dep add X X` refuses the same edge (CYCLE)
    and br refuses the file ('Issue cannot depend on itself')."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.machine = support.Machine(self.box, "self")
        self.machine.bead("init", "--prefix", "ex")

    def test_import_refuses_a_self_dependency_whole(self):
        """G46: an import whose record depends on itself fails whole with VALIDATION and writes nothing,
        whatever the dependency type, as `dep add X X` and br refuse it."""
        for n, kind in enumerate(("blocks", "related", "parent-child")):
            with self.subTest(kind=kind):
                path = self.box.root / f"self-{kind}.jsonl"
                path.write_text(json.dumps(_g46_record(f"ex-ok{n}")) + "\n"
                                + json.dumps(_g46_record(f"ex-a{n}", (f"ex-a{n}", kind))) + "\n")
                proc = self.machine.run("import", str(path), "--json")
                self.assertNotEqual(proc.returncode, 0, f"a {kind} self-dependency was imported: {proc.stdout}")
                self.assertEqual(support.code(proc), "VALIDATION")
                self.assertEqual(self.machine.bead("list", "--all", "--limit", "0")["total"], 0,
                                 "a failed import wrote records")
        self.assertEqual(self.machine.bead("dep", "cycles")["count"], 0, "imported self-dependencies form cycles")


def _g47_loose(root: Path) -> list[str]:
    """Every path under root (root included) that group or others can read, write or enter."""
    out = []
    for path in [root, *root.rglob("*")]:
        mode = os.lstat(path).st_mode
        if stat.S_IMODE(mode) & 0o077:
            out.append(f"{stat.filemode(mode)} {path.relative_to(root)}")
    return out

class G47_StoreIsPrivateByItsOwnModes(unittest.TestCase):
    """G47: the store is not 'private by its own modes (0700/0600)': events/, events/<m>/, claims/, pending/,
    lock, *.torn-*, *.fork-* and all of sync.git (FETCH_HEAD with the remote URL, the loose objects holding
    every event) follow the umask; only BEAD_HOME and <ws> are chmodded 0700."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.addCleanup(os.umask, os.umask(0o022))

    def test_g47_every_store_path_is_owner_only_under_umask_022(self):
        """G47: after init, create, an online claim, syncs with a second machine, a torn tail and a damaged
        foreign segment, every path under BEAD_HOME is owner-only (0700 directories, 0600/0400 files)."""
        a, b = support.Machine(self.box, "g47-a"), support.Machine(self.box, "g47-b")
        a.bead("init", "--prefix", "ex", "--remote", str(self.box.origin))
        key = a.bead("create", "one")["id"]
        a.bead("claim", key)  # online: sync.git, claims/, pending/
        b.bead("sync")
        b.bead("create", "from b")
        b.bead("sync")
        a.bead("sync")
        own = json.loads((a.home / "replica.json").read_text())["id"]
        foreign = json.loads((b.home / "replica.json").read_text())["id"]
        segment = sorted(a.store().glob(f"events/{own}/*.jsonl"))[-1]
        with open(segment, "ab") as fh:
            fh.write(b'{"torn')
        a.bead("create", "after torn")  # *.torn-*
        theirs = sorted(a.store().glob(f"events/{foreign}/*.jsonl"))[-1]
        theirs.write_bytes(theirs.read_bytes().replace(b"from b", b"FROM B"))
        a.bead("sync")  # *.fork-*
        self.assertTrue(list(a.store().glob("events/*/*.torn-*")), "the scenario made no torn fragment")
        self.assertTrue(list(a.store().glob("events/*/*.fork-*")), "the scenario made no fork copy")
        loose = _g47_loose(a.home)
        self.assertEqual(loose, [], f"{len(loose)} store paths are not owner-only")

    def test_g47_machines_name_as_first_command_makes_a_private_home(self):
        """G47: `machines --name` as the first command in a fresh BEAD_HOME creates it 0755."""
        m = support.Machine(self.box, "g47-first")
        named = m.run("machines", "--name", "deadbeef=laptop", "--json")
        self.assertEqual(named.returncode, 0, named.stderr)
        self.assertEqual(_g47_loose(m.home), [])

    def test_g47_two_torn_fragments_in_one_second_are_both_kept(self):
        """G47: (secondary) torn and fork copies are named by whole seconds, so a second fragment of the same
        segment within one second overwrites the first one."""
        bead = support.load_module()
        folder = Path(self.box.root / "torn")
        folder.mkdir()
        segment = folder / "000001.jsonl"
        with mock.patch.object(bead.time, "time", return_value=1791394588.25):
            for fragment in (b'{"first', b'{"second'):
                segment.write_bytes(b'{"v":1}\n' + fragment)
                bead.repair_tail(segment)
        kept = sorted(p.read_bytes() for p in folder.glob("000001.jsonl.torn-*"))
        self.assertEqual(kept, [b'{"first', b'{"second'], "a torn fragment was overwritten by a later one")


def _g48_error(proc):
    """(code, message) of a failed `--json` call; a traceback or other output is reported as such."""
    try:
        error = json.loads(proc.stderr)["error"]
        return error["code"], error["message"]
    except (ValueError, KeyError, TypeError):
        return ("TRACEBACK" if "Traceback" in proc.stderr else f"rc {proc.returncode}"), proc.stderr

def _g48_limited(machine, kib, *args):
    """Run bead with a small file-size limit (EFBIG part-way through a write, as on a full volume). Python
    ignores SIGXFSZ, so the write fails with an OSError instead of killing the process."""
    return subprocess.run(["sh", "-c", f'ulimit -f {kib} && exec "$0" "$@"', support.BEAD, *args, "--json"],
                          cwd=machine.checkout, env=machine.env(), capture_output=True, text=True)

class G48_FailedAtomicWriteLeavesNothingBehind(unittest.TestCase):
    """G48: write_atomic leaves `.NAME.PID.tmp` behind when the write, fsync or rename fails: beside an
    `export -o` target in the user's checkout, and under pending/ and events/ in the store."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.m = support.Machine(self.box, "g48")
        self.m.bead("init", "--prefix", "ex")
        self.keys = [self.m.bead("create", f"issue {n} " + "x" * 100)["id"] for n in range(6)]

    def leftovers(self, root):
        return sorted(str(p.relative_to(root)) for p in root.rglob(".*.tmp"))

    def test_g48_export_onto_a_directory_leaves_no_copy_and_names_the_target(self):
        """G48: `export -o DIR` fails at the rename; a full copy of the export stays in the checkout as
        `.DIR.PID.tmp`, and the IO message names that temporary file instead of DIR."""
        (self.m.checkout / "out").mkdir()
        failed = self.m.run("export", "-o", "out", "--json")
        code, message = _g48_error(failed)
        self.assertEqual(code, "IO", message)
        self.assertEqual(self.leftovers(self.m.checkout), [], "a failed export left a temporary copy in the checkout")
        self.assertNotIn(".tmp", message, "the error names a temporary file the user never gave")
        self.assertIn("out", message)

    def test_g48_export_that_cannot_be_written_whole_leaves_nothing(self):
        """G48: an export that fails part way (file-size limit standing in for a full volume) leaves the
        partial `.NAME.PID.tmp` in the checkout, and a later successful export does not remove it."""
        failed = _g48_limited(self.m, 1, "export", "-o", "big.jsonl")
        code, message = _g48_error(failed)
        self.assertEqual(code, "IO", message)
        self.assertFalse((self.m.checkout / "big.jsonl").exists())
        again = self.m.run("export", "-o", "big.jsonl")
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(self.leftovers(self.m.checkout), [], "a failed export left a partial copy in the checkout")

    def test_g48_failed_store_write_leaves_no_temporary_file(self):
        """G48: a claim whose receipt or attempt cannot be written whole leaves `.ID.command|json.PID.tmp`
        under pending/; nothing ever sweeps it, not even a later successful sync."""
        big = self.box.root / "description.txt"
        big.write_text("d" * 100_000)
        failed = _g48_limited(self.m, 16, "update", self.keys[0], "--claim", "--description-file", str(big))
        code, message = _g48_error(failed)
        self.assertEqual(code, "IO", message)
        synced = self.m.run("sync", "--json")
        self.assertEqual(synced.returncode, 0, synced.stderr)
        self.assertEqual(self.leftovers(self.m.store()), [], "a failed store write left its temporary file")


def _g49_answered(test, proc, what):
    """No traceback: the command either succeeded or gave the structured JSON error envelope."""
    test.assertNotIn("Traceback", proc.stderr, what)
    if proc.returncode:
        try:
            error = json.loads(proc.stderr)["error"]
        except (ValueError, KeyError, TypeError):
            test.fail(f"{what}: exit {proc.returncode} without a structured error: {proc.stderr[-300:]}")
        test.assertIsInstance(error.get("code"), str, what)

class G49_LocalStateShapesTraceback(unittest.TestCase):
    """G49: F13 shape-checks meta.json and replica.json only partly. A non-string `remote` in meta.json
    reaches re.fullmatch in Workspace.remote (TypeError), and a replica.json that is valid JSON but not
    an object reaches own.get in machine_aliases (AttributeError): both end in tracebacks."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "a")
        self.a.bead("init", "--prefix", "ex")
        self.key = self.a.bead("create", "x")["id"]
        self.a.bead("sync")

    def test_non_string_meta_remote_is_not_a_traceback(self):
        """G49: meta.json {"remote": 5} (or a list, true, an object) makes info, sync, claim, release and an
        owner's close (after the close is recorded) end in a TypeError traceback."""
        meta = self.a.store() / "meta.json"
        saved = meta.read_text()
        for body in ('{"remote": 5}', '{"remote": ["x"]}', '{"remote": true}', '{"remote": {"a": 1}}'):
            meta.write_text(body)
            for argv in (["info"], ["sync"], ["claim", self.key], ["release", self.key]):
                with self.subTest(meta=body, argv=argv):
                    _g49_answered(self, self.a.run(*argv, "--json"), f"meta={body} {argv}")
        meta.write_text(saved)
        self.a.bead("claim", self.key)
        meta.write_text('{"remote": 5}')
        closed = self.a.run("close", self.key)  # the close is written locally first, then published
        meta.write_text(saved)
        _g49_answered(self, closed, "owner close with a non-string meta remote")
        self.assertEqual(closed.returncode, 0, "a close that was recorded must not be reported as failed")
        self.assertIn("not yet published", closed.stdout)
        self.assertEqual(self.a.show(self.key)["status"], "closed")

    def test_replica_json_that_is_not_an_object_is_not_a_traceback(self):
        """G49: replica.json holding a JSON list, number, null or string makes claims, machines and log end
        in an AttributeError traceback."""
        self.a.bead("claim", self.key)
        replica = self.a.home / "replica.json"
        saved = replica.read_text()
        try:
            for body in ('["x"]', "5", "null", '"s"'):
                replica.write_text(body)
                for argv in (["claims"], ["machines"], ["log"]):
                    with self.subTest(replica=body, argv=argv):
                        _g49_answered(self, self.a.run(*argv, "--json"), f"replica={body} {argv}")
        finally:
            replica.write_text(saved)


class G50_MachinesNameKeepsAliasesItCouldNotRead(unittest.TestCase):
    """G50: `machines --name` treats an aliases.json it cannot read or parse as empty and replaces it with a
    file holding only the new name: every other machine name is lost, with exit 0."""

    OTHERS = {"abcd0001": "desktop", "abcd0002": "ci"}

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.machine = support.Machine(self.box, "g50")
        self.machine.bead("init", "--prefix", "ex")
        self.aliases = self.machine.home / "aliases.json"
        self.addCleanup(lambda: self.aliases.exists() and os.chmod(self.aliases, 0o600))

    def name_over(self, text, mode=0o600):
        self.aliases.write_text(text, encoding="utf-8")
        os.chmod(self.aliases, mode)
        named = self.machine.run("machines", "--name", "abcd0003=new", "--json")
        os.chmod(self.aliases, 0o600)
        return named, self.aliases.read_text(encoding="utf-8")

    def check_kept(self, label, text, mode=0o600):
        named, after = self.name_over(text, mode)
        self.assertNotIn("Traceback", named.stderr, label)
        if named.returncode:
            # Refused: a structured error and the file left exactly as it was.
            self.assertIn("error", json.loads(named.stderr), label)
            self.assertEqual(after, text, f"{label}: refused, but aliases.json was rewritten anyway")
        else:
            # Accepted: every name that was there survives.
            for ident, alias in self.OTHERS.items():
                self.assertIn(f'"{alias}"', after, f"{label}: {ident}={alias} lost; aliases.json is now {after!r}")

    def test_g50_hand_edited_file_with_a_trailing_comma_is_not_overwritten(self):
        """G50: a hand edit that leaves a trailing comma (or a BOM) makes `machines --name` drop every other
        name; a well-formed file is merged."""
        named, after = self.name_over(json.dumps(self.OTHERS, indent=1) + "\n")
        self.assertEqual(named.returncode, 0, named.stderr)
        self.assertEqual(json.loads(after), {**self.OTHERS, "abcd0003": "new"})
        for label, text in (("trailing comma", '{\n "abcd0001": "desktop",\n "abcd0002": "ci",\n}\n'),
                            ("byte order mark", "\ufeff" + json.dumps(self.OTHERS, indent=1) + "\n")):
            with self.subTest(label):
                self.check_kept(label, text)

    def test_g50_wrong_shape_is_not_overwritten(self):
        """G50: an aliases.json that is not an object is replaced by one holding only the new name."""
        self.check_kept("list", '[["abcd0001", "desktop"], ["abcd0002", "ci"]]\n')

    @unittest.skipIf(os.geteuid() == 0, "root reads mode-0 files")
    def test_g50_unreadable_file_is_not_overwritten(self):
        """G50: an aliases.json bead cannot read (EACCES) is replaced, since os.replace needs only the
        directory to be writable."""
        self.check_kept("EACCES", json.dumps(self.OTHERS, indent=1) + "\n", mode=0)


def _g51_code(proc):
    try:
        return json.loads(proc.stderr)["error"]["code"]
    except (ValueError, KeyError, TypeError):
        return "TRACEBACK" if "Traceback" in proc.stderr else None

class G51_UnusableTrackedConfigIsAnError(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "g51")
        self.config = self.a.checkout / ".beadroll.json"
        self.config.write_text(json.dumps({"workspace": "w-tracked"}))
        self.a.bead("init", "--prefix", "ex", "--offline")  # local: origin must hold no tracker at all for these tests
        self.a.bead("create", "tracked issue", "--offline")

    def _break(self, kind):
        self.config.unlink()
        os.symlink(".beadroll.json" if kind == "loop" else "missing-config.json", self.config)

    def test_unresolvable_config_link_is_config_not_absent(self):
        """G51: a .beadroll.json that is a looping or dangling symlink is treated as absent: the workspace
        silently becomes the origin-derived one instead of a CONFIG error."""
        for kind in ("loop", "dangling"):
            with self.subTest(kind=kind):
                self._break(kind)
                for command in ("info", "list", "sync"):
                    proc = self.a.run(command, "--json")
                    found = json.loads(proc.stdout).get("workspace") if proc.returncode == 0 else None
                    self.assertEqual((proc.returncode, _g51_code(proc)), (1, "CONFIG"),
                                     f"{command} with a {kind} .beadroll.json link: rc={proc.returncode} "
                                     f"workspace={found!r} {proc.stderr.strip()[:200]}")

    def test_following_the_hint_under_a_dangling_link_does_not_start_a_second_tracker(self):
        """G51: under a dangling .beadroll.json link, `bead init` (the hint bead gives) creates a second tracker
        under the origin-derived id and `sync` publishes it to origin."""
        self._break("dangling")
        self.a.run("init", "--prefix", "ex", "--json")
        self.a.run("create", "written after the link broke", "--json")
        self.a.run("sync", "--json")
        published = support.git("for-each-ref", "--format=%(refname)", "refs/beads", cwd=self.box.origin)
        self.assertEqual(published.strip(), "",
                         "a second tracker was created and published under the origin-derived workspace")

    def test_empty_workspace_is_refused(self):
        """G51: `"workspace": ""` in .beadroll.json is accepted and silently means the origin-derived workspace."""
        b = support.Machine(self.box, "g51-empty")
        (b.checkout / ".beadroll.json").write_text(json.dumps({"workspace": ""}))
        proc = b.run("info", "--json")
        found = json.loads(proc.stdout).get("workspace_source") if proc.returncode == 0 else None
        self.assertEqual((proc.returncode, _g51_code(proc)), (1, "CONFIG"),
                         f"an empty tracked workspace was accepted (workspace_source={found!r})")


def _g52_error(proc):
    """(code, message) of a failed `--json` call, or (None, stdout) when it succeeded."""
    if proc.returncode == 0:
        return None, proc.stdout
    try:
        error = json.loads(proc.stderr)["error"]
        return error["code"], error["message"]
    except (ValueError, KeyError, TypeError):
        return ("TRACEBACK" if "Traceback" in proc.stderr else "UNSTRUCTURED"), proc.stderr

def _g52_own_journal(machine):
    """This machine's own journal directory: events/<replica id from replica.json>."""
    replica = json.loads((machine.home / "replica.json").read_text())["id"]
    return machine.store() / "events" / replica

@unittest.skipIf(os.geteuid() == 0, "root ignores directory modes")
class G52_UnreadableStoreIsIO(unittest.TestCase):
    """G52: segment_files/claim_files use glob() (and, on Python 3.14, Path.is_dir()), which turn a store
    directory bead cannot read, or a store path that is not a directory, into 'no journal here'. Readers then
    say NOT_INITIALIZED ('run bead init'), info exits 0 with no issues, and an unreadable claims/ makes a
    claimed issue ready."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def unreadable(self, path, mode=0):
        old = path.stat().st_mode & 0o777
        path.chmod(mode)
        self.addCleanup(path.chmod, old)

    def test_g52_unlistable_own_journal_is_io_not_no_tracker(self):
        """G52: with this machine's journal directory unreadable, show says 'no tracker yet: run bead init'
        (every Python version) instead of an IO error naming the directory."""
        a = support.Machine(self.box, "g52-a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "x")["id"]
        journal = _g52_own_journal(a)
        self.unreadable(journal)
        code, message = _g52_error(a.run("show", key, "--json"))
        self.assertEqual(code, "IO", message)
        self.assertIn(str(journal), message)

    def test_g52_unlistable_claims_do_not_turn_a_claimed_issue_ready(self):
        """G52: with claims/ unreadable, the claimed issue is shown open and listed by ready (exit 0)."""
        a = support.Machine(self.box, "g52-a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "x")["id"]
        a.bead("update", key, "--claim")
        self.assertEqual(a.show(key)["status"], "in_progress")
        self.unreadable(_g52_own_journal(a) / "claims")
        code, message = _g52_error(a.run("ready", "--json"))
        self.assertEqual(code, "IO", f"ready answered from a journal it could not read: {message}")

    def test_g52_create_into_an_unlistable_journal_is_refused_not_lost(self):
        """G52: a create acknowledged while the own journal cannot be listed is not there afterwards (it
        starts a second chain, reported as a copied machine identity)."""
        a, b = support.Machine(self.box, "g52-a"), support.Machine(self.box, "g52-b")
        a.bead("init", "--prefix", "ex")
        a.bead("sync")
        b.bead("sync")
        b.bead("create", "first")
        self.unreadable(_g52_own_journal(b), 0o300)  # writable and searchable, not listable
        code, message = _g52_error(b.run("create", "second", "--json"))
        if code is None:  # acknowledged: it must then exist
            key = json.loads(message)["id"]
            os.chmod(_g52_own_journal(b), 0o700)
            shown = b.run("show", key, "--json")
            self.assertEqual(shown.returncode, 0, f"create returned {key} but it is gone: {shown.stderr.strip()}")
        self.assertEqual(code, "IO", message)

    def test_g52_store_that_is_not_a_directory_is_io(self):
        """G52: BEAD_HOME naming a regular file gives NOT_INITIALIZED for show and exit 0 with no issues for
        info, although create on the same store fails with IO."""
        a = support.Machine(self.box, "g52-a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "x")["id"]
        not_a_dir = self.box.root / "home-file"
        not_a_dir.write_text("not a directory\n")
        a.extra["BEAD_HOME"] = str(not_a_dir)
        for args in (("show", key), ("info",)):
            with self.subTest(args[0]):
                code, message = _g52_error(a.run(*args, "--json"))
                self.assertEqual(code, "IO", message)

    @unittest.skipUnless(sys.version_info >= (3, 14), "Path.is_dir() swallows EACCES only from Python 3.14")
    def test_g52_unenterable_store_is_io_not_no_tracker(self):
        """G52: chmod 0 on the workspace store: show says NOT_INITIALIZED and info exits 0 with no issues
        (Python 3.14, where Path.is_dir() returns False on EACCES)."""
        a = support.Machine(self.box, "g52-a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "x")["id"]
        self.unreadable(a.store())
        for args in (("show", key), ("info",)):
            with self.subTest(args[0]):
                code, message = _g52_error(a.run(*args, "--json"))
                self.assertEqual(code, "IO", message)


def _g53_error(proc):
    """(code, message) of a failed `--json` call, or ("TRACEBACK", stderr) when it crashed."""
    if "Traceback" in proc.stderr:
        return "TRACEBACK", proc.stderr
    try:
        error = json.loads(proc.stderr)["error"]
        return error["code"], error["message"]
    except (ValueError, KeyError, TypeError):
        return None, proc.stderr

class G53_UnreadableIdentityIsIo(unittest.TestCase):
    """G53: replica_id maps every failure to read replica.json, a permission error included, to
    IDENTITY "... is unreadable; restore it, or move it aside to mint a new machine identity".
    Following that advice for what is only a mode problem changes every actor label on the
    machine, so its agents can no longer close or release the claims they hold."""

    def setUp(self):
        if hasattr(os, "geteuid") and os.geteuid() == 0:
            self.skipTest("root reads a mode-000 file")
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "a")
        self.a.bead("init", "--prefix", "gi")
        self.key = self.a.bead("create", "x")["id"]
        self.a.bead("claim", self.key)
        self.replica = self.a.home / "replica.json"

    def test_g53_unreadable_identity_is_io_and_does_not_advise_a_new_identity(self):
        """G53: a permission error on replica.json is IO, and the advice is not to mint a new identity
        (which would orphan this machine's claims); fixing the mode is the whole cure."""
        self.replica.chmod(0)
        try:
            closed = self.a.run("close", self.key, "--json")
        finally:
            self.replica.chmod(0o600)
        self.assertEqual(closed.returncode, 1, closed.stderr)
        code, message = _g53_error(closed)
        self.assertEqual((code, "mint a new" in message), ("IO", False),
                         f"a permission problem is reported as a damaged identity: {closed.stderr.strip()}")
        again = self.a.run("close", self.key, "--json")
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(self.a.show(self.key)["status"], "closed")

    def test_g53_guard_undecodable_identity_stays_a_structured_identity_error(self):
        """G53: guard for the fix (passes before it too): a replica.json that is not UTF-8 is still a damaged
        identity (F13), not a traceback."""
        self.replica.write_bytes(b'{"id": "\xff\xfe"}\n')
        closed = self.a.run("close", self.key, "--json")
        self.assertEqual(_g53_error(closed)[0], "IDENTITY", closed.stderr)


class G54_StaleTwinRuleDropsTheReceipt(unittest.TestCase):
    """Link.merge's stale-twin rule (F11) calls drop_pending, which also deletes the receipt of the
    attempt the remote accepted; the retry of that accepted command is then not recognised. Needs a
    twin that differs from the accepted copy, which only local damage produces."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_damaged_twin_of_an_accepted_release_keeps_its_receipt(self):
        """G54: after a crash between confirming an accepted release and cleaning up, with the pending
        twin damaged, the retried `release X` is recognised (exit 0), not NOT_OWNER."""
        a = support.Machine(self.box, "g54-twin")
        a.bead("init", "--prefix", "ex")
        x = a.bead("create", "X")["id"]
        a.bead("claim", x)
        with mock.patch.dict(os.environ, a.env(), clear=True), contextlib.chdir(a.checkout):
            bp = support.load_module()
            ws, actor = bp.find_workspace(), bp.detect_actor(None)
            args = bp.build_parser().parse_args(["release", x])
            with support.patched("drop_pending", mock.MagicMock(side_effect=RuntimeError("crash after local confirm"))):
                with self.assertRaises(RuntimeError):
                    bp.cmd_release(args, ws, actor)
        pending = a.store() / "pending"
        self.assertEqual(len(list(pending.glob("*.command"))), 1)
        twin = next(pending.glob("*.json"))
        twin.write_bytes(twin.read_bytes() + b"\n")  # local damage: the twin no longer matches the accepted copy
        retry = a.run("release", x, "--json")
        self.assertEqual(retry.returncode, 0, f"retry of the accepted release not recognised: {retry.stderr}")
        self.assertIsNone(a.show(x).get("claim"))


class G55_ReceiptThatIsNotAnObjectBlocksEveryClaim(unittest.TestCase):
    """settle_commands calls recorded.get() on whatever valid JSON a receipt holds; a receipt holding
    `[]` makes every coordinated command of every actor using that store a traceback, and sync never
    sweeps it (load_pending sweeps *.json only)."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_list_receipt_does_not_block_claims(self):
        """G55: a pending/*.command receipt holding valid JSON that is not an object is dropped like an
        unreadable one; `claim X` of any actor goes through instead of an AttributeError traceback."""
        a = support.Machine(self.box, "g55-shape")
        a.bead("init", "--prefix", "ex")
        x = a.bead("create", "X")["id"]
        y = a.bead("create", "Y")["id"]
        a.bead("sync")
        pending = a.store() / "pending"
        pending.mkdir(exist_ok=True)
        receipt = pending / "01900000-0000-7000-8000-0000000000bb.command"
        receipt.write_text("[]")
        self.assertEqual(a.run("sync").returncode, 0)
        self.assertTrue(receipt.exists())  # sync does not sweep it
        claim = a.run("claim", x, "--json", thread="g55-other-thread")
        self.assertNotIn("Traceback", claim.stderr)
        self.assertEqual(claim.returncode, 0, claim.stderr)
        mine = a.run("claim", y, "--json")
        self.assertEqual(mine.returncode, 0, mine.stderr)
        self.assertFalse(receipt.exists(), "the receipt that is not an object was kept")


def _g56_tail(text):
    lines = [line for line in text.strip().split("\n") if line.strip()]
    return lines[-1] if lines else ""

class G56_ClosedStdinIsAStructuredError(unittest.TestCase):
    def test_closed_stdin_for_every_dash_reader_is_a_structured_error(self):
        """G56: '-' names standard input; with fd 0 closed (sys.stdin is None) `comments add -f -`,
        `update/create --description-file -` and `import -` crash with AttributeError instead of a
        structured error, as they already give when fd 0 is open but unreadable (IO)."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        m = support.Machine(box, "g56")
        m.bead("init", "--prefix", "cs")
        key = m.bead("create", "seed")["id"]
        before = m.export()
        cases = {
            "comments add -f -": ["comments", "add", key, "-f", "-"],
            "update --description-file -": ["update", key, "--description-file", "-"],
            "create --description-file -": ["create", "other", "--description-file", "-"],
            "import -": ["import", "-"],
        }
        for label, args in cases.items():
            with self.subTest(label):
                command = " ".join(shlex.quote(a) for a in [support.BEAD, *args, "--json"]) + " <&-"
                proc = subprocess.run(["/bin/sh", "-c", command], cwd=m.checkout, env=m.env(),
                                      capture_output=True, text=True)
                if "Traceback" in proc.stderr:
                    self.fail(f"{label} with stdin closed crashed: {_g56_tail(proc.stderr)}")
                self.assertEqual(proc.returncode, 1, proc.stderr)
                self.assertIn(json.loads(proc.stderr)["error"]["code"], ("IO", "VALIDATION"))
        self.assertEqual(m.export(), before)
        # an open stdin still works
        piped = subprocess.run([support.BEAD, "update", key, "--description-file", "-", "--json"], cwd=m.checkout,
                               env=m.env(), input="from stdin", capture_output=True, text=True)
        self.assertEqual(piped.returncode, 0, piped.stderr)
        self.assertEqual(m.show(key)["description"], "from stdin")


def _g57_tail(text):
    lines = [line for line in text.strip().split("\n") if line.strip()]
    return lines[-1] if lines else ""

def _g57_event_id(n):
    """A UUIDv7 for now, with `n` in its random bits."""
    h = f"{(int(time.time() * 1000) << 80) | (0x7 << 76) | (0b10 << 62) | n:032x}"
    return f"{h[:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:]}"

def _g57_create(n, replica, key, title):
    eid = _g57_event_id(n)
    return {"v": 1, "id": eid, "replica": replica, "deps": [], "actor": {"agent": "human"}, "op": "create",
            "issue": eid, "data": {"key": key, "fields": {"title": title}}}

class G57_LoneSurrogateInAJournalIsScoped(unittest.TestCase):
    def test_export_to_a_file_survives_a_lone_surrogate_in_a_journal(self):
        """G57: a journal event whose text holds a lone surrogate (escaped \\ud800, or raw bytes ED A0 80;
        bead itself refuses to write one) replays unreported, then `export -o` dies with UnicodeEncodeError
        and `export` to stdout writes '?'. It must be reported, and both exports must work and agree."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        m = support.Machine(box, "g57")
        m.bead("init", "--prefix", "ex")
        m.bead("create", "ordinary work")
        escaped = _g57_create(1, "fffffff0", "ex-esc", "bad\ud800title")
        raw = _g57_create(2, "fffffff1", "ex-raw", "raw\ud800title")
        for replica, line in (("fffffff0", json.dumps(escaped).encode()),
                              ("fffffff1", json.dumps(raw, ensure_ascii=False).encode("utf-8", "surrogatepass"))):
            folder = m.store() / "events" / replica
            folder.mkdir(parents=True)
            (folder / "000001.jsonl").write_bytes(line + b"\n")
        out = box.root / "out.jsonl"
        proc = m.run("export", "-o", str(out), "--json")
        if "Traceback" in proc.stderr:
            self.fail(f"export -o crashed: {_g57_tail(proc.stderr)}")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        written = out.read_bytes()
        records = [json.loads(row) for row in written.decode("utf-8").split("\n") if row.strip()]
        titles = {r["id"]: r["title"] for r in records}
        stdout = m.run("export")
        self.assertEqual(stdout.stdout.encode(), written, "export to stdout and export -o disagree")
        self.assertEqual((titles.get("ex-esc"), titles.get("ex-raw")), ("bad\ufffdtitle", "raw\ufffdtitle"))
        origins = [str(row.get("origin")) for row in m.bead("info")["integrity"]]
        for replica in ("fffffff0", "fffffff1"):
            self.assertTrue(any(f"{replica}/000001.jsonl:1" in o for o in origins), f"{replica} not reported: {origins}")

    def test_attempt_file_with_a_lone_surrogate_reads_as_text_that_encodes(self):
        """G57: the same for an attempt file (claims/*.json): its events must be printable and exportable."""
        bp = support.load_module()
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        claim = {"v": bp.FORMAT, "id": _g57_event_id(3), "replica": "fffffff0", "deps": [],
                 "actor": {"agent": "human", "thread": "t\ud800"}, "op": "claim", "issue": _g57_event_id(4),
                 "data": {"generation": 1}}
        path = box.root / f"{claim['id']}.json"
        path.write_text(json.dumps(claim) + "\n")
        problems = []
        events = bp.read_attempt(path, problems)
        self.assertEqual(len(events), 1, problems)
        try:
            json.dumps(events, ensure_ascii=False).encode("utf-8")
            unwritable = None
        except UnicodeEncodeError as error:
            unwritable = str(error)
        self.assertIsNone(unwritable, "an attempt event read back holds text that cannot be written")
        self.assertEqual(events[0]["actor"]["thread"], "t\ufffd")
        self.assertTrue(problems, "the bad text was not reported")


def _g58_ssh_script():
    """A transport whose background job keeps git's stderr open after git has exited."""
    return ("#!/bin/sh\n"
            "if [ \"$1\" = \"-G\" ]; then exit 1; fi\n"
            "for last; do :; done\n"
            "case \"$last\" in\n"
            "  git-receive-pack*) (sleep \"$G58_BG_SECONDS\") & echo \"$!\" >> \"$G58_PIDS\"; eval \"exec $last\" ;;\n"
            "  git-upload-pack*) eval \"exec $last\" ;;\n"
            "esac\nexit 2\n")

def _g58_kill_helpers(pids):
    for pid in pids.read_text().split():
        try:
            os.kill(int(pid), signal.SIGKILL)
        except (OSError, ValueError):
            pass

def _g58_remote_head(box):
    return subprocess.run(["git", "--git-dir", str(box.origin), "rev-parse", "refs/beads/journal"],
                          capture_output=True, text=True).stdout.strip()

class G58_FinishedPushIsNotAnUnansweredOne(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.pids = self.box.root / "g58-helper-pids"
        self.pids.write_text("")
        self.addCleanup(_g58_kill_helpers, self.pids)

    def test_finished_push_is_not_reported_unanswered_when_a_helper_keeps_stderr(self):
        """G58: run_git waits for end-of-file on git's pipes, not for git. When a transport helper's background
        child keeps git's stderr open, a push that git finished (and the remote applied) runs into
        BEAD_NET_TIMEOUT, its exit status is thrown away, and sync answers REMOTE_UNREACHABLE 'the push got no
        answer'."""
        ssh = self.box.root / "g58-ssh.sh"
        ssh.write_text(_g58_ssh_script())
        ssh.chmod(0o755)
        config = self.box.root / "g58-empty.gitconfig"
        config.write_text("")
        a = support.Machine(self.box, "g58")
        a.extra.update(GIT_CONFIG_GLOBAL=str(config), GIT_CONFIG_NOSYSTEM="1", GIT_SSH_COMMAND=str(ssh),
                       G58_PIDS=str(self.pids), G58_BG_SECONDS="0", BEAD_NET_TIMEOUT="4",
                       BEAD_REMOTE=f"ssh://fakehost{self.box.origin}")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "one")["id"]
        a.bead("sync")
        a.bead("comments", "add", key, "publish me", "--offline")  # the sync below, under the slow helper, publishes it
        a.extra["G58_BG_SECONDS"] = "15"
        before = _g58_remote_head(self.box)
        started = time.monotonic()
        synced = a.run("sync", "--json")
        took = time.monotonic() - started
        self.assertNotEqual(_g58_remote_head(self.box), before, "the push did not reach the remote")
        self.assertEqual(synced.returncode, 0,
                         f"git finished and the remote applied the push, but after {took:.1f}s bead said: "
                         + synced.stderr.strip())
        self.assertLess(took, 4, "bead waited for the helper, not for git")


def _g59_env(machine, **more):
    env = machine.env()
    for name in ("NO_COLOR", "FORCE_COLOR", "PYTHON_COLORS", "TERM"):
        env.pop(name, None)
    env.update(more)
    return env

def _g59_skip_without_argparse_colour(test, env):
    """argparse colours help only from Python 3.14 on; bead runs under the python3 its shebang finds."""
    probe = subprocess.run(["/usr/bin/env", "python3", "-c",
                            "import argparse, sys; sys.exit(0 if hasattr(argparse.ArgumentParser(add_help=False), 'color') else 1)"],
                           env=env, capture_output=True, text=True)
    if probe.returncode:
        test.skipTest("the python3 that runs bead has no argparse colour (before 3.14)")

def _g59_run_on_terminal(argv, cwd, env, timeout=30):
    """Run argv with stdin, stdout and stderr on a fresh pseudo-terminal and return what it wrote."""
    master, slave = os.openpty()
    chunks = []

    def drain():
        while True:
            try:
                chunk = os.read(master, 65536)
            except OSError:  # EIO once the slave side is gone
                break
            if not chunk:
                break
            chunks.append(chunk)

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    try:
        proc = subprocess.Popen(argv, cwd=cwd, env=env, stdin=slave, stdout=slave, stderr=slave)
    finally:
        os.close(slave)
    try:
        status = proc.wait(timeout=timeout)
        reader.join(timeout=timeout)
    finally:
        if proc.poll() is None:
            proc.kill()
        os.close(master)
    return status, b"".join(chunks).decode("utf-8", errors="replace").replace("\r\n", "\n")

class G59_HelpIsPlainText(unittest.TestCase):
    longMessage = False  # the help page is long; the messages below quote its start

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.machine = support.Machine(self.box, "help")

    def test_help_command_on_a_terminal_shows_no_colour_codes(self):
        """G59: on Python 3.14, `bead help create` on a terminal prints argparse's colour codes through
        visible(), so the user sees literal \\x1b[1;34m text instead of `usage: bead create`."""
        env = _g59_env(self.machine, TERM="xterm-256color")
        _g59_skip_without_argparse_colour(self, env)
        status, out = _g59_run_on_terminal([support.BEAD, "help", "create"], self.machine.checkout, env)
        self.assertEqual(status, 0, out)
        self.assertNotIn("\\x1b[", out, "help on a terminal shows escaped colour codes: " + repr(out[:100]))
        self.assertNotIn("\x1b[", out)
        self.assertIn("usage: bead create", out)

    def test_command_dash_help_on_a_terminal_writes_no_escape_sequences(self):
        """G59: on Python 3.14, `bead create --help` on a terminal writes raw SGR sequences, unlike every other
        bead output."""
        env = _g59_env(self.machine, TERM="xterm-256color")
        _g59_skip_without_argparse_colour(self, env)
        status, out = _g59_run_on_terminal([support.BEAD, "create", "--help"], self.machine.checkout, env)
        self.assertEqual(status, 0, out)
        self.assertNotIn("\x1b", out, "create --help on a terminal wrote escape sequences: " + repr(out[:100]))
        self.assertIn("usage: bead create", out)

    def test_forced_colour_reaches_neither_help_text_nor_json(self):
        """G59: with FORCE_COLOR=1 (common in CI) on Python 3.14, `help create` through a pipe shows escaped
        colour codes and `--json help create` embeds raw ANSI codes in the help string."""
        env = _g59_env(self.machine, FORCE_COLOR="1")
        _g59_skip_without_argparse_colour(self, env)
        text = subprocess.run([support.BEAD, "help", "create"], cwd=self.machine.checkout, env=env,
                              capture_output=True, text=True, timeout=30)
        self.assertEqual(text.returncode, 0, text.stderr)
        self.assertNotIn("\\x1b[", text.stdout, "help through a pipe shows escaped colour codes: " + repr(text.stdout[:100]))
        self.assertIn("usage: bead create", text.stdout)
        as_json = subprocess.run([support.BEAD, "--json", "help", "create"], cwd=self.machine.checkout, env=env,
                                 capture_output=True, text=True, timeout=30)
        self.assertEqual(as_json.returncode, 0, as_json.stderr)
        page = json.loads(as_json.stdout)["help"]
        self.assertEqual(page.count("\x1b"), 0, "--json help carries ANSI codes: " + repr(page[:100]))


class G60_ArabicLetterMarkEscaped(unittest.TestCase):
    def test_arabic_letter_mark_is_escaped_like_the_other_bidi_controls(self):
        """G60: U+061C ARABIC LETTER MARK, the one Unicode Bidi_Control character missing from CONTROLS,
        reaches the terminal raw in create, list, show and error output, while U+200F beside it is escaped."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        machine = support.Machine(box, "bidi")
        machine.bead("init", "--prefix", "ex")
        alm, rlm = "\u061c", "\u200f"  # ARABIC LETTER MARK, RIGHT-TO-LEFT MARK
        created = machine.run("create", f"alm{alm}mark rlm{rlm}mark", "-d", f"desc{alm}d")
        self.assertEqual(created.returncode, 0, created.stderr)
        key = created.stdout.split()[2].rstrip(":")
        outputs = {"create": created.stdout, "list": machine.run("list").stdout,
                   "show": machine.run("show", key).stdout,
                   "error": machine.run("show", f"ex-{alm}zz").stderr}
        for name, output in outputs.items():
            with self.subTest(output=name):
                self.assertTrue(output.strip(), f"{name} printed nothing")
                self.assertNotIn(rlm, output)  # the neighbouring bidi control is escaped today
                self.assertNotIn(alm, output, f"{name} printed U+061C raw instead of as \\u061c: {output!r}")
                self.assertIn("\\u061c", output)


class G61_MergedCheckSeesOriginMain(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        config = self.box.root / "global.gitconfig"
        config.write_text("")
        self.genv = {"GIT_CONFIG_GLOBAL": str(config), "GIT_CONFIG_NOSYSTEM": "1"}
        self.a = support.Machine(self.box, "a")
        self.a.extra.update(self.genv)
        self.a.bead("init", "--prefix", "mg")

    def git(self, *args, cwd):
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(self.genv)
        proc = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
                              cwd=cwd, env=env, capture_output=True, text=True, timeout=30)
        self.assertEqual(proc.returncode, 0, f"git {' '.join(args)}: {proc.stderr}")
        return proc.stdout.strip()

    def close_on_a_feature_branch(self):
        """An issue closed from a worktree on branch `feat`, whose commit is not on any default branch yet."""
        key = self.a.bead("create", "one")["id"]
        tree = self.box.root / "wt"
        self.git("worktree", "add", "-q", str(tree), "-b", "feat", cwd=self.a.checkout)
        (tree / "f").write_text("x")
        self.git("add", "f", cwd=tree)
        self.git("commit", "-q", "-m", "feat", cwd=tree)
        self.a.bead("close", key, "-r", "done", cwd=tree)
        return key, tree

    def test_commit_on_origin_main_is_merged_although_local_main_is_stale(self):
        """G61: `show` says the closing commit is NOT yet on the default branch when it is on origin/main but
        the local main (usually stale in a worktree workflow) has not been updated, because the local branch is
        checked first and decides alone."""
        key, tree = self.close_on_a_feature_branch()
        self.assertIs(self.a.show(key)["merged"], False)  # control: not merged anywhere yet
        self.git("push", "-q", "origin", "feat:main", cwd=tree)  # merged on the server
        self.git("fetch", "-q", "origin", cwd=self.a.checkout)
        closing = self.a.show(key)["closed_on"]["commit"]
        self.git("merge-base", "--is-ancestor", closing, "refs/remotes/origin/main", cwd=self.a.checkout)
        self.assertNotEqual(self.git("rev-parse", "refs/heads/main", cwd=self.a.checkout),
                            self.git("rev-parse", "refs/remotes/origin/main", cwd=self.a.checkout))
        human = self.a.run("show", key)
        self.assertEqual(human.returncode, 0, human.stderr)
        self.assertIs(self.a.show(key)["merged"], True,
                      "a closing commit already on origin/main is reported as not merged: " + human.stdout)
        self.assertIn("merged into the default branch", human.stdout)

    def test_default_branch_with_a_slash_is_looked_up_whole(self):
        """G61: a default branch named release/main (origin/HEAD) is looked up as `main`, so a closing commit
        merged into origin/release/main is reported as NOT yet on the default branch."""
        key, tree = self.close_on_a_feature_branch()
        self.git("push", "-q", "origin", "feat:refs/heads/release/main", cwd=tree)
        self.git("fetch", "-q", "origin", cwd=self.a.checkout)
        self.git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/release/main", cwd=self.a.checkout)
        self.assertIs(self.a.show(key)["merged"], True,
                      "a closing commit on origin/release/main (the default branch) is reported as not merged")


def _g62_refused_checkout(test, locale):
    """A machine whose checkout git refuses for dubious ownership. git's own knob
    GIT_TEST_ASSUME_DIFFERENT_OWNER runs the real ownership check as if another user owned it."""
    box = support.Sandbox()
    test.addCleanup(box.cleanup)
    a = support.Machine(box, "a")
    scratch = box.root / "global.gitconfig"  # never the developer's own config (it may hold safe.directory)
    scratch.write_text("")
    a.extra.update(GIT_CONFIG_GLOBAL=str(scratch), GIT_CONFIG_NOSYSTEM="1", GIT_TEST_ASSUME_DIFFERENT_OWNER="1",
                   LANG=locale, LC_ALL=locale, LANGUAGE="")
    probe = subprocess.run(["git", "rev-parse", "--git-common-dir"], cwd=a.checkout,
                           env=a.env(LANG="C", LC_ALL="C", LANGUAGE=""), capture_output=True, text=True)
    if "dubious ownership" not in probe.stderr:
        test.skipTest(f"this git does not honour GIT_TEST_ASSUME_DIFFERENT_OWNER: {probe.stderr.strip()!r}")
    return a

def _g62_translates(locale):
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(LANG=locale, LC_ALL=locale, LANGUAGE="", GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
    probe = subprocess.run(["git", "--git-dir", os.devnull, "rev-parse"], cwd=os.path.sep, env=env,
                           capture_output=True, text=True)
    return bool(probe.stderr.strip()) and "fatal:" not in probe.stderr

class G62_DubiousOwnership(unittest.TestCase):
    def test_dubious_ownership_gives_gits_reason_and_fix(self):
        """G62: in a checkout git refuses for dubious ownership (container mounts, shared machines, CI runners),
        bead says 'not inside a git repository (or set BEAD_WORKSPACE)' and drops git's reason and its
        safe.directory fix."""
        a = _g62_refused_checkout(self, "C")
        for argv in (["info"], ["sync"]):
            proc = a.run(*argv, "--json")
            self.assertEqual(proc.returncode, 1, proc.stdout)
            error = json.loads(proc.stderr)["error"]
            self.assertEqual(error["code"], "NO_WORKSPACE")
            self.assertIn("safe.directory", error["message"],
                          f"{argv[0]}: git's reason and fix are dropped: {error['message']!r}")
            self.assertNotIn("not inside a git repository", error["message"])

    def test_human_error_shows_the_fix_command_readably(self):
        """G62: the human error carries git's safe.directory command as one copyable line."""
        a = _g62_refused_checkout(self, "C")
        proc = a.run("info")
        self.assertEqual(proc.returncode, 1, proc.stdout)
        self.assertIn(f"git config --global --add safe.directory {a.checkout}", proc.stderr,
                      f"the fix git gives is missing or mangled: {proc.stderr!r}")

    def test_reason_survives_a_translated_git(self):
        """G62: the same under a translated git (the safe.directory command itself is never translated)."""
        locale = next((name for name in ("de_DE.UTF-8", "fr_FR.UTF-8") if _g62_translates(name)), None)
        if not locale:
            self.skipTest("this git has no German or French messages")
        a = _g62_refused_checkout(self, locale)
        proc = a.run("info", "--json")
        self.assertEqual(proc.returncode, 1, proc.stdout)
        error = json.loads(proc.stderr)["error"]
        self.assertEqual(error["code"], "NO_WORKSPACE")
        self.assertIn("safe.directory", error["message"],
                      f"under {locale}, git's reason and fix are dropped: {error['message']!r}")


class G63_SettleKeepsLineLevelIntegrityReports(unittest.TestCase):
    def test_unreadable_line_stays_reported_when_a_duplicate_forces_a_rebuild(self):
        """G63: Journal.settle() resets `invalid` to the unidentified records whenever it rebuilds, so a
        'line is not valid JSON' report from read_segment vanishes as soon as any duplicate id, malformed
        event or misplaced copy is also present: the same damaged line is reported or not depending on
        what else the journals hold."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "a")
        a.bead("init", "--prefix", "lr")
        a.bead("create", "one")
        own = a.bead("info")["machine"]
        segment = sorted((a.store() / "events" / own).glob("*.jsonl"))[0]
        first_event = segment.read_text().splitlines()[0]
        foreign = a.store() / "events" / "fffffff0" / "000001.jsonl"
        foreign.parent.mkdir()
        foreign.write_text("not json\n{}\n")
        alone = [i["problem"] for i in a.bead("info")["integrity"]]
        self.assertIn("line is not valid JSON", alone)  # control: reported while nothing forces a rebuild
        foreign.write_text("not json\n" + first_event + "\n")  # a copy of one of a's own events
        with_copy = [i["problem"] for i in a.bead("info")["integrity"]]
        self.assertIn("line is not valid JSON", with_copy,
                      f"the damaged line is no longer reported once a copied event forces a rebuild: {with_copy}")


class G64_WritesOnABrokenOwnChain(unittest.TestCase):
    def test_create_on_a_broken_own_chain_is_a_structured_error_and_writes_nothing(self):
        """G64: once this machine's own journal has an event that does not build on its predecessor
        (a copied identity, or a write made while the journal directory could not be listed), every
        later local write is inert on replay. `create` then dies with a KeyError traceback at
        cmd_create's `tx.state.issues[uid]` (nothing is appended), and other writes (update, comments,
        close) exit 0, append an event and change nothing."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "a")
        a.bead("init", "--prefix", "cb")
        key = a.bead("create", "before the break")["id"]
        own = a.bead("info")["machine"]
        segment = sorted((a.store() / "events" / own).glob("*.jsonl"))[0]
        lines = segment.read_text().splitlines()
        bad = json.loads(lines[-1])
        bad.update(id=bad["id"][:-12] + "0123456789ab", issue=bad["id"][:-12] + "0123456789ab", deps=[])
        bad["data"]["key"] = "cb-forked"
        segment.write_text("\n".join(lines + [json.dumps(bad, separators=(",", ":"))]) + "\n")
        before = segment.read_text()
        proc = a.run("create", "after the break", "--json")
        self.assertNotIn("Traceback", proc.stderr, "create on a broken own chain crashed:\n" + proc.stderr[-400:])
        self.assertEqual(proc.returncode, 1, proc.stdout)
        self.assertEqual(json.loads(proc.stderr)["error"]["code"], "JOURNAL_FORK")
        self.assertEqual(segment.read_text(), before, "an inert event was appended")
        title = a.run("update", key, "--title", "renamed", "--json")
        self.assertNotEqual((title.returncode, a.show(key)["title"]), (0, "before the break"),
                            "update exited 0 and changed nothing")


class G65_SupersededOwnerCloseSaysSo(unittest.TestCase):
    def test_close_superseded_by_a_takeover_does_not_answer_closed(self):
        """G65: an owner's close (or delete) published right after another machine's takeover is ignored
        everywhere and listed under stale, as REVIEWING section 2 decides; but the command still answers
        '✓ Closed' / status closed with exit 0, although its own publish has just fetched the takeover
        and this machine now shows the issue in_progress under the new owner."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a, b = support.Machine(box, "a"), support.Machine(box, "b")
        a.bead("init", "--prefix", "so")
        key = a.bead("create", "x")["id"]
        a.bead("sync")
        b.bead("sync")
        a.bead("claim", key)
        b.bead("sync")
        b.bead("takeover", key, "--reason", "a looked dead")
        proc = a.run("close", key, "--reason", "done", "--json")
        shown = a.show(key)
        self.assertEqual((shown["status"], [s["op"] for s in shown.get("stale", [])]), ("in_progress", ["close"]))
        answered = json.loads(proc.stdout)[0]["status"] if proc.returncode == 0 else None
        self.assertNotEqual((proc.returncode, answered), (0, "closed"),
                            "the close answered closed with exit 0, yet it was superseded and ignored: " + proc.stdout)


# ---------------------------------------------------------------- verified properties (V1-V9)
# Passing tests for properties the review verified on e9eed92, one block per investigation area.



def _v4_random(state):
    """A float in [0, 1) from a splitmix64 stream held in state[0] (deterministic per seed)."""
    mask = 0xFFFFFFFFFFFFFFFF
    state[0] = (state[0] + 0x9E3779B97F4A7C15) & mask
    z = state[0]
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & mask
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & mask
    z ^= z >> 31
    return (z >> 11) / float(1 << 53)

def _v4_randint(state, low, high):
    return low + int(_v4_random(state) * (high - low + 1))

def _v4_choice(state, seq):
    seq = list(seq)
    return seq[int(_v4_random(state) * len(seq))]

def _v4_sample(state, seq, k):
    pool, out = list(seq), []
    for _ in range(k):
        out.append(pool.pop(int(_v4_random(state) * len(pool))))
    return out

def _v4_shuffle(state, items):
    for i in range(len(items) - 1, 0, -1):
        j = int(_v4_random(state) * (i + 1))
        items[i], items[j] = items[j], items[i]

def _v4_eid(n):
    return "01900000-%04x-7000-8000-%012x" % (n >> 48, n & (2**48 - 1))

def _v4_journal(bead, events):
    journal = bead.Journal()
    for ev in events:
        journal.add(ev, ev["replica"])
    return journal

def _v4_projection(bead, events):
    """Each issue's dependency edges (by key) and unresolved dangling keys, plus what replay could
    not apply (pending) or flagged (integrity)."""
    st = bead.replay(_v4_journal(bead, events))
    issues = {i["id"]: [sorted((st.key(t), k) for (t, k) in i["deps"]),
                        sorted(d["depends_on_id"] for d in i["dangling"])] for i in st.issues.values()}
    return json.dumps(issues, sort_keys=True), len(st.pending), len(st.integrity)

def _v4_generate(bead, state, twins):
    """One CLI-shaped history on three replicas, as model_dangling.py builds it: each replica acts on
    its own replay of what it knows. Imports carry dependencies on keys it has not seen (dangling) or
    has (imported dep_add); `dep remove` goes by key (a dangling entry) or by uid (an edge); explicit
    dep adds; random pairwise syncs. With twins, a key may be minted on two replicas (a rename)."""
    fields = {"title": "t", "status": "open", "priority": 2, "issue_type": "task"}
    pool = [f"ex-{c}" for c in "abcdefgh"]
    counter, events, used = [0], [], set()
    known = {r: set() for r in ("r0", "r1", "r2")}

    def emit(replica, op, issue=None, data=None):
        counter[0] += 1
        i = _v4_eid(counter[0])
        ev = {"v": 1, "id": i, "replica": replica, "deps": sorted(known[replica]), "op": op,
              "issue": issue or i, "data": data or {}}
        events.append(ev)
        known[replica].add(i)
        return ev

    init = emit("r0", "init", data={"prefix": "ex", "name": "ex"})
    for r in ("r1", "r2"):
        known[r].add(init["id"])
    for _ in range(_v4_randint(state, 8, 22)):
        r = _v4_choice(state, ("r0", "r1", "r2"))
        st = bead.replay(_v4_journal(bead, [e for e in events if e["id"] in known[r]]))
        roll = _v4_random(state)
        if roll < 0.15:  # sync two replicas
            other = _v4_choice(state, [x for x in known if x != r])
            union = known[r] | known[other]
            known[r], known[other] = set(union), set(union)
            continue
        fresh = [k for k in pool if k not in (st.by_key if twins else used)]
        if roll < 0.5 and fresh:  # import one record
            key = _v4_choice(state, fresh)
            used.add(key)
            targets = _v4_sample(state, [k for k in pool if k != key], _v4_randint(state, 0, 2))
            dangling = [{"issue_id": key, "depends_on_id": t, "type": "blocks"} for t in targets if t not in st.by_key]
            create = emit(r, "create", data={"key": key, "fields": fields, "imported": True,
                                             **({"dangling": dangling} if dangling else {})})
            for t in targets:
                if t in st.by_key:
                    emit(r, "dep_add", create["id"], {"to": st.by_key[t], "type": "blocks", "meta": {}, "imported": True})
            continue
        issues = list(st.issues.values())
        if not issues:
            continue
        issue = _v4_choice(state, issues)
        if roll < 0.7 and issue["dangling"]:  # dep remove by key, or a re-import that drops it
            d = _v4_choice(state, issue["dangling"])
            emit(r, "dep_remove", issue["uid"], {"key": d["depends_on_id"], "type": _v4_choice(state, (None, "blocks"))})
        elif roll < 0.85 and issue["deps"]:  # dep remove by uid
            to, _kind = _v4_choice(state, sorted(issue["deps"]))
            emit(r, "dep_remove", issue["uid"], {"to": to, "type": None})
        elif len(issues) > 1:  # explicit dep add
            other = _v4_choice(state, [i for i in issues if i is not issue])
            emit(r, "dep_add", issue["uid"], {"to": other["uid"], "type": "blocks"})
    return events

def _v4_record(key, *depends_on):
    rec = {"id": key, "title": key.upper(), "status": "open", "priority": 2, "issue_type": "task",
           "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}
    if depends_on:
        rec["dependencies"] = [{"issue_id": key, "depends_on_id": d, "type": "blocks"} for d in depends_on]
    return rec

class V4_DanglingReplayDeterminism(unittest.TestCase):
    """Replay of histories with dangling imported dependencies does not depend on the order in which
    the replicas' journals are delivered."""

    SEEDS = 1000
    ORDERS = 4

    def check(self, twins):
        bead = support.load_module()
        stats = {"events": 0, "key_removals": 0, "uid_removals": 0, "dangling_creates": 0, "twin_seeds": 0}
        for seed in range(self.SEEDS):
            state = [seed * 7919 + (104729 if twins else 0)]
            events = _v4_generate(bead, state, twins)
            stats["events"] += len(events)
            stats["key_removals"] += sum(1 for e in events if e["op"] == "dep_remove" and e["data"].get("key"))
            stats["uid_removals"] += sum(1 for e in events if e["op"] == "dep_remove" and e["data"].get("to"))
            stats["dangling_creates"] += sum(1 for e in events if e["op"] == "create" and e["data"].get("dangling"))
            keys = [e["data"]["key"] for e in events if e["op"] == "create"]
            stats["twin_seeds"] += len(keys) != len(set(keys))
            base, pending, integrity = _v4_projection(bead, events)
            self.assertEqual((pending, integrity), (0, 0), f"seed {seed}: replay left events unapplied or flagged")
            per_replica = {}
            for e in events:
                per_replica.setdefault(e["replica"], []).append(e)
            for _ in range(self.ORDERS):  # whole journals in a random order, each in its own order
                replicas = list(per_replica)
                _v4_shuffle(state, replicas)
                order = [e for r in replicas for e in per_replica[r]]
                self.assertEqual(_v4_projection(bead, order)[0], base, f"seed {seed}: delivery order {replicas}")
        # the generated histories actually exercise what the property is about
        self.assertGreater(stats["key_removals"], 400, stats)
        self.assertGreater(stats["uid_removals"], 200, stats)
        self.assertGreater(stats["dangling_creates"], 1000, stats)
        if twins:
            self.assertGreater(stats["twin_seeds"], 200, stats)
        else:
            self.assertEqual(stats["twin_seeds"], 0, stats)
        return stats

    def test_unique_key_histories_replay_identically_under_any_delivery_order(self):
        """Verified (V4): replay is deterministic for dangling histories: the same events give the same
        projection (edges by key and unresolved dangling keys) under any whole-journal delivery order.
        The review's model_dangling.py ran 1000 seeds with unique keys (9,936 events: 990 key-based and
        568 uid-based dep_removes) across three replicas with random pairwise syncs, each replayed under
        4 shuffled delivery orders: 0 nondeterministic. This test replays 1000 seeds x 4 orders of the
        same generator from its own seeded stream (not the review's exact histories; on the pristine
        bead about 10,150 events, 949 key-based and 613 uid-based dep_removes); a FIFO tie-break in
        causal_order instead of the id heap makes it fail."""
        self.check(twins=False)

    def test_twin_key_histories_replay_identically_under_any_delivery_order(self):
        """Verified (V4): the same determinism holds when a key may be minted on two replicas (twin
        keys, so one copy is renamed). The review ran 1000 seeds in this mode (10,019 events: 935
        key-based and 481 uid-based dep_removes; 551 seeds with renames), 0 nondeterministic; all 12
        two- and three-machine end-to-end scenarios also settled to equal exports. This test replays
        1000 seeds x 4 delivery orders of the same generator with twins allowed (on the pristine bead
        about 10,350 events, 929 key-based and 502 uid-based dep_removes, 564 seeds minting a key
        twice)."""
        self.check(twins=True)

class V4_DanglingOrdersThatHold(unittest.TestCase):
    """End to end on two machines: the orders in which a retraction of an imported dependency already
    holds, and add-wins against a key-based retraction."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def write(self, name, *records):
        path = self.box.root / f"{name}.jsonl"
        path.write_text("".join(json.dumps(r) + "\n" for r in records))
        return str(path)

    def pair(self):
        """m1 imports ex-a with a dependency on ex-b, which exists nowhere yet (a dangling entry);
        m2 syncs it."""
        m1 = support.Machine(self.box, "m1")
        m1.bead("init", "--prefix", "ex")
        m1.bead("import", self.write("a", _v4_record("ex-a", "ex-b")))
        m1.bead("sync")
        m2 = support.Machine(self.box, "m2")
        m2.bead("sync")
        self.assertEqual(self.edges(m2, "ex-a"), [])
        return m1, m2

    def settle(self, *machines):
        for machine in (*machines, *machines):
            machine.bead("sync")
        self.assertEqual(len({m.export() for m in machines}), 1, "machines did not converge")

    @staticmethod
    def edges(machine, key):
        return sorted((d["id"], d["dependency_type"]) for d in machine.show(key).get("dependencies", []))

    def unseen(self, machine, key):
        self.assertNotEqual(machine.run("show", key, "--json").returncode, 0, f"{machine.name} already has {key}")

    def test_key_retraction_that_sorts_first_holds(self):
        """Verified (V4): a key-based `dep remove` whose event sorts before the target's create holds:
        m1 removes ex-a's dangling dependency on ex-b before m2 imports ex-b (which depends on ex-a);
        after settling, ex-a has no edge and there is no cycle. The review ran this end to end:
        3/3 order tests pass on the current bead and on the full patch."""
        m1, m2 = self.pair()
        self.unseen(m1, "ex-b")
        m1.bead("dep", "remove", "ex-a", "ex-b")
        m2.bead("import", self.write("b", _v4_record("ex-b", "ex-a")))
        self.settle(m1, m2)
        self.assertEqual(self.edges(m1, "ex-a"), [])
        self.assertEqual(self.edges(m2, "ex-a"), [])
        self.assertEqual(m2.bead("dep", "cycles")["count"], 0)

    def test_uid_retraction_after_the_target_arrived_holds(self):
        """Verified (V4): once the target's create has arrived and the dangling entry has become an
        edge, a uid-based `dep remove` takes it away for good on every machine. The review ran this end
        to end: 3/3 order tests pass on the current bead and on the full patch."""
        m1, m2 = self.pair()
        m2.bead("import", self.write("b", _v4_record("ex-b", "ex-a")))
        m2.bead("sync")
        m1.bead("sync")
        self.assertEqual(self.edges(m1, "ex-a"), [("ex-b", "blocks")])
        m1.bead("dep", "remove", "ex-a", "ex-b")
        self.settle(m1, m2)
        self.assertEqual(self.edges(m1, "ex-a"), [])
        self.assertEqual(self.edges(m2, "ex-a"), [])

    def test_concurrent_explicit_add_survives_a_key_retraction(self):
        """Verified (V4): add-wins: m2 sees ex-b arrive, removes the imported edge and adds it back
        explicitly, while m1 (which has not seen ex-b) concurrently removes the dependency by key;
        after settling, the explicit edge ex-a -> ex-b (blocks) remains on both machines. The review
        ran this end to end: 3/3 order tests pass on the current bead and on the full patch."""
        m1, m2 = self.pair()
        m2.bead("import", self.write("b", _v4_record("ex-b")))
        m2.bead("dep", "remove", "ex-a", "ex-b")
        m2.bead("dep", "add", "ex-a", "ex-b")
        self.unseen(m1, "ex-b")
        m1.bead("dep", "remove", "ex-a", "ex-b")  # key-based, concurrent with m2's explicit add
        self.settle(m1, m2)
        self.assertEqual(self.edges(m1, "ex-a"), [("ex-b", "blocks")])
        self.assertEqual(self.edges(m2, "ex-a"), [("ex-b", "blocks")])


def _v3_common():
    """The fields every imported record in these tests carries besides id and title."""
    return {"status": "open", "priority": 2, "issue_type": "task", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "created_by": "seed"}

def _v3_git(repo, *args, data=None, env=None):
    """git with byte arguments, so that path names need not be UTF-8."""
    argv = [b"git", *[a if isinstance(a, bytes) else a.encode() for a in args]]
    return subprocess.run(argv, cwd=repo, input=data, env=env, capture_output=True, check=True).stdout

def _v3_add_foreign_folder(bare, name, ref=b"refs/beads/journal", content=b""):
    """Commit on the coordination ref of `bare` adding events/<name>/000001.jsonl, as a foreign
    writer would; `name` is raw bytes and need not be UTF-8."""
    tip = _v3_git(bare, b"rev-parse", ref).strip()
    root = [e for e in _v3_git(bare, b"ls-tree", b"-z", tip).split(b"\0") if e]
    events_sha = next(e.split(b"\t")[0].split()[2] for e in root if e.endswith(b"\tevents"))
    blob = _v3_git(bare, b"hash-object", b"-w", b"--stdin", data=content).strip()
    sub = _v3_git(bare, b"mktree", b"-z", data=b"100644 blob " + blob + b"\t000001.jsonl\0").strip()
    entries = [e for e in _v3_git(bare, b"ls-tree", b"-z", events_sha).split(b"\0") if e]
    entries.append(b"040000 tree " + sub + b"\t" + name)
    events = _v3_git(bare, b"mktree", b"-z", data=b"".join(e + b"\0" for e in entries)).strip()
    new_root = [e for e in root if not e.endswith(b"\tevents")] + [b"040000 tree " + events + b"\tevents"]
    tree = _v3_git(bare, b"mktree", b"-z", data=b"".join(e + b"\0" for e in new_root)).strip()
    commit = _v3_git(bare, b"-c", b"user.name=x", b"-c", b"user.email=x@example.invalid",
                     b"commit-tree", tree, b"-p", tip, b"-m", b"foreign folder").strip()
    _v3_git(bare, b"update-ref", ref, commit)

class V3_NonUtf8IdentityAndText(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_non_utf8_identity_and_text_are_validation_and_write_nothing(self):
        """Verified (V3): non-UTF-8 identity or text arriving through argv or the environment
        (--actor, BEAD_THREAD on claim and on update --claim, BEAD_AGENT, BEAD_ACTOR, --author) is a
        structured VALIDATION error and writes nothing; F07 holds for argv/env. The review measured
        6 cases on an initialised, synced machine, all rc 1 with
        {"error":{"code":"VALIDATION","message":"text is not valid UTF-8"}}, no traceback, export
        byte-identical afterwards and info pending_ownership == []."""
        machine = support.Machine(self.box, "v3-id")
        machine.bead("init", "--prefix", "ex")
        key = machine.bead("create", "x")["id"].encode()
        machine.bead("sync")
        before = machine.export()
        env = {k.encode(): v.encode() for k, v in machine.env().items()}
        cases = [([b"--actor", b"n\xff", b"comments", b"add", key, b"hi"], {}),
                 ([b"claim", key], {b"BEAD_THREAD": b"t\xff"}),
                 ([b"create", b"y"], {b"BEAD_AGENT": b"a\xff"}),
                 ([b"create", b"y"], {b"BEAD_ACTOR": b"a\xff"}),
                 ([b"comments", b"add", key, b"hi", b"--author", b"z\xff"], {}),
                 ([b"update", key, b"--claim"], {b"BEAD_THREAD": b"t\xff"})]
        for argv, extra in cases:
            with self.subTest(argv=argv, extra=extra):
                proc = subprocess.run([os.fsencode(support.BEAD), *argv, b"--json"], cwd=machine.checkout,
                                      env={**env, **extra}, capture_output=True, timeout=60)
                self.assertNotIn(b"Traceback", proc.stderr)
                self.assertEqual(proc.returncode, 1, proc.stderr[-300:])
                self.assertEqual(json.loads(proc.stderr)["error"],
                                 {"code": "VALIDATION", "message": "text is not valid UTF-8"})
        self.assertEqual(machine.export(), before)
        self.assertEqual(machine.bead("info")["pending_ownership"], [])

class V3_ImportShapeErrors(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_import_shape_errors_are_validation(self):
        """Verified (V3): import records whose labels or dependencies have the wrong container type,
        or whose comment members have a wrong-typed id or text, are rejected with VALIDATION (no
        traceback) and leave no integrity entries. The review measured 6 single-record files
        (labels "a,b" and [1], dependencies as a dict and as a str, comment id 1.5, comment text 5):
        all 6 VALIDATION, and info integrity == [] afterwards."""
        machine = support.Machine(self.box, "v3-shape")
        machine.bead("init", "--prefix", "ex")
        comment = {"id": 1, "author": "a", "text": "t", "created_at": None}
        cases = (("labels-str", {"labels": "a,b"}),
                 ("labels-int", {"labels": [1]}),
                 ("deps-dict", {"dependencies": {"depends_on_id": "x"}}),
                 ("deps-str", {"dependencies": "x"}),
                 ("c-id-float", {"comments": [{**comment, "id": 1.5}]}),
                 ("c-text-int", {"comments": [{**comment, "text": 5}]}))
        for name, extra in cases:
            with self.subTest(name=name):
                src = self.box.root / f"{name}.jsonl"
                src.write_text(json.dumps({"id": f"ex-{name}", "title": name, **_v3_common(), **extra}) + "\n")
                proc = machine.run("import", str(src), "--json")
                self.assertNotIn("Traceback", proc.stderr)
                self.assertNotEqual(proc.returncode, 0)
                self.assertEqual(json.loads(proc.stderr)["error"]["code"], "VALIDATION")
        self.assertEqual(machine.bead("info")["integrity"], [])
        self.assertEqual(machine.bead("list", "--all", "--limit", "0")["total"], 0)

class V3_ForeignFolderDefaultQuotePath(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_non_utf8_ref_folder_does_not_crash_sync_under_default_quotepath(self):
        """Verified (V3): a coordination-ref folder events/<name>/000001.jsonl whose name is not
        UTF-8 (caf\\xe9) or holds U+2028 does not crash sync under git's default core.quotePath
        (empty GIT_CONFIG_GLOBAL, GIT_CONFIG_NOSYSTEM=1): git ASCII-escapes the path, so it decodes,
        and the folder is skipped because the path pattern no longer matches. The review measured
        sync rc 0 for both names (the crash needs core.quotePath=false, a separate finding)."""
        machine = support.Machine(self.box, "v3-lstree")
        machine.bead("init", "--prefix", "ex")
        machine.bead("create", "x")
        machine.bead("sync")
        config = self.box.root / "gitconfig-empty"
        config.write_text("")
        events = machine.store() / "events"
        folders = sorted(os.listdir(os.fsencode(events)))
        for name in (b"caf\xe9", "line\u2028sep".encode()):
            with self.subTest(name=name):
                _v3_add_foreign_folder(self.box.origin, name)
                proc = machine.run("sync", "--json", GIT_CONFIG_GLOBAL=str(config), GIT_CONFIG_NOSYSTEM="1")
                self.assertNotIn("Traceback", proc.stderr)
                self.assertEqual(proc.returncode, 0, proc.stderr[-300:])
                self.assertEqual(sorted(os.listdir(os.fsencode(events))), folders)


def _v8_drain(proc, *masters, timeout=60):
    """Read PTY masters without blocking until the child has exited and every master is empty."""
    out = {fd: b"" for fd in masters}
    for fd in masters:
        os.set_blocking(fd, False)

    def sweep(live):
        got = False
        for fd in list(live):
            while True:
                try:
                    chunk = os.read(fd, 65536)
                except BlockingIOError:
                    break
                except OSError:  # EIO: every slave descriptor is closed
                    chunk = b""
                if not chunk:
                    live.discard(fd)
                    break
                out[fd] += chunk
                got = True
        return got

    live, deadline = set(masters), time.time() + timeout
    while live and time.time() < deadline:
        if sweep(live):
            continue
        if proc.poll() is not None:
            sweep(live)  # whatever the child wrote just before exiting
            break
        time.sleep(0.02)
    return [out[fd] for fd in masters]

def _v8_controlling_terminal(slave, default_hangup=False):
    """preexec_fn: the child becomes a session leader whose controlling terminal is `slave`."""
    def become():
        if default_hangup:
            signal.signal(signal.SIGHUP, signal.SIG_DFL)
        os.login_tty(slave)  # setsid, TIOCSCTTY, slave on 0/1/2
    return become

def _v8_needs_pty(test, controlling=False):
    if not hasattr(os, "openpty"):
        test.skipTest("not a PTY platform: os.openpty is missing")
    if controlling and not hasattr(os, "login_tty"):
        test.skipTest("os.login_tty (Python 3.11+, Unix) is needed to give bead a controlling terminal")

def _v8_rng(seed):
    """A small deterministic generator in [0, 1) (64-bit LCG), so the operation mix is reproducible."""
    state = [seed & 0xFFFFFFFFFFFFFFFF]

    def nxt():
        state[0] = (state[0] * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        return (state[0] >> 11) / float(1 << 53)
    return nxt

def _v8_pick(rng, items):
    return items[min(int(rng() * len(items)), len(items) - 1)]

def _v8_converge_and_compare(test, machines, remote):
    """Two full sync rounds, then: identical exports and claims, a clean `info`, and a clean remote fsck."""
    for _ in range(2):
        for m in machines:
            m.bead("sync")
    exports = {m.name: m.export() for m in machines}
    claims = {m.name: sorted((c["id"], c["owner"], c["state"]) for c in m.bead("claims")["claims"])
              for m in machines}
    test.assertEqual(len(set(exports.values())), 1, "exports differ between machines")
    first = claims[machines[0].name]
    for name, seen in claims.items():
        test.assertEqual(seen, first, f"claims on {name} differ from {machines[0].name}")
    for m in machines:
        info = m.bead("info")
        test.assertEqual(info["integrity"], [], m.name)
        test.assertEqual(info["waiting_events"], 0, m.name)
        test.assertEqual(info["pending_ownership"], [], m.name)
    fsck = subprocess.run(["git", "--git-dir", str(remote), "fsck", "--full", "--strict"],
                          capture_output=True, text=True)
    test.assertEqual(fsck.returncode, 0, fsck.stderr)
    return claims

def _v8_mixed_ops(machines, keys, steps, seed, after_remote_op=None):
    """Random claims, releases, edits and syncs; returns the owner model built from claims accepted online and
    every failure whose code is not an expected refusal."""
    rng, owner, unexpected = _v8_rng(seed), {}, []
    for step in range(steps):
        m, key, r = _v8_pick(rng, machines), _v8_pick(rng, keys), rng()
        if r < 0.3:
            op, args = "sync", ["sync"]
        elif r < 0.55:
            op, args = "claim", ["claim", key]
        elif r < 0.75:
            op, args = "release", ["release", key]
        else:
            op, args = "update", ["update", key, "--notes", f"step {step}"]
        proc = m.run(*args, "--json")
        if proc.returncode == 0:
            if op == "claim":
                owner[key] = m.name
            elif op == "release":
                owner.pop(key, None)
        else:
            try:
                code = json.loads(proc.stderr)["error"]["code"]
            except (ValueError, KeyError, TypeError):
                code = proc.stderr[-300:]
            if code not in ("ALREADY_CLAIMED", "NOT_OWNER", "ISSUE_NOT_FOUND"):
                unexpected.append((step, m.name, args, code))
        if op != "update" and after_remote_op:
            after_remote_op()
    return owner, unexpected

def _v8_wait_for_remote_gc(remote, limit=20):
    """A detached `gc --auto` may still be running on the remote; let it finish before the sandbox goes."""
    deadline = time.time() + limit
    while (remote / "gc.pid").exists() and time.time() < deadline:
        time.sleep(0.1)

def _v8_python312():
    found = shutil.which("python3.12")
    if found:
        return found
    uv = Path.home() / ".local" / "share" / "uv" / "python"
    if not uv.is_dir():
        return None
    for candidate in sorted(uv.glob("cpython-3.12*/bin/python3.12")):
        if os.access(candidate, os.X_OK):
            return str(candidate)
    return None

class V8_PtyOutputMatchesPipeOutput(unittest.TestCase):
    def test_pty_output_matches_pipe_output_and_escapes_hostile_text(self):
        """Verified (V8): under a PTY (stdout and stderr on two separate PTYs) human output equals the pipe output
        modulo CRLF, no raw C0/C1/bidi/LS/PS character reaches the terminal, errors go only to stderr and exit
        codes match. The review ran 25 commands under 3 locales (75 runs) over hostile titles, descriptions,
        labels, comments and a claim thread: exit codes equal 75/75, stderr equal 75/75, stdout equal 69/75 (the 6
        differences are `help create` / `create --help`, finding ENV-2, not exercised here); only U+061C (ENV-3,
        not in this fixture) reached the PTY raw; 0 tracebacks; all 15 error runs had empty stdout; the
        5000-char and wide/combining titles were byte-identical."""
        _v8_needs_pty(self)
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "a")
        a.bead("init", "--prefix", "ex")
        hostile = ["esc\x1b[31mRED\x1b]0;t\x07", "c1\u009b31m\u0085", "bidi\u202eevil\u2066x\u2069", "tab\tX",
                   "ls\u2028X\u2029", "cr\rX\x08", "nl\nFORGED",
                   "宽 é́ \U0001f469\u200d\U0001f469\u200d\U0001f467", "L" * 3000]
        keys = []
        for title in hostile:
            keys.append(a.bead("create", title, "-d", title, "-l", "l" + str(len(keys)))["id"])
            a.bead("comments", "add", keys[-1], title)
        a.bead("sync")
        a.bead("claim", keys[2], thread="thr\u202eevil\x1b[2J")
        raw = re.compile("[\x00-\x09\x0b-\x1f\x7f-\x9f\u2028\u2029\u200e\u200f\u202a-\u202e\u2066-\u2069]")
        commands = [["show", *keys], ["list", "--all"], ["ready"], ["claims"], ["log"], ["info"], ["sync"],
                    ["comments", keys[0]], ["show", "ex-nope\x1b[31m"], ["bogus\x1b]0;x\x07"]]
        for args in commands:
            with self.subTest(args=args[:2]):
                env = a.env()
                pipe = subprocess.run([support.BEAD, *args], cwd=a.checkout, env=env, capture_output=True,
                                      stdin=subprocess.DEVNULL, timeout=60)
                mo, so = os.openpty()
                me, se = os.openpty()
                try:
                    proc = subprocess.Popen([support.BEAD, *args], cwd=a.checkout, env=env,
                                            stdin=so, stdout=so, stderr=se)
                finally:
                    os.close(so)
                    os.close(se)
                try:
                    out, err = _v8_drain(proc, mo, me)
                    proc.wait(timeout=60)
                finally:
                    os.close(mo)
                    os.close(me)
                self.assertEqual(proc.returncode, pipe.returncode)
                self.assertEqual(out.replace(b"\r\n", b"\n"), pipe.stdout)
                self.assertEqual(err.replace(b"\r\n", b"\n"), pipe.stderr)
                if pipe.returncode:
                    self.assertEqual(out, b"", "an error wrote to stdout")
                    self.assertNotEqual(err, b"")
                text = out.decode("utf-8").replace("\r\n", "\n") + err.decode("utf-8").replace("\r\n", "\n")
                self.assertEqual(raw.findall(text), [])
                self.assertNotIn("\nFORGED", text)
                self.assertNotIn("Traceback", text)

class V8_SyncUnderAControllingTerminalCannotPrompt(unittest.TestCase):
    def test_sync_under_a_controlling_terminal_cannot_prompt(self):
        """Verified (V8): sync under a controlling terminal cannot prompt, because git and ssh run in their own
        session (run_git uses start_new_session=True), so a transport that wants /dev/tty fails fast instead of
        hanging. The review measured rc 1 after 0.44 s with bead as session leader of a PTY and a fake ssh that
        opens /dev/tty for a passphrase: the terminal showed '/dev/tty: Device not configured ... fatal: Could not
        read from remote repository.' and no prompt."""
        _v8_needs_pty(self, controlling=True)
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "a")
        a.bead("init", "--prefix", "ex")
        bindir = box.root / "fake-bin"
        bindir.mkdir()
        fake = bindir / "ssh"  # also first on PATH, so no real ssh can run whatever bead does to GIT_SSH_COMMAND
        marker = box.root / "fake-ssh-ran"
        fake.write_text("#!/bin/sh\n"
                        f"echo ran >> '{marker}'\n"
                        "exec 3<>/dev/tty || { echo 'fake-ssh: no terminal' >&2; exit 255; }\n"
                        "printf 'Passphrase: ' >&3\nread x <&3\nexit 255\n")
        fake.chmod(0o755)
        env = a.env(GIT_SSH_COMMAND=str(fake), BEAD_NET_TIMEOUT="30",
                    PATH=f"{bindir}{os.pathsep}{os.environ.get('PATH', '')}")
        mo, so = os.openpty()
        start = time.time()
        try:
            proc = subprocess.Popen([support.BEAD, "sync", "--remote", "ssh://nowhere.invalid/x.git"],
                                    cwd=a.checkout, env=env, stdin=so, stdout=so, stderr=so,
                                    preexec_fn=_v8_controlling_terminal(so))
        finally:
            os.close(so)
        try:
            out, = _v8_drain(proc, mo, timeout=40)
            try:
                proc.wait(timeout=40)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
                self.fail("bead sync hung on a transport that wants the terminal")
        finally:
            os.close(mo)
        elapsed = time.time() - start
        self.assertTrue(marker.exists(), "the fake ssh never ran")
        self.assertEqual(proc.returncode, 1, out)
        self.assertLess(elapsed, 15)
        self.assertNotIn(b"Passphrase:", out)
        self.assertIn(b"cannot reach the coordination remote", out)

class V8_HangupOfTheControllingTerminal(unittest.TestCase):
    def test_hangup_of_the_controlling_terminal_ends_bead_quietly(self):
        """Verified (V8): closing the controlling terminal mid-output ends bead quietly by SIGHUP, with no
        traceback. The review ran `list --all --limit 0` over 40-60 issues with 1500-char titles and closed the
        master after reading 1 KB: rc -1 (-SIGHUP), nothing written. (With SIGHUP ignored, or without a
        controlling terminal, the result is ENV-1's EIO path instead, so SIGHUP is reset to its default here.)"""
        _v8_needs_pty(self, controlling=True)
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "a")
        a.bead("init", "--prefix", "ex")
        for i in range(40):
            a.bead("create", f"bulk {i} " + "y" * 1500)
        master, slave = os.openpty()
        try:
            proc = subprocess.Popen([support.BEAD, "list", "--all", "--limit", "0"], cwd=a.checkout, env=a.env(),
                                    stdin=slave, stdout=slave, stderr=slave,
                                    preexec_fn=_v8_controlling_terminal(slave, default_hangup=True))
        finally:
            os.close(slave)
        os.set_blocking(master, False)
        got, deadline = b"", time.time() + 30
        try:
            while len(got) < 1024 and time.time() < deadline and proc.poll() is None:
                try:
                    got += os.read(master, 512)
                except BlockingIOError:
                    time.sleep(0.01)
        finally:
            os.close(master)  # the terminal window closes
        self.assertGreaterEqual(len(got), 1024, got[-300:])
        self.assertEqual(proc.wait(timeout=60), -signal.SIGHUP)
        self.assertNotIn(b"Traceback", got)

class V8_RemoteMaintenance(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.machines = [support.Machine(self.box, n) for n in ("a", "b", "c")]
        a = self.machines[0]
        a.bead("init", "--prefix", "gc")
        a.bead("sync")
        self.keys = [a.bead("create", f"t{i}")["id"] for i in range(5)]
        for m in self.machines:
            m.bead("sync")

    def check(self, owner, unexpected):
        self.assertEqual(unexpected, [])
        claims = _v8_converge_and_compare(self, self.machines, self.box.origin)
        seen = sorted((c[0], c[1].split(":", 1)[1].split("-thread")[0]) for c in claims["a"] if c[2] == "confirmed")
        self.assertEqual(seen, sorted(owner.items()))

    def test_gc_prune_now_aggressive_after_every_remote_operation(self):
        """Verified (V8): remote maintenance run after every remote operation (not concurrently) leaves convergence
        and ownership intact. Here `git gc --prune=now --aggressive` runs on the bare remote after every
        sync/claim/release of 60 mixed operations on 3 machines. The review's soak (300 ops, 3 machines, modes gc
        --prune=now --aggressive, gc --auto with gc.auto=1, repack -adf + prune-packed) measured, in each mode:
        exports identical, claims identical, owners equal to the model (16 accepted claims), integrity [],
        waiting 0, pending [], fsck 0, 0 unexpected errors over 200/200/400 maintenance runs; sync mean/p95
        453/645 ms without gc vs 498/789 ms with the aggressive gc."""
        def gc():
            subprocess.run(["git", "--git-dir", str(self.box.origin), "gc", "--prune=now", "--aggressive", "--quiet"],
                           check=True, capture_output=True)
        self.check(*_v8_mixed_ops(self.machines, self.keys, 60, 7, gc))

    def test_repack_and_auto_gc_running_concurrently_with_pushes_and_fetches(self):
        """Verified (V8): `git repack -adf` + `prune-packed` and `gc --auto` with gc.auto=1 looping on the remote
        CONCURRENTLY with the machines' pushes and fetches (60 mixed operations here) cause no maintenance error,
        no unexpected bead error and no divergence. The review measured, over 300 ops: repack 4740 maintenance
        runs and auto1 8529 runs, 0 errors, owner model 17/17, fsck 0; gc_race with 3 machines publishing in
        parallel saw 1154 repack runs during 150 commands, 0 errors, identical exports and claims. (Concurrent
        `gc --prune=now` corrupted the remote in 4 of 4 runs, finding ENV-4, and is not exercised here.)"""
        support.git("--git-dir", str(self.box.origin), "config", "gc.auto", "1")
        self.addCleanup(_v8_wait_for_remote_gc, self.box.origin)
        stop, failures = threading.Event(), []

        def loop():
            while not stop.is_set():
                for cmd in (["repack", "-adf", "-q"], ["prune-packed"], ["gc", "--auto", "--quiet"]):
                    proc = subprocess.run(["git", "--git-dir", str(self.box.origin), *cmd],
                                          capture_output=True, text=True)
                    if proc.returncode:
                        failures.append((cmd, proc.stderr))
        worker = threading.Thread(target=loop, daemon=True)
        worker.start()
        try:
            result = _v8_mixed_ops(self.machines, self.keys, 60, 8)
        finally:
            stop.set()
            worker.join(timeout=60)
        self.assertEqual(failures, [])
        _v8_wait_for_remote_gc(self.box.origin)
        self.check(*result)

class V8_PruneOnThePrivateSyncRepo(unittest.TestCase):
    def test_prune_now_on_the_private_sync_repo_during_syncs_is_transient(self):
        """Verified (V8): `git gc --prune=now` looping on a machine's own private sync.git during its syncs causes
        only transient errors; nothing is left pending and the next syncs converge. The review's soak (776 prune
        runs during 200 ops) saw 89 GIT_ERROR ('git mktree: fatal: entry ... is unavailable') and 2
        REMOTE_UNREACHABLE, all transient; final exports and claims identical, pending [], integrity [], remote
        fsck 0. Here: 30 create+sync rounds on a while the prune loops, then b sees all 30 issues."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a, b = support.Machine(box, "a"), support.Machine(box, "b")
        a.bead("init", "--prefix", "ex")
        a.bead("sync")
        b.bead("sync")
        sync_git = a.store() / "sync.git"
        self.assertTrue(sync_git.is_dir(), sync_git)
        stop, codes = threading.Event(), []

        def loop():
            while not stop.is_set():
                subprocess.run(["git", "--git-dir", str(sync_git), "gc", "--prune=now", "--quiet"],
                               capture_output=True)
        worker = threading.Thread(target=loop, daemon=True)
        worker.start()
        try:
            for i in range(30):
                a.bead("create", f"t{i}")
                proc = a.run("sync", "--json")
                if proc.returncode:
                    codes.append(json.loads(proc.stderr)["error"]["code"])
        finally:
            stop.set()
            worker.join(timeout=60)
        self.assertTrue(set(codes) <= {"GIT_ERROR", "REMOTE_UNREACHABLE"}, codes)
        _v8_converge_and_compare(self, [a, b], box.origin)
        self.assertEqual(len(b.bead("list", "--all", "--limit", "0")["issues"]), 30)

class V8_Python312(unittest.TestCase):
    def test_storage_module_passes_with_bead_itself_running_on_312(self):
        """Verified (V8): on CPython 3.12 the suite passes with both the runner and every bead subprocess on 3.12
        (a scratch copy of bead whose shebang is 3.12, reached through BEAD_UNDER_TEST). The review ran the six
        modules on 3.12.10: test_replay 60 OK in 50.5 s, test_storage 9 OK in 11.8 s, test_compat 3 OK in 46.2 s,
        test_commands 97 OK in 297.6 s, test_protocol 54 OK in 163.1 s, test_stress 2 OK in 911.8 s; 225 tests
        OK, no warnings or skips. This test reruns test_storage that way."""
        python = _v8_python312()
        if python is None:
            self.skipTest("CPython 3.12 is not installed (no python3.12 on PATH or under uv's python directory)")
        tests_dir = Path(support.__file__).resolve().parent
        if not (tests_dir / "test_storage.py").exists():
            self.skipTest("tests/test_storage.py is not beside tests/support.py")
        with tempfile.TemporaryDirectory(prefix="bead-r7-312-") as tmp:
            copy = Path(tmp) / "bead"  # the launcher's shape with a 3.12 shebang and the checkout's package on the path
            copy.write_text(f"#!{python}\nimport sys\nsys.path.insert(0, {str(support.ROOT)!r})\n"
                            "from beadroll import *  # noqa\nfrom beadroll.cli import main\n"
                            "if __name__ == '__main__':\n    sys.exit(main())\n", encoding="utf-8")
            copy.chmod(0o755)
            env = {**os.environ, "BEAD_UNDER_TEST": str(copy), "BEAD_HOME": str(Path(tmp) / "home"),
                   "PYTHONDONTWRITEBYTECODE": "1"}
            proc = subprocess.run([python, "-m", "unittest", "tests.test_storage"], cwd=tests_dir.parent,
                                  env=env, capture_output=True, text=True, timeout=600)
            self.assertEqual(proc.returncode, 0, proc.stderr[-2000:])
            self.assertRegex(proc.stderr, r"Ran [1-9][0-9]* tests?")
            self.assertIn("OK", proc.stderr)
            self.assertNotIn("skipped", proc.stderr)

class V8_RecoveryFromAPrunedRemote(unittest.TestCase):
    def test_resetting_the_ref_to_the_last_valid_commit_converges_without_loss(self):
        """Verified (V8): recovery from a remote whose tip commit was lost (what a concurrent `gc --prune=now`
        left, ENV-4): `git update-ref refs/beads/journal <last valid commit>` on the bare remote, then syncs, lets
        all machines converge with no local data lost; the acknowledged claim in the lost commit is republished
        from the claimant's local copy. The review measured, on its corrupted soak sandbox: exports identical,
        the same 8 confirmed claims on a, b and c, integrity [] and waiting 0 everywhere, remote fsck clean; nobody
        could claim during the outage because every fetch failed."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a, b, c = (support.Machine(box, n) for n in ("a", "b", "c"))
        a.bead("init", "--prefix", "ex")
        a.bead("sync")
        key, other = a.bead("create", "work")["id"], a.bead("create", "other")["id"]
        for m in (a, b, c):
            m.bead("sync")
        valid = support.git("--git-dir", str(box.origin), "rev-parse", "refs/beads/journal").strip()
        c.bead("claim", key)  # acknowledged: published in a new tip commit
        tip = support.git("--git-dir", str(box.origin), "rev-parse", "refs/beads/journal").strip()
        self.assertNotEqual(tip, valid)
        self.assertEqual(support.git("--git-dir", str(box.origin), "rev-parse", f"{tip}^").strip(), valid)
        loose = box.origin / "objects" / tip[:2] / tip[2:]
        self.assertTrue(loose.exists())  # small pushes arrive as loose objects
        loose.unlink()  # the lost commit
        outage = b.run("claim", other, "--json")
        self.assertEqual(outage.returncode, 1, outage.stdout)  # nobody can claim while the remote is damaged
        support.git("--git-dir", str(box.origin), "update-ref", "refs/beads/journal", valid)  # the operator's repair
        for m in (c, a, b):
            m.bead("sync")
        claims = _v8_converge_and_compare(self, [a, b, c], box.origin)
        confirmed = [(cid, owner) for cid, owner, state in claims["a"] if state == "confirmed"]
        self.assertEqual(len(confirmed), 1, claims)
        self.assertEqual(confirmed[0][0], key)
        self.assertIn("c-thread", confirmed[0][1])
        titles = sorted(i["title"] for i in b.bead("list", "--all", "--limit", "0")["issues"])
        self.assertEqual(titles, ["other", "work"])


def _v9_scratch_git_config(box, text=""):
    """A scratch global git config for this sandbox (never the user's), with system config off."""
    path = box.root / "global.gitconfig"
    path.write_text(text)
    return {"GIT_CONFIG_GLOBAL": str(path), "GIT_CONFIG_NOSYSTEM": "1"}

def _v9_git_env(genv, **more):
    """The test process's environment without GIT_* variables, plus the scratch git config."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(genv)
    env.update(more)
    return env

def _v9_sh(*args, cwd=None, env=None):
    """Run git with a fixed identity; fail the test with git's stderr if it fails."""
    proc = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
                          cwd=cwd, env=env, capture_output=True, text=True)
    if proc.returncode:
        raise AssertionError(f"git {' '.join(args)}: {proc.stderr}")
    return proc.stdout.strip()

def _v9_has_ref(git_dir, ref):
    """Whether the repository at git_dir has `ref`."""
    return subprocess.run(["git", "--git-dir", str(git_dir), "rev-parse", "-q", "--verify", ref],
                          capture_output=True).returncode == 0

def _v9_round(test, text, **extra):
    """A two-machine round under one scratch global git config: init, create, sync, claim on b,
    a competing claim on a (ALREADY_CLAIMED), close on b, sync both; then both machines agree."""
    box = support.Sandbox()
    test.addCleanup(box.cleanup)
    genv = _v9_scratch_git_config(box, text)
    a, b = support.Machine(box, "a"), support.Machine(box, "b")
    for m in (a, b):
        m.extra.update(genv, **extra)
    a.bead("init")
    issue = a.bead("create", "one")["id"]
    a.bead("sync")
    b.bead("sync")
    b.bead("claim", issue)
    test.assertEqual(support.code(a.run("claim", issue, "--json")), "ALREADY_CLAIMED")
    b.bead("close", issue, "-r", "done")
    a.bead("sync")
    b.bead("sync")
    test.assertEqual(a.export(), b.export())
    test.assertEqual(a.bead("show", issue), b.bead("show", issue))
    test.assertEqual(a.bead("show", issue)[0]["status"], "closed")

class V9_HarmlessGlobalGitSettings(unittest.TestCase):
    SIGNING_AND_PUSH = [
        # commit-tree does not honour commit.gpgSign, so a bad signing key is never used
        "[commit]\n\tgpgSign = true\n[gpg]\n\tformat = ssh\n[user]\n\tsigningKey = /nonexistent\n",
        "[push]\n\tgpgSign = if-asked\n",
        "[push]\n\tdefault = nothing\n",
        "[transfer]\n\tfsckObjects = true\n[receive]\n\tfsckObjects = true\n",
        "[fetch]\n\tprune = true\n\tpruneTags = true\n",
    ]
    PROTOCOL_AND_REPOSITORY = [
        "[protocol]\n\tversion = 0\n",
        "[protocol]\n\tversion = 1\n",
        "[safe]\n\tbareRepository = explicit\n",
        "[core]\n\tfsmonitor = true\n",
    ]
    FORMAT_AND_HOUSEKEEPING = [
        "[init]\n\tdefaultRefFormat = reftable\n",
        "[gc]\n\tauto = 1\n\tautoDetach = false\n",
        "[core]\n\tautocrlf = true\n\tsafecrlf = true\n",
        "[color]\n\tui = always\n",
    ]

    def _rounds(self, settings):
        for text in settings:
            with self.subTest(text=text):
                _v9_round(self, text)

    def test_signing_push_and_fetch_settings_are_harmless(self):
        """Verified (V9): global git settings reach sync.git without harm. In a two-machine round
        (init, create, sync, claim, a competing claim gets ALREADY_CLAIMED, close, sync both) the
        exports and shows are identical and the status is closed. This part covers commit.gpgSign
        with gpg.format=ssh and a nonexistent key (harmless because `git commit-tree` does not
        honour commit.gpgSign: a direct check printed a commit id with rc=0), push.gpgSign=if-asked,
        push.default=nothing, transfer/receive.fsckObjects and fetch.prune+pruneTags. The review
        measured 13/13 settings harmless in this round, and 22/24 in its wider experiment (the
        failures were push.gpgSign=true against a remote without signed-push support, as the
        user's own push fails, and the no-setting control)."""
        self._rounds(self.SIGNING_AND_PUSH)

    def test_protocol_and_repository_settings_are_harmless(self):
        """Verified (V9): global git settings reach sync.git without harm, as above (two-machine
        round with ALREADY_CLAIMED on the competing claim, identical exports and shows, status
        closed). This part covers protocol.version=0, protocol.version=1,
        safe.bareRepository=explicit and core.fsmonitor=true. The review measured 13/13 settings
        harmless in this round and 22/24 in its wider experiment."""
        self._rounds(self.PROTOCOL_AND_REPOSITORY)

    def test_format_and_housekeeping_settings_are_harmless(self):
        """Verified (V9): global git settings reach sync.git without harm, as above (two-machine
        round with ALREADY_CLAIMED on the competing claim, identical exports and shows, status
        closed). This part covers init.defaultRefFormat=reftable (the planned Git 3.0 default),
        gc.auto=1 with gc.autoDetach=false, core.autocrlf+safecrlf and color.ui=always. The review
        measured 13/13 settings harmless in this round and 22/24 in its wider experiment."""
        self._rounds(self.FORMAT_AND_HOUSEKEEPING)

class V9_CheckoutHooksDoNotRunOnBeadsPush(unittest.TestCase):
    def test_checkout_and_husky_pre_push_hooks_never_run(self):
        """Verified (V9): per-repository hooks do not leak into bead's push. A refusing
        .git/hooks/pre-push in the checkout and a refusing husky hook (per-repository
        core.hooksPath=.husky) never run, and sync and claim succeed. The review measured 0 hook
        runs out of the 2 refusing per-repository hooks, on both sync and claim."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        marker = box.root / "ran"
        a = support.Machine(box, "a")
        a.extra.update(_v9_scratch_git_config(box))
        for hook in (a.checkout / ".git" / "hooks" / "pre-push", a.checkout / ".husky" / "pre-push"):
            hook.parent.mkdir(exist_ok=True)
            hook.write_text(f"#!/bin/sh\ntouch '{marker}'\nexit 1\n")
            hook.chmod(0o755)
        support.git("config", "core.hooksPath", ".husky", cwd=a.checkout)
        a.bead("init")
        issue = a.bead("create", "one")["id"]
        a.bead("sync")
        self.assertFalse(marker.exists(), "a checkout pre-push hook ran on bead sync")
        a.bead("claim", issue)
        self.assertFalse(marker.exists(), "a checkout pre-push hook ran on bead claim")
        self.assertEqual(a.show(issue)["status"], "in_progress")

class V9_CallerGitEnvironment(unittest.TestCase):
    def test_hook_and_alias_git_environment_keeps_workspace_and_claims(self):
        """Verified (V9): a caller's git environment as hooks, aliases and `git -c` set it does not
        change the workspace or break claims once sync.git exists. Cases: GIT_DIR alone (as a hook
        in a linked worktree gets it, cwd at the worktree top), GIT_DIR+GIT_WORK_TREE,
        GIT_INDEX_FILE+GIT_PREFIX (pre-commit), GIT_CONFIG_PARAMETERS, fixed
        GIT_COMMITTER_DATE/GIT_AUTHOR_DATE, and GIT_TRACE=1. The review measured 6/6 cases with the
        same workspace and claim and release rc 0 (git 2.55)."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        a = support.Machine(box, "a")
        a.extra.update(_v9_scratch_git_config(box))
        a.bead("init")
        issue = a.bead("create", "one")["id"]
        a.bead("sync")
        workspace = a.bead("info")["workspace"]
        (a.checkout / "src").mkdir()
        cases = [dict(GIT_DIR=str(a.checkout / ".git")),
                 dict(GIT_DIR=str(a.checkout / ".git"), GIT_WORK_TREE=str(a.checkout)),
                 dict(GIT_INDEX_FILE=".git/index", GIT_PREFIX="src/"),
                 dict(GIT_CONFIG_PARAMETERS="'user.name'='t'"),
                 dict(GIT_COMMITTER_DATE="2001-01-01T00:00:00Z", GIT_AUTHOR_DATE="2001-01-01T00:00:00Z"),
                 dict(GIT_TRACE="1")]
        for extra in cases:
            with self.subTest(extra=extra):
                info = a.run("info", "--json", **extra)
                self.assertEqual(info.returncode, 0, info.stderr)
                self.assertEqual(json.loads(info.stdout)["workspace"], workspace)
                claim = a.run("claim", issue, "--json", **extra)
                self.assertEqual(claim.returncode, 0, claim.stderr)
                release = a.run("release", issue, "--json", **extra)
                self.assertEqual(release.returncode, 0, release.stderr)

class V9_EveryCheckoutShapeIsOneWorkspace(unittest.TestCase):
    def test_checkout_shapes_of_one_repository_share_the_workspace(self):
        """Verified (V9): with an untracked, URL-derived workspace, every checkout shape of one
        repository resolves to the same workspace and reads the same issue: a subdirectory, a
        linked worktree, a detached worktree, a bare clone as cwd, a shallow clone (depth 1), an
        unborn HEAD with the same origin, and a submodule checkout of the repository. The review
        measured 7/7 places agreeing (with a tracked .beadroll.json, 7 of 11 places agree; the
        other 4 are its finding G06 and are not tested here)."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        genv = _v9_scratch_git_config(box)
        env = _v9_git_env(genv)
        a = support.Machine(box, "a")
        a.extra.update(genv)
        a.bead("init")
        issue = a.bead("create", "one")["id"]
        a.bead("sync")
        workspace = a.bead("info")["workspace"]
        (a.checkout / "src").mkdir()
        _v9_sh("worktree", "add", "-q", str(box.root / "wt"), "-b", "feature", cwd=a.checkout, env=env)
        _v9_sh("worktree", "add", "-q", "--detach", str(box.root / "detached"), cwd=a.checkout, env=env)
        _v9_sh("clone", "-q", "--bare", str(box.origin), str(box.root / "bare.git"), env=env)
        _v9_sh("clone", "-q", "--depth", "1", f"file://{box.origin}", str(box.root / "shallow"), env=env)
        _v9_sh("remote", "set-url", "origin", str(box.origin), cwd=box.root / "shallow", env=env)
        _v9_sh("init", "-q", str(box.root / "unborn"), env=env)
        _v9_sh("remote", "add", "origin", str(box.origin), cwd=box.root / "unborn", env=env)
        _v9_sh("init", "-q", str(box.root / "super"), env=env)
        _v9_sh("-c", "protocol.file.allow=always", "submodule", "add", "-q", str(box.origin), "lib",
               cwd=box.root / "super", env=env)
        places = (a.checkout / "src", box.root / "wt", box.root / "detached", box.root / "bare.git",
                  box.root / "shallow", box.root / "unborn", box.root / "super" / "lib")
        for where in places:
            with self.subTest(where=where.name):
                self.assertEqual(a.bead("info", cwd=where)["workspace"], workspace)
                self.assertEqual(a.bead("show", issue, cwd=where)[0]["title"], "one")

class V9_TransportAndUrlSettings(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_git_ssh_command_in_the_environment_is_honoured(self):
        """Verified (V9): bead's sync honours a GIT_SSH_COMMAND already in the environment (bead
        only sets its own default when none is present). No real ssh runs: the origin is
        ssh://fakehost/<sandbox origin>, a fake `ssh` first on PATH and a global core.sshCommand
        both refuse and log, and the user's wrapper runs the remote git command locally. The review
        measured the user's git fetch rc 0 and bead sync rc 0, with the wrapper invoked for both
        fetch (git-upload-pack) and push (git-receive-pack)."""
        bin_ = self.box.root / "bin"
        bin_.mkdir()
        log = self.box.root / "transport.log"
        log.write_text("")
        (bin_ / "ssh").write_text(f"#!/bin/sh\necho \"plain-ssh $*\" >> '{log}'\n"
                                  "echo 'plain ssh: blocked in this test' >&2\nexit 255\n")
        (bin_ / "refusing-ssh").write_text(f"#!/bin/sh\necho \"core-sshcommand $*\" >> '{log}'\n"
                                           "echo 'core.sshCommand: blocked in this test' >&2\nexit 255\n")
        (bin_ / "user-ssh-wrapper").write_text(f"#!/bin/sh\necho \"user-ssh-wrapper $*\" >> '{log}'\n"
                                               "for last; do :; done\nexec sh -c \"$last\"\n")
        for f in bin_.iterdir():
            f.chmod(0o755)
        genv = _v9_scratch_git_config(self.box, f"[core]\n\tsshCommand = {bin_ / 'refusing-ssh'}\n")
        a = support.Machine(self.box, "a")
        _v9_sh("remote", "set-url", "origin", f"ssh://fakehost{self.box.origin}", cwd=a.checkout, env=_v9_git_env(genv))
        a.extra.update(genv, PATH=f"{bin_}{os.pathsep}{os.environ['PATH']}",
                       GIT_SSH_COMMAND=str(bin_ / "user-ssh-wrapper"))
        user_fetch = subprocess.run(["git", "fetch", "-q", "origin"], cwd=a.checkout, env=a.env(),
                                    capture_output=True, text=True)
        self.assertEqual(user_fetch.returncode, 0, user_fetch.stderr)
        log.write_text("")
        a.bead("init")
        a.bead("create", "one")
        sync = a.run("sync", "--json")
        self.assertEqual(sync.returncode, 0, sync.stderr)
        lines = log.read_text().splitlines()
        self.assertFalse([line for line in lines if not line.startswith("user-ssh-wrapper ")], lines)
        self.assertTrue(any("git-upload-pack" in line for line in lines), lines)
        self.assertTrue(any("git-receive-pack" in line for line in lines), lines)
        self.assertTrue(_v9_has_ref(self.box.origin, "refs/beads/journal"))

    def test_global_push_instead_of_is_honoured(self):
        """Verified (V9): bead's sync honours a global url.<primary>.pushInsteadOf: origin is a
        read-only mirror whose pre-receive refuses, and the global config rewrites pushes to it
        towards the primary. The review measured bead sync rc 0 in 0.4 s; the journal lands on
        the primary, not the mirror."""
        ro = self.box.root / "ro.git"
        subprocess.run(["git", "clone", "-q", "--mirror", str(self.box.origin), str(ro)], check=True,
                       capture_output=True)
        hook = ro / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\necho 'read-only mirror' >&2\nexit 1\n")
        hook.chmod(0o755)
        a = support.Machine(self.box, "a")
        a.extra.update(_v9_scratch_git_config(self.box, f'[url "{self.box.origin}"]\n\tpushInsteadOf = {ro}\n'))
        support.git("remote", "set-url", "origin", str(ro), cwd=a.checkout)
        a.bead("init", "--offline")  # one publish: the mirror fetched from never learns of it, a second could not fast-forward
        a.bead("create", "x", "--offline")
        started = time.monotonic()
        sync = a.run("sync", "--json")
        elapsed = time.monotonic() - started
        self.assertEqual(sync.returncode, 0, sync.stderr)
        self.assertLess(elapsed, 30)
        self.assertTrue(_v9_has_ref(self.box.origin, "refs/beads/journal"))
        self.assertFalse(_v9_has_ref(ro, "refs/beads/journal"))

    def test_relative_origin_resolves_like_git(self):
        """Verified (V9): a relative origin path (../origin.git) resolves to the same workspace from
        the top of the checkout and from a subdirectory, even when the subdirectory holds an
        unrelated directory named origin.git. A linked worktree at another directory depth breaks
        the relative URL for git itself (git fetch rc 128), and bead does not resolve it to the
        checkout's workspace there either. The review measured top and src/ both 8b1be9c24d984347,
        worktrees one level deeper 19a8e93ad3f52888, and git fetch rc 128 there."""
        genv = _v9_scratch_git_config(self.box)
        env = _v9_git_env(genv)
        a = support.Machine(self.box, "a")
        a.extra.update(genv)
        co = a.checkout
        _v9_sh("remote", "set-url", "origin", "../origin.git", cwd=co, env=env)
        (co / "src" / "origin.git").mkdir(parents=True)
        (self.box.root / "wts").mkdir()
        feature = self.box.root / "wts" / "feature"
        _v9_sh("worktree", "add", "-q", str(feature), "-b", "feature", cwd=co, env=env)
        (feature / "deep").mkdir()

        def fetch_rc(where):
            return subprocess.run(["git", "fetch", "-q", "origin"], cwd=where, env=env,
                                  capture_output=True, text=True).returncode

        def workspace(where):
            proc = a.run("info", "--json", cwd=where)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            return json.loads(proc.stdout)["workspace"]

        top = workspace(co)
        self.assertEqual(fetch_rc(co), 0)
        self.assertEqual(fetch_rc(co / "src"), 0)
        self.assertEqual(workspace(co / "src"), top)
        for where in (feature, feature / "deep"):
            with self.subTest(where=where.name):
                self.assertEqual(fetch_rc(where), 128)
                self.assertNotEqual(workspace(where), top)


def _v5_mode(path):
    return stat.S_IMODE(os.lstat(path).st_mode)

def _v5_modes(paths):
    return {str(p): oct(_v5_mode(p)) for p in paths}

class V5_DataFileModes(unittest.TestCase):
    def setUp(self):
        self.saved = os.umask(0o022)
        self.box = support.Sandbox()

    def tearDown(self):
        os.umask(self.saved)
        self.box.cleanup()

    def test_data_files_are_0600_and_checkout_files_follow_umask(self):
        """Verified (V5): files bead writes into its own store are 0600 under umask 022, 002 and 077:
        segments, claims/*.json, pending/*.json and *.command (after a crash before push and after a
        lost ack), meta.json, replica.json, aliases.json; BEAD_HOME and <ws> are 0700 after a write.
        Checkout files (.beadroll.json, export -o) follow the umask: 0644 / 0664 / 0600 (F04).
        The review measured 3 umasks x 1 full sequence; per umask 5 own claim files, 2 segments,
        2 pending files in each crash snapshot, and meta/replica/aliases all -rw-------."""
        for umask, expected in ((0o022, 0o644), (0o002, 0o664), (0o077, 0o600)):
            with self.subTest(umask=oct(umask)):
                os.umask(umask)
                m = support.Machine(self.box, f"v5-{umask:o}")
                m.bead("machines", "--name", "deadbeef=laptop")
                m.bead("init", "--prefix", "ex", "--tracked", "--remote", str(self.box.bare(f"r{umask:o}.git")))
                key = m.bead("create", "x")["id"]
                crashed = m.run("claim", key, "--json", BEAD_TEST_CRASH_BEFORE_PUSH="1")
                self.assertEqual(crashed.returncode, 98, crashed.stderr)
                pending = sorted((m.store() / "pending").iterdir())
                self.assertEqual(len(pending), 2, pending)
                self.assertEqual(set(_v5_modes(pending).values()), {oct(0o600)}, _v5_modes(pending))
                m.bead("claim", key)
                other = m.bead("create", "y")["id"]
                lost = m.run("claim", other, "--json", BEAD_TEST_CRASH_AFTER_PUSH="1")
                self.assertEqual(lost.returncode, 99, lost.stderr)
                after_ack = sorted((m.store() / "pending").iterdir())
                self.assertEqual(len(after_ack), 2, after_ack)
                self.assertEqual(set(_v5_modes(after_ack).values()), {oct(0o600)}, _v5_modes(after_ack))
                m.bead("claim", other)
                m.bead("export", "-o", "issues.jsonl")
                data = [*m.store().glob("events/*/*.jsonl"), *m.store().glob("events/*/claims/*.json"),
                        m.store() / "meta.json", m.home / "replica.json", m.home / "aliases.json"]
                self.assertTrue(all(p.exists() for p in data), data)
                self.assertTrue(list(m.store().glob("events/*/claims/*.json")))
                self.assertEqual(set(_v5_modes(data).values()), {oct(0o600)}, _v5_modes(data))
                self.assertEqual(set(_v5_modes((m.home, m.store())).values()), {oct(0o700)})
                for path in (m.checkout / ".beadroll.json", m.checkout / "issues.jsonl"):
                    self.assertEqual(oct(_v5_mode(path)), oct(expected), f"{path.name} under {umask:o}")

class V5_Retighten(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()

    def tearDown(self):
        self.box.cleanup()

    def test_a_write_retightens_a_relaxed_store(self):
        """Verified (V5): a write command chmods BEAD_HOME and <ws> back to 0700 and a read command
        (list) does not. The review measured relaxed 0755/0755 -> after list 0755/0755 -> after
        create 0700/0700."""
        m = support.Machine(self.box, "v5-relax")
        m.bead("init", "--prefix", "ex")
        os.chmod(m.home, 0o755)
        os.chmod(m.store(), 0o755)
        m.bead("list")
        self.assertEqual(set(_v5_modes((m.home, m.store())).values()), {oct(0o755)})
        m.bead("create", "x")
        self.assertEqual(set(_v5_modes((m.home, m.store())).values()), {oct(0o700)})

    def test_existing_group_directory_becomes_owner_only(self):
        """Verified (V5): an existing, user-chosen BEAD_HOME (a 0775 team directory) is silently made
        0700 by the first write (design observation). The review measured 0775 -> 0700 after init."""
        m = support.Machine(self.box, "v5-group")
        m.home.mkdir()
        os.chmod(m.home, 0o775)
        self.assertEqual(oct(_v5_mode(m.home)), oct(0o775))
        made = m.run("init", "--prefix", "ex", "--json")
        self.assertEqual(made.returncode, 0, made.stderr)
        self.assertEqual(oct(_v5_mode(m.home)), oct(0o700))

class V5_GitDirInHook(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()

    def tearDown(self):
        self.box.cleanup()

    def test_sync_repo_lands_in_the_store_with_git_dir_set(self):
        """Verified (V5): bead run from a git hook (GIT_DIR set, relative '.git' or absolute) still
        creates sync.git in the store and leaves the checkout's .git alone. The review measured 2
        variants: sync rc 0, sync.git/HEAD present in the store, checkout core.bare 'false' before
        and after, `git status` rc 0."""
        pub = support.Machine(self.box, "v5-pub")
        pub.bead("init", "--prefix", "ex", "--remote", str(self.box.origin))
        pub.bead("create", "x")
        pub.bead("sync")
        for variant in ("rel", "abs"):
            with self.subTest(variant=variant):
                m = support.Machine(self.box, f"v5-hook-{variant}")
                gitdir = ".git" if variant == "rel" else str(m.checkout / ".git")
                before = support.git("config", "--get", "core.bare", cwd=m.checkout).strip()
                self.assertEqual(before, "false")
                synced = m.run("sync", "--json", GIT_DIR=gitdir)
                self.assertEqual(synced.returncode, 0, synced.stderr)
                self.assertTrue((m.store() / "sync.git" / "HEAD").exists())
                self.assertFalse((m.checkout / "sync.git").exists())
                after = support.git("config", "--get", "core.bare", cwd=m.checkout).strip()
                self.assertEqual(after, "false")
                status = subprocess.run(["git", "status", "--porcelain"], cwd=m.checkout,
                                        capture_output=True, text=True)
                self.assertEqual(status.returncode, 0, status.stderr)

class V5_SideFilesIgnored(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()

    def tearDown(self):
        self.box.cleanup()

    def test_leftover_side_files_are_never_read_as_journal(self):
        """Verified (V5): side files next to journals (`.X.<pid>.tmp`, `*.torn-*`, `*.fork-*`, in
        events/<m>/ and in claims/) are never read as segments or attempts and never published. The
        review wrote garbage into all five name shapes, then measured sync integrity 0, info
        integrity [], segments 1, and export byte-identical before and after."""
        m = support.Machine(self.box, "v5-side")
        m.bead("init", "--prefix", "ex", "--remote", str(self.box.origin))
        key = m.bead("create", "x")["id"]
        m.bead("claim", key)
        before = m.export()
        own = m.store() / "events" / json.loads((m.home / "replica.json").read_text())["id"]
        self.assertTrue((own / "claims").is_dir())
        for name in (".000001.jsonl.1.tmp", "000001.jsonl.torn-1", "000001.jsonl.fork-1"):
            (own / name).write_text('{"garbage": true}\n{"v":1,"id":"not-an-event"}\n')
        for name in (".x.json.1.tmp", "x.json.fork-1"):
            (own / "claims" / name).write_text("{\n")
        synced = m.bead("sync")
        info = m.bead("info")
        self.assertEqual((synced["integrity"], info["integrity"], info["segments"]), (0, [], 1))
        self.assertEqual(m.export(), before)

class V5_KillAtRename(unittest.TestCase):
    KILLER = (
        "import os, signal\n"
        "_target = os.environ.get('BEAD_KILL_AT_REPLACE')\n"
        "if _target:\n"
        "    _real = os.replace\n"
        "    def _replace(src, dst, *a, **k):\n"
        "        if _target in str(dst):\n"
        "            os.kill(os.getpid(), signal.SIGKILL)\n"
        "        return _real(src, dst, *a, **k)\n"
        "    os.replace = _replace\n"
    )

    def setUp(self):
        if not hasattr(signal, "SIGKILL"):
            self.skipTest("platform has no SIGKILL")
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.inject = self.box.root / "inject"
        self.inject.mkdir()
        (self.inject / "sitecustomize.py").write_text(self.KILLER)

    def test_kill_at_rename_keeps_targets_whole(self):
        """Verified (V5): a SIGKILL between the temp write and the rename never damages the target:
        an export -o target keeps its old bytes, and a claim killed while writing its confirmed
        attempt file is recognised on retry. The review measured export killed rc -9 with target
        'previous export' and a leftover .issues.jsonl.<pid>.tmp; claim killed rc -9 leaving
        claims/.<id>.json.<pid>.tmp plus pending json and command; retry claim rc 0, owner
        codex:<thread>@<machine>, integrity [], pending_ownership []."""
        m = support.Machine(self.box, "v5k")
        probe = subprocess.run(["python3", "-c", "import sitecustomize; print(sitecustomize.__file__)"],
                               env=m.env(PYTHONPATH=str(self.inject)), capture_output=True, text=True)
        if probe.returncode or Path(probe.stdout.strip()).resolve() != (self.inject / "sitecustomize.py").resolve():
            self.skipTest("python3 on PATH does not load sitecustomize from PYTHONPATH: fault injection unavailable")
        m.bead("init", "--prefix", "ex", "--remote", str(self.box.origin))
        key = m.bead("create", "one")["id"]
        (m.checkout / "issues.jsonl").write_text("previous export\n")
        killed = m.run("export", "-o", "issues.jsonl", PYTHONPATH=str(self.inject), BEAD_KILL_AT_REPLACE="issues.jsonl")
        self.assertEqual(killed.returncode, -signal.SIGKILL, killed.stderr)
        self.assertEqual((m.checkout / "issues.jsonl").read_text(), "previous export\n")
        self.assertTrue(list(m.checkout.glob(".issues.jsonl.*.tmp")))
        killed = m.run("claim", key, "--json", PYTHONPATH=str(self.inject), BEAD_KILL_AT_REPLACE="/claims/")
        self.assertEqual(killed.returncode, -signal.SIGKILL, killed.stderr)
        self.assertTrue(list(m.store().glob("events/*/claims/.*.json.*.tmp")))
        self.assertEqual(len(list((m.store() / "pending").iterdir())), 2)
        retried = m.run("claim", key, "--json")
        self.assertEqual(retried.returncode, 0, retried.stderr)
        info = m.bead("info")
        self.assertEqual((info["integrity"], info["pending_ownership"]), ([], []))
        self.assertEqual(m.show(key)["claim"]["owner"], "codex:v5k-thread-0001@" + info["machine"])

class V5_SigningConfigIsInert(unittest.TestCase):
    def setUp(self):
        if os.name != "posix" or not Path("/bin/sh").exists():
            self.skipTest("needs /bin/sh for the fake gpg program")
        self.box = support.Sandbox()

    def tearDown(self):
        self.box.cleanup()

    def test_global_commit_signing_does_not_affect_claims(self):
        """Verified (V5): a global commit.gpgsign=true does not reach bead's `git commit-tree`
        (git 2.55): claims and syncs succeed and gpg is never called. The review measured, with a
        gpg.program that logs and exits 2, claim rc 0 (0.8 s), sync rc 0, and the gpg log never
        created."""
        log = self.box.root / "gpg.log"
        gpg = self.box.root / "fakegpg"
        gpg.write_text(f"#!/bin/sh\necho called >> '{log}'\necho 'gpg: signing failed' >&2\nexit 2\n")
        gpg.chmod(0o755)
        config = self.box.root / "global"
        config.write_text(f"[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = {gpg}\n")
        m = support.Machine(self.box, "v5s")
        m.bead("init", "--prefix", "ex", "--remote", str(self.box.origin))
        key = m.bead("create", "one")["id"]
        m.extra["GIT_CONFIG_GLOBAL"] = str(config)
        m.extra["GIT_CONFIG_NOSYSTEM"] = "1"
        claimed = m.run("claim", key, "--json")
        self.assertEqual(claimed.returncode, 0, claimed.stderr)
        m.bead("create", "two")
        synced = m.run("sync", "--json")
        self.assertEqual(synced.returncode, 0, synced.stderr)
        self.assertFalse(log.exists())
        self.assertEqual(m.show(key)["claim"]["owner"], "codex:v5s-thread-0001@" + m.bead("info")["machine"])


def _v2_machine(box, name, init=True):
    machine = support.Machine(box, name)
    machine.bead(*(["init", "--prefix", "ex", "--offline"] if init else ["sync"]))  # local until the first claim or sync
    return machine

def _v2_pending(machine):
    folder = machine.store() / "pending"
    return sorted(p.name for p in folder.iterdir()) if folder.is_dir() else []

def _v2_releases(machine, key):
    return [event["op"] for event in machine.bead("log", key, "--limit", "0")].count("release")

def _v2_remote_notes(box):
    """The kind of each commit on the remote journal, oldest first ("claim", "fence", ...)."""
    subjects = support.git("log", "--reverse", "--format=%s", "refs/beads/journal", cwd=box.origin).split()
    return [word for word in subjects if word in ("claim", "fence", "sync")]

class V2_ReceiptsArePerActor(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_other_actor_on_the_same_machine_never_drops_a_receipt(self):
        """Verified (V2): receipts are per actor. Another thread on the same machine whose `claim Z`
        crashes after its build (before the push, rc 98) and whose `claim Y` then succeeds (rc 0) leaves
        A's settled receipt for its lost-ack `release X` in place, and A's retry is recognised. The
        review measured 1 receipt left after B's rc-98 and rc-0 claims and A's retry `release X` rc 0
        with X open (1/1 test run plus 1 repro run)."""
        a = _v2_machine(self.box, "v2-actors")
        x = a.bead("create", "X")["id"]
        y = a.bead("create", "Y")["id"]
        z = a.bead("create", "Z")["id"]
        a.bead("claim", x)
        a.bead("sync")
        self.assertEqual(a.run("release", x, "--json", BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
        other = a.run("claim", z, "--json", thread="other", BEAD_TEST_CRASH_BEFORE_PUSH="1")
        self.assertEqual(other.returncode, 98, other.stderr)
        landed = a.run("claim", y, "--json", thread="other")
        self.assertEqual(landed.returncode, 0, landed.stderr)
        self.assertEqual(len(list((a.store() / "pending").glob("*.command"))), 1, _v2_pending(a))
        retry = a.run("release", x, "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(a.show(x)["status"], "open")
        self.assertNotIn("claim", a.show(x))
        self.assertEqual(_v2_releases(a, x), 1)
        self.assertEqual(a.show(y)["claim"]["thread"], "other")

class V2_InFlightAttemptAcrossActors(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def _hook(self, script):
        hook = self.box.origin / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\n" + script)
        hook.chmod(0o755)
        self.addCleanup(hook.unlink, missing_ok=True)
        return hook

    def _in_flight(self, lands_first):
        real_sleep = time.sleep
        m = _v2_machine(self.box, f"v2-flight-{int(lands_first)}")
        x = m.bead("create", "X", "--offline")["id"]  # the claim below is the first push the hook sees
        y = m.bead("create", "Y", "--offline")["id"]
        m.bead("claim", x, thread="actor-a")
        m.bead("sync")
        gate, started, done = self.box.root / "gate", self.box.root / "started", self.box.root / "done"
        gate.mkdir()
        g, s, d = (shlex.quote(str(p)) for p in (gate, started, done))
        # The first push to arrive (A's release) stalls in the hook until the gate is removed.
        self._hook(f"if mkdir {s} 2>/dev/null; then\n  while [ -d {g} ]; do sleep 0.05; done\n"
                   f"  touch {d}; exit 0\nfi\nexit 0\n")
        seen, pending_after_b = [], []
        try:
            with mock.patch.dict(os.environ, m.env(thread="actor-a", BEAD_NET_TIMEOUT="1"), clear=True), \
                    contextlib.chdir(m.checkout):
                bp = support.load_module()

                def between_rounds(seconds):  # A's lock is released and its push is still in flight
                    if seen:
                        return
                    if lands_first:
                        gate.rmdir()
                        for _ in range(400):
                            if done.exists():
                                break
                            real_sleep(0.05)
                        real_sleep(0.3)
                    seen.append(m.run("claim", y, "--json", thread="actor-b").returncode)
                    pending_after_b.extend(_v2_pending(m))

                with open(os.devnull, "w") as sink, mock.patch.object(bp.time, "sleep", between_rounds), \
                        contextlib.redirect_stdout(sink):
                    rc = bp.run(["release", x, "--json"])
        finally:
            if gate.exists():
                gate.rmdir()
        for _ in range(200):  # let the stalled receive finish before anything else touches the remote
            if done.exists():
                break
            real_sleep(0.05)
        real_sleep(0.3)
        self.assertEqual((rc, seen), (0, [0]))
        self.assertEqual(_v2_pending(m), [])
        m.bead("sync")
        peer = _v2_machine(self.box, f"v2-flight-peer-{int(lands_first)}", init=False)
        for machine in (m, peer):
            self.assertEqual(machine.show(x)["status"], "open")
            self.assertNotIn("claim", machine.show(x))
            self.assertEqual(_v2_releases(machine, x), 1)
            self.assertEqual(machine.show(y)["claim"]["thread"], "actor-b")
        self.assertEqual(_v2_remote_notes(self.box),
                         ["claim", "claim", "claim"] if lands_first else ["claim", "fence", "claim", "claim"])
        return pending_after_b

    def test_another_actor_fences_an_in_flight_release_during_the_unlocked_sleep(self):
        """Verified (V2): during A's unlocked sleep between rounds, with A's `release X` still in flight
        (stalled receive hook, BEAD_NET_TIMEOUT=1), actor B on the same machine fences and drops A's
        attempt and receipt; A's next round rebuilds and releases exactly once. The review measured
        5/5 subprocess runs and 1/1 deterministic run: A rc 0, B's `claim Y` rc 0, X open with no claim
        and exactly 1 release event on machine m and on a fresh peer, Y claimed by actor-b, no pending
        files; remote history claim, fence, claim, claim (the stalled first push's ref update failed after
        the fence)."""
        self.assertEqual(self._in_flight(lands_first=False), [])  # B dropped A's attempt and its receipt

    def test_in_flight_release_lands_before_another_actors_command(self):
        """Verified (V2): when A's in-flight `release X` push lands before B's command, B's merge
        confirms A's attempt and keeps A's receipt (B's settle_commands skips another actor's receipts),
        and A's next round returns its result. The review measured 1/1 run (plus 1 repro run): pending
        after B = [<id>.command], A rc 0, X open, 1 release event on m and on the peer, Y claimed by
        actor-b, remote history without a fence (claim, claim, claim)."""
        pending_after_b = self._in_flight(lands_first=True)
        self.assertEqual(len(pending_after_b), 1, pending_after_b)
        self.assertTrue(pending_after_b[0].endswith(".command"), pending_after_b)

class V2_ConfirmedTwinKeepsTheRetryRecognised(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_identical_pending_twin_confirms_and_the_retry_is_recognised(self):
        """Verified (V2): the Q03 shape still recognises the retry. A `release X` that crashed after local
        confirmation, before its pending files were dropped, leaves a pending twin identical to the
        confirmed file plus its receipt; the retry confirms the twin normally, keeps the receipt long
        enough to be recognised, and does not release twice. The review measured (repro_d control,
        undamaged twin) retry `release X` rc 0 and pending empty afterwards (1/1)."""
        a = _v2_machine(self.box, "v2-twin")
        x = a.bead("create", "X")["id"]
        a.bead("claim", x)
        with mock.patch.dict(os.environ, a.env(), clear=True), contextlib.chdir(a.checkout):
            bp = support.load_module()
            ws, actor = bp.find_workspace(), bp.detect_actor(None)
            args = bp.build_parser().parse_args(["release", x])
            with support.patched("drop_pending", mock.MagicMock(side_effect=RuntimeError("crash after local confirm"))):
                with self.assertRaisesRegex(RuntimeError, "crash after local confirm"):
                    bp.cmd_release(args, ws, actor)
        pending = _v2_pending(a)
        self.assertEqual(sorted(name.rsplit(".", 1)[1] for name in pending), ["command", "json"], pending)
        twin = a.store() / "pending" / next(name for name in pending if name.endswith(".json"))
        confirmed = next(a.store().rglob(f"claims/{twin.name}"))
        self.assertEqual(twin.read_bytes(), confirmed.read_bytes())
        retry = a.run("release", x, "--json")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(_v2_pending(a), [])
        self.assertEqual(a.show(x)["status"], "open")
        self.assertEqual(_v2_releases(a, x), 1)


def _v1_locales():
    """The translated settings the review ran, plus the C control. Each blanks the ambient
    LC_ALL, LC_MESSAGES and LANGUAGE first (an empty value counts as unset), so that only the
    named variables decide git's language."""
    blank = {"LC_ALL": "", "LC_MESSAGES": "", "LANGUAGE": ""}
    return [
        ("C", {**blank, "LC_ALL": "C"}),
        ("LANG=de_DE.UTF-8", {**blank, "LANG": "de_DE.UTF-8"}),
        ("LC_ALL=de_DE.UTF-8", {**blank, "LC_ALL": "de_DE.UTF-8"}),
        ("LANG=fr_FR.UTF-8", {**blank, "LANG": "fr_FR.UTF-8"}),
        ("LANG=fr_FR.ISO8859-1", {**blank, "LANG": "fr_FR.ISO8859-1"}),
        ("LANGUAGE=de LANG=en_US.UTF-8", {**blank, "LANG": "en_US.UTF-8", "LANGUAGE": "de"}),
    ]

def _v1_bead_reads_rejected(stderr):
    """The test Link.publish applies to a failed push's stderr (bead, `publish`): any of these
    tokens means 'the remote definitely did not apply it', which a coordinated command retries."""
    return re.search(rb"\[rejected\]|\[remote rejected\]|non-fast-forward|fetch first|stale info|cannot lock ref|"
                     rb"failed to update ref", stderr) is not None

def _v1_git_is_translated(env, where):
    """Whether git answers in another language than English under `env`: a message from outside
    any repository, which every git translation covers."""
    (where / "not-a-repo").mkdir(exist_ok=True)
    probe = subprocess.run(["git", "rev-parse", "--git-dir"], cwd=where / "not-a-repo", capture_output=True,
                           env={**env, "GIT_CEILING_DIRECTORIES": str(where)})
    return probe.returncode != 0 and b"not a git repository" not in probe.stderr

def _v1_winner_identity():
    """Shell lines giving a hook's own commit an identity, whatever the user's git config holds."""
    return ("export GIT_AUTHOR_NAME=winner GIT_AUTHOR_EMAIL=w@example.invalid "
            "GIT_COMMITTER_NAME=winner GIT_COMMITTER_EMAIL=w@example.invalid\n")

def _v1_pid_alive(pid):
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True

class V1_PushSideTranslation(unittest.TestCase):
    """Push-side race classification survives translated git (run-git area)."""

    def test_git_keeps_the_rejection_tokens_bead_reads_untranslated(self):
        """Verified (V1): the push-side tokens Link.publish matches stay English under translated git,
        so a lost race is still read as 'rejected'. The review compared pure git across 6 locale
        settings (C, LANG=de_DE.UTF-8, LC_ALL=de_DE.UTF-8, LANG=fr_FR.UTF-8, LANG=fr_FR.ISO8859-1,
        LANGUAGE=de with LANG=en_US.UTF-8): in all 6, ' ! [rejected] ... (non-fast-forward)',
        ' ! [remote rejected] ... (pre-receive hook declined)', 'remote: error: cannot lock ref ...'
        and ' ! [remote rejected] ... (incorrect old value provided)' stay English, and the porcelain
        status line '!\\t<sha>:refs/beads/journal\\t[rejected] (non-fast-forward)' is untranslated; only
        'error: failed to push some refs' and the hints are translated ('Fehler: Fehler beim
        Versenden...', 'erreur : impossible de pousser...'). Locales without git translations skip."""
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        (box.root / "empty.gitconfig").write_text("")
        remote, client = box.bare("tokens-remote.git"), box.bare("tokens-client.git")
        base = {k: v for k, v in os.environ.items() if not k.startswith(("GIT_", "LC_", "LANG"))}
        base.update(GIT_CONFIG_GLOBAL=str(box.root / "empty.gitconfig"), GIT_CONFIG_NOSYSTEM="1",
                    GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
                    GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")

        def git(*args, env=None, data=None, git_dir=client):
            return subprocess.run(["git", *args], env={**(env or base), "GIT_DIR": str(git_dir)},
                                  input=data, capture_output=True)

        blob = git("hash-object", "-w", "--stdin", data=b"x\n").stdout.decode().strip()
        tree = git("mktree", data=f"100644 blob {blob}\tf\n".encode()).stdout.decode().strip()
        c1 = git("commit-tree", tree, "-m", "one").stdout.decode().strip()
        c2 = git("commit-tree", tree, "-m", "two").stdout.decode().strip()
        c3 = git("commit-tree", tree, "-p", c1, "-m", "three").stdout.decode().strip()
        self.assertEqual(git("push", "-q", str(remote), f"{c1}:refs/beads/journal").returncode, 0)
        hook = remote / "hooks" / "pre-receive"
        refuse = "#!/bin/sh\ncat >/dev/null\necho 'policy: refs/beads/* are read-only here' >&2\nexit 1\n"
        race = ("#!/bin/sh\ncat >/dev/null\nunset GIT_QUARANTINE_PATH GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES\n"
                f"{_v1_winner_identity()}W=$(GIT_DIR={remote} git rev-parse refs/beads/journal)\n"
                f"N=$(GIT_DIR={remote} git commit-tree $W^{{tree}} -p $W -m winner)\n"
                f"GIT_DIR={remote} git update-ref refs/beads/journal $N $W\nexit 0\n")
        for name, loc in _v1_locales():
            with self.subTest(locale=name):
                env = {**base, **loc}
                if name != "C" and not _v1_git_is_translated(env, box.root):
                    self.skipTest(f"git has no translation for {name} on this machine")
                hook.unlink(missing_ok=True)
                git("update-ref", "refs/beads/journal", c1, git_dir=remote)
                # 1. client-side lost race: the remote head is not an ancestor of what we push
                lost = git("push", "--quiet", str(remote), f"{c2}:refs/beads/journal", env=env)
                self.assertNotEqual(lost.returncode, 0)
                self.assertRegex(lost.stderr, rb" ! \[rejected\] +\S+ -> refs/beads/journal \(non-fast-forward\)")
                self.assertTrue(_v1_bead_reads_rejected(lost.stderr), lost.stderr)
                porcelain = git("push", "--porcelain", str(remote), f"{c2}:refs/beads/journal", env=env)
                self.assertIn(f"!\t{c2}:refs/beads/journal\t[rejected] (non-fast-forward)".encode(), porcelain.stdout)
                if name == "C":
                    self.assertIn(b"error: failed to push some refs", lost.stderr)
                else:
                    self.assertNotIn(b"failed to push some refs", lost.stderr)  # the locale took effect
                # 2. a policy refusal by pre-receive
                hook.write_text(refuse)
                hook.chmod(0o755)
                refused = git("push", "--quiet", str(remote), f"{c3}:refs/beads/journal", env=env)
                self.assertNotEqual(refused.returncode, 0)
                self.assertIn(b"[remote rejected]", refused.stderr)
                self.assertIn(b"(pre-receive hook declined)", refused.stderr)
                self.assertTrue(_v1_bead_reads_rejected(refused.stderr), refused.stderr)
                # 3. server-side lost race: a concurrent winner moves the ref while ours is received
                hook.write_text(race)
                raced = git("push", "--quiet", str(remote), f"{c3}:refs/beads/journal", env=env)
                self.assertNotEqual(raced.returncode, 0)
                self.assertRegex(raced.stderr, rb"remote: error: cannot lock ref 'refs/beads/journal'")
                self.assertIn(b"[remote rejected]", raced.stderr)
                self.assertIn(b"(incorrect old value provided)", raced.stderr)
                self.assertTrue(_v1_bead_reads_rejected(raced.stderr), raced.stderr)

    def test_lost_races_are_recognised_and_retried_under_translated_git(self):
        """Verified (V1): under translated git a coordinated publication that loses a race is still
        recognised as rejected and retried. The review ran bead end to end under LANG=de_DE.UTF-8: a
        server-side race inside one sync (a pre-receive moves the ref once, so git reports 'cannot lock
        ref') gave sync rc=0 with attempts=2; its TestCase passed 3 subtests (LANG=de_DE.UTF-8,
        LANG=fr_FR.UTF-8, LANGUAGE=de with LANG=en_US.UTF-8) on e9eed92. The first publication runs
        with LC_ALL=C, because the fetch side of a never-published remote is translated (lead
        observation 1). Locales without git translations skip."""
        measured = ("LANG=de_DE.UTF-8", "LANG=fr_FR.UTF-8", "LANGUAGE=de LANG=en_US.UTF-8")
        for name, loc in [(n, l) for n, l in _v1_locales() if n in measured]:
            with self.subTest(locale=name):
                box = support.Sandbox()
                self.addCleanup(box.cleanup)
                m = support.Machine(box, "x")
                m.extra.update(loc)
                if not _v1_git_is_translated(m.env(), box.root):
                    self.skipTest(f"git has no translation for {name} on this machine")
                m.bead("init", "--prefix", "ex")
                key = m.bead("create", "one")["id"]
                first = m.run("sync", "--json", LC_ALL="C")  # first publication
                self.assertEqual(first.returncode, 0, first.stderr)
                gate = box.root / "raced"
                hook = box.origin / "hooks" / "pre-receive"  # a concurrent winner moves the ref, once
                hook.write_text(
                    "#!/bin/sh\ncat >/dev/null\nunset GIT_QUARANTINE_PATH GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES\n"
                    f"{_v1_winner_identity()}if mkdir {gate} 2>/dev/null; then\n"
                    f"  W=$(GIT_DIR={box.origin} git rev-parse refs/beads/journal)\n"
                    f"  N=$(GIT_DIR={box.origin} git commit-tree $W^{{tree}} -p $W -m winner)\n"
                    f"  GIT_DIR={box.origin} git update-ref refs/beads/journal $N $W\nfi\nexit 0\n")
                hook.chmod(0o755)
                m.bead("comments", "add", key, "racing", "--offline")  # the sync below is the racing push
                synced = m.run("sync", "--json")
                self.assertEqual(synced.returncode, 0, synced.stderr)
                self.assertTrue(gate.exists())
                self.assertEqual(json.loads(synced.stdout)["attempts"], 2)

class V1_StalledReceiveTimeouts(unittest.TestCase):
    """Stalled receive sides do not hold bead past BEAD_NET_TIMEOUT; their outcomes settle later."""

    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)
        self.a = support.Machine(self.box, "a")
        self.a.bead("init", "--prefix", "ex")
        self.key = self.a.bead("create", "one")["id"]
        self.a.bead("sync")

    def test_stalled_receive_with_a_stderr_flood_returns_within_the_timeout(self):
        """Verified (V1): a receive side that floods stderr and then stalls does not hold bead past
        BEAD_NET_TIMEOUT (no hang, no pipe deadlock); the receive side finishes on its own and the next
        sync settles it. The review measured, at BEAD_NET_TIMEOUT=2, REMOTE_UNREACHABLE after 2.33 s
        (pre-receive sleeping 6 s), 2.32 s (200 KiB on stderr, then 6 s), 2.34 s and 2.34 s (a backgrounded
        child holding stderr, pre- and post-receive); leftovers were only git-receive-pack and the hook,
        reparented, no zombies, all gone after 7 s. Its TestCase: at 1.5 s the stalled sync returned in
        under 3.5 s, and the next sync left no pending attempts."""
        done = self.box.root / "hook-done"
        hook = self.box.origin / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\ncat >/dev/null\nhead -c 204800 /dev/zero | tr '\\0' x >&2\n"
                        f"sleep 4\ntouch {done}\nexit 0\n")
        hook.chmod(0o755)
        self.a.bead("comments", "add", self.key, "stalled", "--offline")  # the sync below meets the stall
        started = time.monotonic()
        stalled = self.a.run("sync", "--json", BEAD_NET_TIMEOUT="1.5")
        self.assertLess(time.monotonic() - started, 1.5 + 2.0)
        self.assertEqual(support.code(stalled), "REMOTE_UNREACHABLE")
        deadline = time.monotonic() + 15
        while not done.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        self.assertTrue(done.exists(), "the receive side never finished")
        hook.unlink()
        time.sleep(0.3)
        settled = self.a.run("sync", "--json")
        self.assertEqual(settled.returncode, 0, settled.stderr)
        self.assertEqual(self.a.bead("info")["pending_ownership"], [])
        b = support.Machine(self.box, "b")
        b.bead("sync")
        self.assertIn("stalled", json.dumps(b.show(self.key)))

    def test_claim_whose_ack_is_held_by_a_slow_post_receive_is_recovered(self):
        """Verified (V1): a claim whose acknowledgement is held by a slow post-receive (the ref is
        already updated) is recovered rather than reported lost. The review measured, with a 4 s
        post-receive at BEAD_NET_TIMEOUT=1.5: claim rc 0, claim generation 1, no pending ownership left."""
        gate = self.box.root / "slow-once"
        done = self.box.root / "slow-done"
        hook = self.box.origin / "hooks" / "post-receive"
        hook.write_text(f"#!/bin/sh\ncat >/dev/null\nif mkdir {gate} 2>/dev/null; then sleep 4; touch {done}; fi\nexit 0\n")
        hook.chmod(0o755)
        claimed = self.a.run("claim", self.key, "--json", BEAD_NET_TIMEOUT="1.5")
        self.assertEqual(claimed.returncode, 0, claimed.stderr)
        self.assertTrue(gate.exists(), "the slow post-receive never ran")
        self.assertEqual(json.loads(claimed.stdout)[0]["claim"]["generation"], 1)
        self.assertEqual(self.a.bead("info")["pending_ownership"], [])
        deadline = time.monotonic() + 15  # let the receive side finish before the sandbox goes
        while not done.exists() and time.monotonic() < deadline:
            time.sleep(0.1)

class V1_RunGitTimeoutHygiene(unittest.TestCase):
    """In-process run_git timeouts: no fd leak, no zombie, helpers reaped as documented (F15)."""

    def test_timeouts_leak_nothing_and_reap_as_documented(self):
        """Verified (V1): timed-out run_git calls return on time, leak no file descriptor and leave no
        zombie; reap=True (fetch) kills the helpers git started, reap=False (push) leaves them running,
        as F15 documents. The review ran 20 calls per mode with timeout=0.2 against a git alias whose
        helper sleeps 3 s: reap=True 20 timeouts in 4.08 s, fds 4->4, zombies 0, leftover helpers 0;
        reap=False 4.10 s, fds 4->4, zombies 0, leftover helpers 13 (by design)."""
        if not os.path.isdir("/dev/fd"):
            self.skipTest("no /dev/fd to count this process's descriptors")
        box = support.Sandbox()
        self.addCleanup(box.cleanup)
        bead = support.load_module()
        log = box.root / "helper-pids"
        helper = box.root / "stall.sh"
        helper.write_text(f"#!/bin/sh\necho $$ >> {shlex.quote(str(log))}\nexec sleep 3\n")
        helper.chmod(0o755)
        args = ["-C", str(box.root), "-c", f"alias.stall=!{shlex.quote(str(helper))}", "stall"]
        survivors = []
        self.addCleanup(lambda: [os.kill(pid, 9) for pid in survivors if _v1_pid_alive(pid)])
        for reap, expect_left in ((True, False), (False, True)):
            with self.subTest(reap=reap):
                log.write_text("")
                before = len(os.listdir("/dev/fd"))
                started = time.monotonic()
                for _ in range(20):
                    with self.assertRaises(bead.BpError) as raised:
                        bead.run_git(args, timeout=0.2, reap=reap, check=False)
                    self.assertEqual(raised.exception.code, "REMOTE_UNREACHABLE")
                self.assertLess(time.monotonic() - started, 20 * 0.2 + 2)
                time.sleep(0.3)
                self.assertEqual(len(os.listdir("/dev/fd")), before)
                ps = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,stat="], capture_output=True, text=True).stdout
                zombies = [row for row in ps.splitlines()
                           if len(row.split()) >= 3 and row.split()[1] == str(os.getpid()) and "Z" in row.split()[2]]
                self.assertEqual(zombies, [])
                started_helpers = [int(pid) for pid in log.read_text().split()]
                self.assertTrue(started_helpers, "no helper started within the timeout")
                alive = [pid for pid in started_helpers if _v1_pid_alive(pid)]
                survivors.extend(alive)
                self.assertEqual(bool(alive), expect_left, (reap, alive))
                for pid in alive:
                    os.kill(pid, 9)


def _v6_code(proc):
    """The error code a failed `bead ... --json` call reported, TRACEBACK for a crash, else None."""
    try:
        return json.loads(proc.stderr)["error"]["code"]
    except (ValueError, KeyError, TypeError):
        return "TRACEBACK" if "Traceback" in proc.stderr else None

def _v6_limited(machine, limit, *argv):
    """Run `bead argv --json` as `machine` under a shell resource limit such as `-f 200` or `-n 8`."""
    line = f"ulimit {limit}; exec " + " ".join(shlex.quote(str(x)) for x in (support.BEAD, *argv, "--json"))
    return subprocess.run(["/bin/sh", "-c", line], cwd=machine.checkout, env=machine.env(),
                          capture_output=True, text=True, timeout=120)

def _v6_need_unprivileged(case):
    if os.geteuid() == 0:
        case.skipTest("runs as root: chmod 0 does not deny access, so EACCES cannot be produced")

def _v6_need_sh(case):
    if not os.access("/bin/sh", os.X_OK):
        case.skipTest("no /bin/sh to apply a ulimit with")

class V6_UserSuppliedPaths(unittest.TestCase):
    def setUp(self):
        _v6_need_unprivileged(self)
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_oserror_on_a_user_supplied_input_path_is_io_and_writes_nothing(self):
        """Verified (V6): an OSError on a user-supplied input path is answered with code IO and exit 1
        and writes nothing. The review ran 6 kinds (ENOENT, EISDIR, EACCES, ELOOP, ENAMETOOLONG,
        ENOTDIR) x 5 commands (update --description-file, create --description-file, comments add -f,
        import, update --claim --description-file): 30/30 IO exit 1, no traceback, the journal event
        count unchanged and no claim taken."""
        a = support.Machine(self.box, "a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "x")["id"]
        a.bead("sync")
        root = self.box.root
        (root / "afile").write_text("x")
        (root / "adir").mkdir()
        (root / "noread").write_text("x")
        (root / "noread").chmod(0)
        self.addCleanup((root / "noread").chmod, 0o600)
        os.symlink("loop", root / "loop")
        paths = [root / "missing", root / "adir", root / "noread", root / "loop", root / ("n" * 300), root / "afile" / "x"]
        before = a.bead("info")["events"]
        for path in paths:
            for argv in (["update", key, "--description-file", str(path)],
                         ["create", "t", "--description-file", str(path)],
                         ["comments", "add", key, "-f", str(path)],
                         ["import", str(path)],
                         ["update", key, "--claim", "--description-file", str(path)]):
                with self.subTest(path=path.name[:12], argv=argv[0]):
                    proc = a.run(*argv, "--json")
                    self.assertNotIn("Traceback", proc.stderr)
                    self.assertEqual((proc.returncode, _v6_code(proc)), (1, "IO"), proc.stderr)
        self.assertEqual(a.bead("info")["events"], before)
        self.assertNotIn("claim", a.show(key))

    def test_export_output_path_errors_are_io_and_missing_parents_are_created(self):
        """Verified (V6): `export -o` into an unusable path is IO, and missing parent directories are
        created. The review measured EISDIR, EACCES, ENAMETOOLONG and ENOTDIR all IO (ENOTDIR reported
        as 'File exists'), a path under missing parents exit 0, and `-o /dev/stdout` IO ('Operation not
        permitted: /dev/.stdout.PID.tmp')."""
        a = support.Machine(self.box, "a")
        a.bead("init", "--prefix", "ex")
        a.bead("create", "x")
        root = self.box.root
        (root / "afile").write_text("x")
        (root / "adir").mkdir()
        (root / "locked").mkdir()
        (root / "locked").chmod(0o500)
        self.addCleanup((root / "locked").chmod, 0o700)
        bad = {"EISDIR": root / "adir", "EACCES": root / "locked" / "out.jsonl",
               "ENAMETOOLONG": root / ("n" * 300), "ENOTDIR": root / "afile" / "out.jsonl"}
        if not os.access("/dev", os.W_OK):
            bad["/dev/stdout"] = "/dev/stdout"
        for name, path in bad.items():
            with self.subTest(case=name):
                proc = a.run("export", "-o", str(path), "--json")
                self.assertNotIn("Traceback", proc.stderr)
                self.assertEqual((proc.returncode, _v6_code(proc)), (1, "IO"), proc.stderr)
        self.assertEqual((root / "afile").read_text(), "x")
        self.assertEqual(list((root / "adir").iterdir()), [])
        target = root / "new" / "deeper" / "out.jsonl"
        proc = a.run("export", "-o", str(target), "--json")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(len(target.read_text().splitlines()), 1)

class V6_StoreDamage(unittest.TestCase):
    def setUp(self):
        _v6_need_unprivileged(self)
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_store_damage_is_answered_with_a_structured_code(self):
        """Verified (V6): damage to the local store is answered with a structured error code, never a
        traceback. The review ran 16 damages x 5 commands (show, create, claim, sync, comments add) =
        80 runs with 0 tracebacks; lock dir / lock 000 gave IO for writers with show ok, segment 000
        or a directory gave IO everywhere, pending/ as a file gave claim IO, and the replica's claims
        dir at 0500 gave claim IO after the push. This test covers 6 of those damages x the 5 commands
        and accepts only IO, GIT_ERROR or REMOTE_UNREACHABLE for a failure."""
        a = support.Machine(self.box, "a")
        a.bead("init", "--prefix", "ex")
        key = a.bead("create", "x")["id"]
        a.bead("sync")
        store = a.store()
        replica = json.loads((a.home / "replica.json").read_text())["id"]
        seg = store / "events" / replica / "000001.jsonl"
        claims = store / "events" / replica / "claims"
        claims.mkdir(exist_ok=True)

        def as_dir(path):
            saved = path.with_name(path.name + ".saved")
            path.rename(saved)
            path.mkdir()
            return lambda: (path.rmdir(), saved.rename(path))

        def chmod(path, mode, back):
            path.chmod(mode)
            return lambda: path.chmod(back)

        def pending_file():
            (store / "pending").mkdir(exist_ok=True)
            (store / "pending").rmdir()
            (store / "pending").write_text("x")
            return lambda: (store / "pending").unlink()

        damages = {"lock dir": lambda: as_dir(store / "lock"), "lock 000": lambda: chmod(store / "lock", 0, 0o600),
                   "segment 000": lambda: chmod(seg, 0, 0o600), "segment dir": lambda: as_dir(seg),
                   "pending file": pending_file, "claims 0500": lambda: chmod(claims, 0o500, 0o700)}
        for name, damage in damages.items():
            for argv in (["show", key], ["create", "n"], ["claim", key], ["sync"], ["comments", "add", key, "c"]):
                repair = damage()
                try:
                    proc = a.run(*argv, "--json")
                finally:
                    repair()
                a.run("release", key)
                with self.subTest(damage=name, argv=argv[0]):
                    self.assertNotIn("Traceback", proc.stderr)
                    if proc.returncode:
                        self.assertIn(_v6_code(proc), {"IO", "GIT_ERROR", "REMOTE_UNREACHABLE"}, proc.stderr)

class V6_FileSizeLimit(unittest.TestCase):
    def setUp(self):
        _v6_need_sh(self)
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_file_size_limit_is_io_and_the_next_write_and_sync_are_clean(self):
        """Verified (V6): EFBIG from a file-size limit is IO, and the next write and sync are clean.
        The review measured, under `ulimit -f 200`: create with a 300 KB --description-file IO
        ('[Errno 27] File too large'), the next create exit 0; sync pulling another machine's 300 KB
        segment IO, the next sync exit 0; integrity [] afterwards. Under `ulimit -f 1200` both
        succeeded."""
        a = support.Machine(self.box, "a")
        a.bead("init", "--prefix", "ex")
        a.bead("sync")
        other = support.Machine(self.box, "other")
        other.bead("sync")
        body = self.box.root / "big.txt"
        body.write_text("z" * 300000)

        created = _v6_limited(a, "-f 200", "create", "big", "--description-file", str(body))
        self.assertNotIn("Traceback", created.stderr)
        self.assertEqual(_v6_code(created), "IO", created.stderr)
        self.assertEqual(a.run("create", "after", "--json").returncode, 0)
        other.bead("create", "growth", "--description-file", str(body))
        other.bead("sync")
        synced = _v6_limited(a, "-f 200", "sync")
        self.assertNotIn("Traceback", synced.stderr)
        # Adapted on closing G58: git's output for bead now goes through a file bead reads back, so the limit
        # can hit git first (GIT_ERROR naming the SIGXFSZ that killed it) before bead's own segment write (IO).
        self.assertIn(_v6_code(synced), ("IO", "GIT_ERROR"), synced.stderr)
        self.assertTrue(re.search(r"File too large|SIGXFSZ", synced.stderr), synced.stderr)
        self.assertEqual(a.run("sync", "--json").returncode, 0)
        self.assertEqual(a.bead("info")["integrity"], [])
        self.assertIn("growth", {i["title"] for i in a.bead("list")["issues"]})

class V6_DescriptorExhaustion(unittest.TestCase):
    def setUp(self):
        _v6_need_sh(self)

    def test_descriptor_exhaustion_never_drops_a_pending_attempt(self):
        """Verified (V6): running out of file descriptors never drops a pending attempt and fails
        without a traceback. The review measured, after a lost-ack `update --claim --status closed`
        (crash after push): under `ulimit -n` 8, 9, 10 and 11, `show X`, `claim Y` and `sync` leave the
        pending .json and .command both in place, and the retry exits 0. (At -n 6/8 everything is IO
        'Too many open files'; at -n >= 12 commands succeed.) This test runs limits 8, 10 and 11.

        Adapted on closing G58: git's output is captured through files rather than pipes, so `claim` and `sync`
        get by with fewer descriptors and may now succeed at -n 10/11, in which case they confirm the attempt
        (the remote holds it) rather than leave it pending; either way it is never dropped. Once a later command
        of the actor has gone through, the retry of the lost one is answered on its merits (x is closed)."""
        for limit in (8, 10, 11):
            with self.subTest(limit=limit):
                box = support.Sandbox()
                self.addCleanup(box.cleanup)
                a = support.Machine(box, f"fd{limit}")
                a.bead("init", "--prefix", "ex")
                x = a.bead("create", "x")["id"]
                y = a.bead("create", "y")["id"]
                a.bead("sync")
                argv = ["update", x, "--claim", "--status", "closed"]
                self.assertEqual(a.run(*argv, "--json", BEAD_TEST_CRASH_AFTER_PUSH="1").returncode, 99)
                confirmed = False
                for cmd in (["show", x], ["claim", y], ["sync"]):
                    proc = _v6_limited(a, f"-n {limit}", *cmd)
                    self.assertNotIn("Traceback", proc.stderr)
                    pending = sorted(p.suffix for p in (a.store() / "pending").iterdir())
                    if pending != [".command", ".json"]:  # settled, then: the attempt is confirmed (x was open)
                        self.assertEqual((pending, proc.returncode, a.show(x)["status"]), ([], 0, "closed"),
                                         f"ulimit -n {limit} {cmd[0]}: {proc.stderr}")
                        confirmed = True
                retry = a.run(*argv, "--json")
                self.assertTrue(retry.returncode == 0 or (confirmed and _v6_code(retry) == "VALIDATION"), retry.stderr)
                self.assertEqual(a.show(x)["status"], "closed")

class V6_IdentityAndConfigShapes(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_identity_and_tracked_config_shapes_are_validated(self):
        """Verified (V6): replica.json and .beadroll.json shapes are validated. The review measured
        replica.json with an integer alias or extra keys: create, machines and claims exit 0. The
        refs refs/heads/x, refs/beads/../x, refs/beads/x.lock and 'refs/beads/a b' are CONFIG; the
        workspaces '.', '..', 101 characters, 'wö' and 'w1\\n' are CONFIG; unknown keys are
        accepted."""
        a = support.Machine(self.box, "a")
        a.bead("init", "--prefix", "ex")
        a.bead("create", "x")
        replica = a.home / "replica.json"
        ident = json.loads(replica.read_text())["id"]
        for body in ({"id": ident, "alias": 5}, {"id": ident, "alias": "m", "extra": [1]}):
            replica.write_text(json.dumps(body))
            for argv in (["create", "y"], ["machines"], ["claims"]):
                with self.subTest(replica=body, argv=argv[0]):
                    proc = a.run(*argv, "--json")
                    self.assertEqual(proc.returncode, 0, proc.stderr)
        config = a.checkout / ".beadroll.json"
        bad = [{"workspace": "w", "ref": r} for r in ("refs/heads/x", "refs/beads/../x", "refs/beads/x.lock", "refs/beads/a b")]
        bad += [{"workspace": w} for w in (".", "..", "a" * 101, "wö", "w1\n")]
        for body in bad:
            config.write_text(json.dumps(body))
            with self.subTest(config=body):
                proc = a.run("info", "--json")
                self.assertNotIn("Traceback", proc.stderr)
                self.assertEqual(_v6_code(proc), "CONFIG", proc.stderr)
        config.write_text(json.dumps({"workspace": "w", "future": {"x": 1}}))
        proc = a.run("info", "--json")
        self.assertEqual(proc.returncode, 0, proc.stderr)


def _v7_git_env(root, **more):
    """git with no user, system or global config, no proxies and no prompts: it sees only `root`."""
    home = Path(root) / "v7-home"
    home.mkdir(exist_ok=True)
    config = home / "gitconfig"
    config.touch()
    env = {k: v for k, v in os.environ.items()
           if not k.startswith("GIT_") and k.lower() not in ("http_proxy", "https_proxy", "all_proxy", "no_proxy",
                                                            "ssh_askpass")}
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home), GIT_CONFIG_GLOBAL=str(config), GIT_CONFIG_NOSYSTEM="1",
               GIT_TERMINAL_PROMPT="0", NO_PROXY="*", no_proxy="*")
    env.update(more)
    return env

def _v7_fake_ssh(root):
    """An ssh stand-in for GIT_SSH_COMMAND: it runs the remote command locally, in $V7_SSH_ROOT."""
    path = Path(root) / "v7-fake-ssh"
    path.write_text("#!/bin/sh\n"
                    "[ \"$1\" = \"-G\" ] && exit 0\n"
                    "for last; do :; done\n"
                    "PATH=\"$(git --exec-path):$PATH\"\n"
                    "cd \"$V7_SSH_ROOT\" && exec sh -c \"$last\"\n")
    path.chmod(0o755)
    return path

def _v7_info(test, machine, cwd=None):
    proc = machine.run("info", "--json", cwd=cwd)
    test.assertEqual(proc.returncode, 0, proc.stderr)
    return json.loads(proc.stdout)

def _v7_auth_server(test):
    """An HTTP server on 127.0.0.1 that answers every request 401 (Basic) and records its Authorization
    header. Returns (port, list of headers seen)."""
    seen = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            seen.append(self.headers.get("Authorization") or "")
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="v7"')
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    test.addCleanup(server.server_close)
    test.addCleanup(server.shutdown)
    return server.server_address[1], seen

def _v7_basic(header):
    """The user:password a Basic Authorization header carries ('' when it carries none)."""
    if not header.startswith("Basic "):
        return ""
    alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    bits = "".join(f"{alphabet.index(c):06b}" for c in header[6:].strip().rstrip("="))
    return bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits) - 7, 8)).decode(errors="replace")

class V7_OriginsWithAUserOrSchemeAreOneWorkspace(unittest.TestCase):
    def setUp(self):
        self.box = support.Sandbox()
        self.addCleanup(self.box.cleanup)

    def test_shapes_with_a_user_or_scheme_give_one_workspace_everywhere(self):
        """Verified (V7): an origin with a user part or a scheme (scp `git@host.example:org/repo.git`,
        `ssh://git@...`, `https://...`, `git://...`) gives one workspace id, and `info` reports the origin itself
        as the remote, in a clone, its linked worktree, a subdirectory and a second clone. The review measured
        4 shapes x 4 locations = 16 info calls and 1 distinct id, 987c37c1f56322a0."""
        one, two = support.Machine(self.box, "one"), support.Machine(self.box, "two")
        worktree = self.box.root / "one-wt"
        support.git("worktree", "add", "-q", str(worktree), cwd=one.checkout)
        (one.checkout / "sub").mkdir()
        ids, calls = set(), 0
        for url in ("git@host.example:org/repo.git", "ssh://git@host.example/org/repo.git",
                    "https://host.example/org/repo.git", "git://host.example/org/repo.git"):
            for machine in (one, two):
                support.git("remote", "set-url", "origin", url, cwd=machine.checkout)
            for where, machine, cwd in (("clone", one, None), ("worktree", one, worktree),
                                        ("subdirectory", one, one.checkout / "sub"), ("second clone", two, None)):
                with self.subTest(origin=url, where=where):
                    data = _v7_info(self, machine, cwd=cwd)
                    calls += 1
                    ids.add(data["workspace"])
                    self.assertEqual(data["remote"], url)
        self.assertEqual(calls, 16)
        self.assertEqual(ids, {"987c37c1f56322a0"})

    def test_scp_origin_with_a_user_syncs_over_ssh(self):
        """Verified (V7): an scp-like origin with a user part (`git@myhost:origin.git`, reached through an ssh
        stand-in that runs the remote command in the sandbox) syncs, and the clone, its linked worktree and a
        second clone agree on the workspace. The review measured sync rc 0 with pushed 1 and the one id
        606ef04ef5c6313e in all three places."""
        url = "git@myhost:origin.git"
        env = _v7_git_env(self.box.root, GIT_SSH_COMMAND=shlex.quote(str(_v7_fake_ssh(self.box.root))),
                          V7_SSH_ROOT=str(self.box.root))
        git_only = {k: v for k, v in env.items() if k.startswith("GIT_") or k == "V7_SSH_ROOT"}
        a, b = support.Machine(self.box, "a"), support.Machine(self.box, "b")
        for machine in (a, b):
            support.git("remote", "set-url", "origin", url, cwd=machine.checkout)
            machine.extra.update(git_only)
        reached = subprocess.run(["git", "ls-remote", "origin"], cwd=a.checkout, env=env, capture_output=True,
                                 text=True, timeout=60)
        self.assertEqual(reached.returncode, 0, f"git itself cannot reach {url}: {reached.stderr}")
        worktree = self.box.root / "a-wt"
        support.git("worktree", "add", "-q", str(worktree), cwd=a.checkout)
        made = a.run("init", "--prefix", "ex", "--offline", "--json")  # kept local: the sync below is the publish
        self.assertEqual(made.returncode, 0, made.stderr)
        synced = a.run("sync", "--json")
        self.assertEqual(synced.returncode, 0, synced.stderr)
        result = json.loads(synced.stdout)
        self.assertEqual(result["pushed"], 1, result)
        self.assertEqual(result["remote"], url)
        published = support.git("for-each-ref", "--format=%(refname)", "refs/beads", cwd=self.box.origin)
        self.assertIn("refs/beads/journal", published.split(), "the push did not reach the bare origin")
        seen = {"clone": _v7_info(self, a), "worktree": _v7_info(self, a, cwd=worktree), "second clone": _v7_info(self, b)}
        for where, info in seen.items():
            self.assertEqual(info["workspace"], "606ef04ef5c6313e", where)
            self.assertEqual(info["remote"], url, where)

class V7_PasswordShapesFollowGit(unittest.TestCase):
    def setUp(self):
        self.bead = support.load_module()
        self.root = Path(tempfile.mkdtemp(prefix="v7-urls-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)

    def _ls_remote(self, url, **more):
        return subprocess.run(["git", "ls-remote", url], cwd=self.root, env=_v7_git_env(self.root, **more),
                              capture_output=True, text=True, timeout=60)

    def test_git_confirmed_password_shapes_are_dropped(self):
        """Verified (V7): without_password drops the password of every shape git sends one for (user:password,
        a percent-encoded password, an upper-case scheme, :TOKEN, TOKEN@ on https, an IPv6 host with a port,
        ssh user:password, persistent-https) and leaves ssh users and ports, IPv6 hosts, scp with a user,
        `ssh://git%40corp@host` and '@' inside local paths unchanged. The review asserted 8 dropped and
        7 unchanged shapes (63 shapes tabulated, 29 checked against git).

        Adapted on closing G06: over http(s) the whole user-info goes, user included (the review's own G06 rule:
        `https://TOKEN:x-oauth-basic@h` carries the token as the user, and `https://alice@h` alone is dropped as a
        token, so keeping `alice@` from `alice:secret@` would only be dropped by the next application). An https
        user that selects among several credentials for one host belongs in git's credential.<url>.username."""
        dropped = {
            "https://alice:secret@host.example/org/repo.git": "https://host.example/org/repo.git",
            "https://alice:p%40ss@host.example/org/repo.git": "https://host.example/org/repo.git",
            "HTTPS://alice:secret@host.example/org/repo.git": "HTTPS://host.example/org/repo.git",
            "https://:TOKEN@host.example/org/repo.git": "https://host.example/org/repo.git",
            "https://TOKEN@host.example/org/repo.git": "https://host.example/org/repo.git",
            "https://alice:secret@[::1]:8443/org/repo.git": "https://[::1]:8443/org/repo.git",
            "ssh://git:secret@host.example/org/repo.git": "ssh://git@host.example/org/repo.git",
            "persistent-https://alice:secret@host.example/org/repo": "persistent-https://alice@host.example/org/repo",
        }
        for url, expected in dropped.items():
            with self.subTest(url=url):
                self.assertEqual(self.bead.without_password(url), (expected, True))
        unchanged = ("ssh://git@host.example:2222/org/repo.git", "ssh://git@[::1]:2222/org/repo.git",
                     "git@host.example:org/repo.git", "ssh://git%40corp@host.example/org/repo.git",
                     "https://[::1]:8443/org/repo.git", "/srv/git/a@b/repo.git", "file:///srv/git/a@b/repo.git")
        for url in unchanged:
            with self.subTest(url=url):
                self.assertEqual(self.bead.without_password(url), (url, False))
        self.assertEqual((len(dropped), len(unchanged)), (8, 7))

    def test_http_secrets_git_sends_are_the_ones_bead_drops(self):
        """Verified (V7): against a local 401 server recording Authorization, git sends the secret of
        `alice:secret`, `alice:p%40ss` (as `alice:p@ss`), an upper-case `HTTP://`, `:TOKEN` and `TOKEN@`; with the
        URL without_password returns, git sends no secret. This is the review's oracle (git ls-remote with a
        scratch GIT_CONFIG_GLOBAL and GIT_CONFIG_NOSYSTEM=1), which found all of these dropped correctly.
        Adapted on closing G06: the user `alice` is no longer kept over http(s) either (see
        test_git_confirmed_password_shapes_are_dropped), so git sends no user with bead's URL."""
        port, seen = _v7_auth_server(self)
        shapes = {  # template: (the secret git sends, the user git should still send with bead's URL)
            "http://alice:secret@{hp}/org/repo.git": ("secret", None),
            "http://alice:p%40ss@{hp}/org/repo.git": ("p@ss", None),
            "HTTP://alice:secret@{hp}/org/repo.git": ("secret", None),
            "http://:TOKEN@{hp}/org/repo.git": ("TOKEN", None),
            "http://TOKEN@{hp}/org/repo.git": ("TOKEN", None),
        }
        for template, (secret, user) in shapes.items():
            url = template.format(hp=f"127.0.0.1:{port}")
            with self.subTest(url=template):
                seen.clear()
                probe = self._ls_remote(url)
                if not seen:  # this git never contacted the server (git 2.43 rejects an upper-case scheme outright)
                    self.skipTest(f"this git sends nothing for {template}: {probe.stderr.strip()[:120]}")
                sent = [_v7_basic(h) for h in seen]
                self.assertTrue(any(secret in auth for auth in sent), f"git did not send {secret!r} from {url}: {sent}")
                rewritten, was_dropped = self.bead.without_password(url)
                self.assertTrue(was_dropped)
                seen.clear()
                self._ls_remote(rewritten)
                sent = [_v7_basic(h) for h in seen]
                self.assertTrue(seen, f"git did not reach the server with bead's {rewritten}")
                self.assertFalse(any(secret in auth for auth in sent), f"git still sends {secret!r} from {rewritten}")
                if user:
                    self.assertTrue(any(auth.startswith(user + ":") for auth in sent), sent)

    def test_ssh_logins_bead_keeps_are_what_git_hands_ssh(self):
        """Verified (V7): through a logging GIT_SSH_COMMAND, git hands ssh `git:secret@host.example` for
        `ssh://git:secret@...` (bead drops the password, and git then hands `git@host.example`), `git@corp@host.example`
        for `ssh://git%40corp@...`, the host `alice` for the scp `alice:secret@host.example:org/repo.git`, and
        `-p 2222 git@host.example` for an ssh port; bead leaves the last three and the scp `git@host` form
        unchanged, which matches git (the review's oracle over 29 shapes)."""
        log = self.root / "v7-ssh.log"
        script = self.root / "v7-ssh-log"
        script.write_text('#!/bin/sh\nfor a in "$@"; do printf "%s\\n" "$a" >> "$V7_SSH_LOG"; done\nexit 1\n')
        script.chmod(0o755)
        ssh = {"GIT_SSH_COMMAND": shlex.quote(str(script)), "V7_SSH_LOG": str(log)}

        def handed(url):
            log.write_text("")
            self._ls_remote(url, **ssh)
            return log.read_text().splitlines()

        args = handed("ssh://git:secret@host.example/org/repo.git")
        self.assertIn("git:secret@host.example", args, "git does not hand ssh the user:password login")
        rewritten, was_dropped = self.bead.without_password("ssh://git:secret@host.example/org/repo.git")
        self.assertEqual((rewritten, was_dropped), ("ssh://git@host.example/org/repo.git", True))
        args = handed(rewritten)
        self.assertIn("git@host.example", args)
        self.assertFalse(any("secret" in a for a in args), args)
        kept = {
            "ssh://git%40corp@host.example/org/repo.git": ["git@corp@host.example"],
            "alice:secret@host.example:org/repo.git": ["alice"],
            "ssh://git@host.example:2222/org/repo.git": ["-p", "2222", "git@host.example"],
            "git@host.example:org/repo.git": ["git@host.example"],
        }
        for url, expected in kept.items():
            with self.subTest(url=url):
                args = handed(url)
                for arg in expected:
                    self.assertIn(arg, args, f"git handed ssh {args} for {url}")
                self.assertEqual(self.bead.without_password(url), (url, False))

class V7_NormalizeRemote(unittest.TestCase):
    def setUp(self):
        self.bead = support.load_module()

    def test_spellings_of_one_remote_share_an_id_and_ports_separate(self):
        """Verified (V7): normalize_remote reduces one repository's usual spellings (scp, ssh://, https with
        .git and a trailing slash, an https user part, host case, git://, git+ssh://) to one string, and keeps
        distinct ports apart. The review measured 6 spellings giving 1 normalised string and 3 port variants
        giving 3."""
        same = ["git@github.com:Org/Repo.git", "ssh://git@github.com/org/repo", "https://github.com/org/repo.git/",
                "https://alice@GitHub.com/org/repo", "git://github.com/org/repo.git", "git+ssh://git@github.com/org/repo"]
        self.assertEqual(len({self.bead.normalize_remote(u) for u in same}), 1)
        ports = ["ssh://git@host.example/org/repo", "ssh://git@host.example:2222/org/repo",
                 "https://host.example:8443/org/repo"]
        self.assertEqual(len({self.bead.normalize_remote(u) for u in ports}), 3)


if __name__ == "__main__":
    unittest.main()
