"""Shared test support: the bead under test, a Sandbox (one local bare remote plus a code
repository) and Machines (each with its own home, clone and identity). Nothing outside the
sandbox is read or written."""
import concurrent.futures
import importlib.machinery
import sys
import contextlib
import importlib.util
import itertools
import json
import os
import shutil
import subprocess
import tempfile
import time
import unittest
import uuid
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
BEAD = os.environ.get("BEAD_UNDER_TEST") or str(ROOT / "bead")  # the launcher: runs the checkout's package


def load_module():
    """The beadroll package: one namespace over its modules (`beadroll/__init__.py`). Nothing in it mutates
    module state, so the cached import serves every test; to replace a name, use `patched` below."""
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    return importlib.import_module("beadroll")


_MISSING = object()


@contextlib.contextmanager
def patched(name, value=_MISSING):
    """Replace a beadroll name in every module that holds it, for the duration of the block. The package
    imports names (`from .base import run_git`), so patching the defining module alone would not reach its
    users. Without a value, a `mock.MagicMock`."""
    load_module()
    value = mock.MagicMock() if value is _MISSING else value
    targets = [m for n, m in list(sys.modules.items()) if n == "beadroll" or n.startswith("beadroll.")]
    saved = [(m, getattr(m, name)) for m in targets if hasattr(m, name)]
    if not saved:
        raise AttributeError(f"no beadroll module holds {name!r}")
    for module, _ in saved:
        setattr(module, name, value)
    try:
        yield value
    finally:
        for module, old in saved:
            setattr(module, name, old)


def git(*args, cwd=None):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


def code(proc):
    """The error code a failed `bead ... --json` call reported."""
    return json.loads(proc.stderr)["error"]["code"]


class Machine:
    """One developer machine: its own state directory and its own clones."""

    def __init__(self, sandbox, name):
        self.sandbox, self.name = sandbox, name
        self.home = sandbox.root / f"home-{name}"
        self.checkout = sandbox.clone(f"{name}-main")
        self.extra = {}

    def env(self, thread=None, agent="codex", **more):
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("CLAUDE", "CODEX", "BEAD_", "GIT_"))}
        env.update(BEAD_HOME=str(self.home), BEAD_AGENT=agent, BEAD_THREAD=thread or f"{self.name}-thread-0001")
        env.update(self.extra)
        env.update(more)
        return env

    def run(self, *args, cwd=None, thread=None, agent="codex", **more):
        return subprocess.run([BEAD, *args], cwd=cwd or self.checkout, env=self.env(thread, agent, **more),
                              capture_output=True, text=True)

    def bead(self, *args, cwd=None, thread=None, agent="codex"):
        proc = self.run(*args, "--json", cwd=cwd, thread=thread, agent=agent)
        if proc.returncode:
            raise AssertionError(f"bead {' '.join(args)} failed: {proc.stderr}")
        return json.loads(proc.stdout) if proc.stdout.strip() else None

    def export(self):
        proc = self.run("export")
        assert proc.returncode == 0, proc.stderr
        return proc.stdout

    def show(self, key):
        return self.bead("show", key)[0]

    def ready(self):
        return {i["id"] for i in self.bead("ready")}

    def store(self):
        return next(p for p in self.home.iterdir() if p.is_dir())


class Sandbox:
    def __init__(self):
        self.root = Path(tempfile.mkdtemp(prefix="bead-test-")).resolve()
        self.origin = self.bare("origin.git")
        seed = self.clone("seed")
        (seed / "README.md").write_text("example project\n")
        git("add", "-A", cwd=seed)
        git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "init", cwd=seed)
        git("push", "-q", "origin", "HEAD:main", cwd=seed)

    def bare(self, name):
        path = self.root / name
        git("init", "-q", "--bare", "-b", "main", str(path))
        return path

    def clone(self, name, origin=None):
        path = self.root / name
        git("clone", "-q", str(origin or self.origin), str(path))
        return path

    def cleanup(self):
        subprocess.run(["chmod", "-R", "u+w", str(self.root)], check=False)
        shutil.rmtree(self.root, ignore_errors=True)


load_bead = load_module  # the third review's name for it
