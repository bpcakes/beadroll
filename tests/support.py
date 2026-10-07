"""Shared test support: the bead under test, a Sandbox (one local bare remote plus a code
repository) and Machines (each with its own home, clone and identity). Nothing outside the
sandbox is read or written."""
import concurrent.futures
import importlib.machinery
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

BEAD = os.environ.get("BEAD_UNDER_TEST") or str(Path(__file__).resolve().parent.parent / "bead")


def load_module():
    loader = importlib.machinery.SourceFileLoader("bp_module", BEAD)
    module = importlib.util.module_from_spec(importlib.util.spec_from_loader("bp_module", loader))
    loader.exec_module(module)
    return module


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
        self.root = Path(tempfile.mkdtemp(prefix="bp-test-")).resolve()
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
