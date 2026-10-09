"""The ownership protocol, enumerated (seventh independent review, 2026-10-07): three machines, nine
operations at seven fault boundaries, every ordered pair of four follow-ups; after each step at most one
confirmed owner, generations monotonic, identical exports after settling, and every command's answer
consistent with the remote. The default is a twelve-schedule slice touching every operation and boundary,
including retries of an owner's close/delete interrupted before publishing. BEAD_SWEEP=full runs all
660; BEAD_SWEEP_GC=1 and BEAD_SWEEP_CONCURRENT=1 select variants.
Run through `./check.sh --stress`."""
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

# ------------------------------------------------------------------------------------ the sweep

A1, A2, B1, C1 = "m1-a1", "m1-a2", "m2-b1", "m3-c1"  # threads: two actors on m1, one each on m2, m3

OPS = {  # name: (precondition, argv builder); every op runs as A1 on m1
    "claim": ("free", lambda k: ["claim", k["X"]]),
    "release": ("a1holds", lambda k: ["release", k["X"]]),
    "takeover": ("b1holds", lambda k: ["takeover", k["X"], "--reason", "sweep"]),
    "close": ("a1holds", lambda k: ["close", k["X"], "--reason", "done"]),
    "delete": ("a1holds", lambda k: ["delete", k["X"], "--reason", "gone"]),
    "update-edits": ("free", lambda k: ["update", k["X"], "--claim", "--title", "edited", "--add-label", "swept"]),
    "update-parent": ("free", lambda k: ["update", k["X"], "--claim", "--parent", k["P"]]),
    "update-closed": ("free", lambda k: ["update", k["X"], "--claim", "--status", "closed"]),
    "sync": ("sync", lambda k: ["sync"]),
}
COORDINATED = {"claim", "release", "takeover", "update-edits", "update-parent", "update-closed"}
FAULTS = ["crash-before-push", "crash-after-push", "timeout-fenced", "timeout-landed",
          "kill-confirm", "kill-write-pending", "kill-write-confirm"]
FOLLOW = {  # R retry, D different command same actor, O other actor same machine, S sync elsewhere
    "R": "retry of the same command by A1 on m1",
    "D": "`claim Z` by A1 on m1",
    "O": "`claim X` by A2 on m1",
    "S": "`sync` on m2",
}
PAIRS = ["".join(p) for p in itertools.permutations("RDOS", 2)]


def applicable(op, fault):
    """Which boundaries an operation passes through. close, delete and sync publish through
    `synchronise`, which has no crash hook: the push boundary is reached with a wrapper instead.
    They never write or confirm an attempt of their own; a sync copies other machines' attempts,
    so a kill during that write applies to it."""
    if fault in ("kill-confirm", "kill-write-pending"):
        return op in COORDINATED
    if fault == "kill-write-confirm":
        return op in COORDINATED or op == "sync"
    return True


def schedules():
    return [(op, fault, pair) for op in OPS for fault in FAULTS if applicable(op, fault) for pair in PAIRS]


# Faults inside one bead process, injected by wrapping module globals (bead itself is unchanged).
WRAPPER = r'''
import os, signal, sys, runpy
from pathlib import Path
bead = sys.argv[1]
ns = runpy.run_path(bead)  # the launcher imports the package
mods = [m for n, m in list(sys.modules.items()) if n.startswith("beadroll")]
class Everywhere(dict):  # the globals of every beadroll module: read from the first holder, written to all
    def __getitem__(self, key):
        return next(getattr(m, key) for m in mods if hasattr(m, key))
    def __setitem__(self, key, value):
        for m in mods:
            if hasattr(m, key):
                setattr(m, key, value)
g = Everywhere()
fault = os.environ.pop("BEAD_SWEEP_FAULT", "")
fired = []
if fault in ("push-before", "push-after"):
    real_run_git = g["run_git"]
    def run_git(args, **kw):
        if args and args[0] == "push" and not fired:
            fired.append(1)
            if fault == "push-before":
                os._exit(98)
            proc = real_run_git(args, **kw)
            if proc.returncode == 0:
                os._exit(99)
            return proc
        return real_run_git(args, **kw)
    g["run_git"] = run_git
elif fault == "kill-confirm":
    real_confirm = g["confirm_ownership"]
    def confirm_ownership(ws, events):
        real_confirm(ws, events)
        if events:
            os.kill(os.getpid(), signal.SIGKILL)  # confirmed, not yet dropped from pending
    g["confirm_ownership"] = confirm_ownership
elif fault in ("kill-write-pending", "kill-write-confirm"):
    folder = "pending" if fault == "kill-write-pending" else "claims"
    real_write = g["write_atomic"]
    def write_atomic(path, data, private=True):
        path = Path(path)
        if path.parent.name == folder and path.suffix == ".json":
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
            with open(tmp, "wb") as fh:  # half the bytes under the temporary name, as a kill mid-write leaves them
                fh.write(data[: max(1, len(data) // 2)])
                fh.flush()
                os.fsync(fh.fileno())
            os.kill(os.getpid(), signal.SIGKILL)
        return real_write(path, data, private)
    g["write_atomic"] = write_atomic
raise SystemExit(g["main"](sys.argv[2:]))
'''

STALL_FENCED = """#!/bin/sh
# The first push stalls past the client's timeout, then lets receive-pack try its update; every
# later push (the fence) goes straight through, so the fence lands first and the stalled one loses.
if mkdir {root}/hook-first 2>/dev/null; then
  touch {root}/hook-started; sleep {stall}; touch {root}/hook-finished
fi
exit 0
"""
STALL_LANDED = """#!/bin/sh
# The first push stalls past the client's timeout and then lands; every later push (the fence)
# waits behind it, so the first one wins and the fence's ref update fails.
if mkdir {root}/hook-first 2>/dev/null; then
  touch {root}/hook-started; sleep {stall}; touch {root}/hook-finished; exit 0
fi
n=0; while [ ! -f {root}/hook-finished ] && [ $n -lt 400 ]; do sleep 0.05; n=$((n+1)); done
sleep 0.3
exit 0
"""
NET_TIMEOUT, STALL = "1.0", "3.5"
UNCERTAIN = {"UNRESOLVED", "REMOTE_UNREACHABLE", "SYNC_CONFLICT"}


class Run:
    """One schedule: a sandbox, three machines and an observer that only reads the remote."""

    def __init__(self, op, fault, pair, gc=False, concurrent=False):
        self.op, self.fault, self.pair, self.gc, self.concurrent = op, fault, pair, gc, concurrent
        self.box = support.Sandbox()
        if gc:  # the remote collects garbage after every push it accepts, synchronously
            hook = self.box.origin / "hooks" / "post-receive"
            hook.write_text("#!/bin/sh\ngit gc --prune=now --quiet >/dev/null 2>&1\nexit 0\n")
            hook.chmod(0o755)
        self.m1, self.m2, self.m3 = (support.Machine(self.box, n) for n in ("m1", "m2", "m3"))
        self.obs = support.Machine(self.box, "obs")
        self.steps, self.violations, self.generations = [], [], {"m1": [], "m2": [], "m3": []}
        self.forks = []
        self.wrapper = self.box.root / "fault.py"
        self.wrapper.write_text(WRAPPER)

    # -- running commands
    def call(self, machine, argv, thread, fault_env=None, wrapper_fault=None, label=None):
        env = machine.env(thread=thread, **(fault_env or {}))
        cmd = [support.BEAD, *argv, "--json"]
        if wrapper_fault:
            env["BEAD_SWEEP_FAULT"] = wrapper_fault
            cmd = [sys.executable, str(self.wrapper), support.BEAD, *argv, "--json"]
        started = time.monotonic()
        try:
            proc = subprocess.run(cmd, cwd=machine.checkout, env=env, capture_output=True, text=True, timeout=180)
            rc, out, err = proc.returncode, proc.stdout, proc.stderr
        except subprocess.TimeoutExpired:
            rc, out, err = "HANG", "", "no exit within 180 s"
        try:
            parsed = json.loads(out) if rc == 0 and out.strip() else None
        except ValueError:
            parsed = None
        try:
            code = json.loads(err)["error"]["code"] if rc not in (0, "HANG") and err.strip() else None
        except (ValueError, KeyError, TypeError):
            code = "TRACEBACK" if "Traceback" in err else None
        step = {"step": label or argv[0], "machine": machine.name, "thread": thread, "argv": argv, "rc": rc,
                "code": code, "ms": int((time.monotonic() - started) * 1000), "out": parsed, "raw": out,
                "err": err[-400:]}
        self.steps.append(step)
        if rc == "HANG" or code == "TRACEBACK":
            self.violations.append(f"{step['step']}: {rc} {err[-300:]}")
        if argv[0] == "sync" and isinstance(parsed, dict) and parsed.get("forked_journals"):
            self.forks.append((step["step"], parsed["forked_journals"]))
        return step

    def ok(self, machine, *argv, thread):
        step = self.call(machine, list(argv), thread, label="setup")
        if step["rc"] != 0:
            raise AssertionError(f"setup {argv} failed: {step['err']}")
        return step["out"]

    # -- setup
    def prepare(self):
        m1, m2, m3 = self.m1, self.m2, self.m3
        self.ok(m1, "init", "--prefix", "sw", thread=A1)
        keys = {"Q": self.ok(m1, "create", "old parent q", thread=A1)["id"]}
        keys["X"] = self.ok(m1, "create", "target x", "--parent", keys["Q"], thread=A1)["id"]
        for name in "YZP":
            keys[name] = self.ok(m1, "create", f"issue {name.lower()}", thread=A1)["id"]
        for machine, thread in ((m1, A1), (m2, B1), (m3, C1)):
            self.ok(machine, "sync", thread=thread)
        self.keys = keys
        self.replica = {m.name: self.ok(m, "info", thread=A1)["machine"] for m in (m1, m2, m3)}
        pre = OPS[self.op][0]
        if pre == "a1holds":
            self.ok(m1, "claim", keys["X"], thread=A1)
            self.ok(m2, "sync", thread=B1), self.ok(m3, "sync", thread=C1)
        elif pre == "b1holds":
            self.ok(m2, "claim", keys["X"], thread=B1)
            self.ok(m1, "sync", thread=A1), self.ok(m3, "sync", thread=C1)
        elif pre == "sync":  # local work to publish, and another machine's attempt to copy in
            self.ok(m1, "comments", "add", keys["X"], "local work", thread=A1)
            self.ok(m2, "claim", keys["Y"], thread=B1)

    def label(self, machine_name, thread):
        return f"codex:{thread}@{self.replica[machine_name]}"

    # -- faults
    def fault_args(self):
        fault, coordinated = self.fault, self.op in COORDINATED
        if fault == "crash-before-push":
            return ({"BEAD_TEST_CRASH_BEFORE_PUSH": "1"}, None) if coordinated else (None, "push-before")
        if fault == "crash-after-push":
            return ({"BEAD_TEST_CRASH_AFTER_PUSH": "1"}, None) if coordinated else (None, "push-after")
        if fault.startswith("timeout"):
            hook = self.box.origin / "hooks" / "pre-receive"
            template = STALL_FENCED if fault == "timeout-fenced" else STALL_LANDED
            hook.write_text(template.format(root=shlex.quote(str(self.box.root)), stall=STALL))
            hook.chmod(0o755)
            return {"BEAD_NET_TIMEOUT": NET_TIMEOUT}, None
        return None, fault

    def quiesce(self):
        """Let a stalled receive-pack finish (landing or failing), then take the hook away."""
        hook = self.box.origin / "hooks" / "pre-receive"
        if not hook.exists():
            return
        deadline = time.monotonic() + 20
        if (self.box.root / "hook-started").exists():
            while not (self.box.root / "hook-finished").exists() and time.monotonic() < deadline:
                time.sleep(0.05)
        while time.monotonic() < deadline:
            busy = subprocess.run(["pgrep", "-f", f"receive-pack.*{self.box.origin}"], capture_output=True, text=True)
            if not busy.stdout.strip():
                break
            time.sleep(0.05)
        else:
            self.violations.append("a receive-pack was still running 20 s after the stalled push")
        hook.unlink()

    # -- observation
    def observe(self, after):
        """Local views (no network) on every machine, then the remote as a fresh reader sees it."""
        X = self.keys["X"]
        views = {}
        for machine, thread in ((self.m1, A1), (self.m2, B1), (self.m3, C1)):
            step = self.call(machine, ["show", X], thread, label=f"view after {after}")
            views[machine.name] = (step["out"] or [{}])[0]
        remote = self.remote()
        accepted = self.accepted_ids()
        for name, view in views.items():
            claim = view.get("claim")
            if claim:
                if claim["id"] not in accepted:
                    self.violations.append(f"after {after}: {name} counts claim {claim['id']} the remote never accepted")
                history = self.generations[name]
                if history and claim["generation"] < history[-1]:
                    self.violations.append(f"after {after}: {name} generation went back {history[-1]} -> {claim['generation']}")
                history.append(claim["generation"])
            if view.get("lost_claims"):
                self.violations.append(f"after {after}: {name} shows lost_claims {view['lost_claims']}")
        return remote

    def remote(self):
        step = self.call(self.obs, ["sync"], "obs-thread", label="observer sync")
        if step["rc"] != 0:
            self.violations.append(f"observer could not read the remote: {step['err']}")
            return {}
        shown = self.call(self.obs, ["show", *(self.keys[k] for k in "XYZ")], "obs-thread", label="observer show")
        return {k: v for k, v in zip("XYZ", shown["out"] or [])}

    def accepted_ids(self):
        ids = set()
        for path in self.obs.store().glob("events/*/claims/*.json"):
            for line in path.read_text().splitlines():
                if line.strip():
                    ids.add(json.loads(line)["id"])
        return ids

    def judge(self, step, remote, who, earlier=None):
        """The command's answer must match what the remote holds right after it. A refusal may
        also rest on the acting machine's own unpublished writes, which it legitimately sees."""
        rc, code, argv = step["rc"], step["code"], step["argv"]
        if rc in (98, 99, -9) or code in UNCERTAIN or not remote:
            return
        me = self.label(*who)
        verb = argv[0]
        key = "X" if self.keys["X"] in argv else "Z" if self.keys["Z"] in argv else None
        machine = step_machine(self, step)
        local = {}
        if key:
            shown = self.call(machine, ["show", self.keys[key]], step["thread"], label="view of actor")["out"]
            local = (shown or [{}])[0]
        held = (remote.get(key) or {}).get("claim") if key else None
        status = (remote.get(key) or {}).get("status") if key else None
        if earlier is not None:  # run beside another command: the answer may rest on the remote before it
            before = earlier["remote"].get(key) or {} if key else {}
            if before.get("claim") and before["claim"]["owner"] != me and not (held and held["owner"] != me):
                local = {**local, "claim": local.get("claim") or before["claim"]}
            if before.get("status") in ("closed", "tombstone"):
                local = {**local, "status": before["status"]}
        bad = lambda why: self.violations.append(f"{step['step']} {' '.join(argv)} -> rc {rc} {code or ''}: {why}")
        claims = verb in ("claim", "takeover") or (verb == "update" and "--claim" in argv)
        if claims and rc == 0:
            if "--status" in argv and "closed" in argv:
                if status != "closed":
                    bad(f"reported done but the remote has status {status}")
            elif not held or held["owner"] != me:
                bad(f"reported claimed but the remote owner is {held and held['owner']}")
            if "--title" in argv and remote[key]["title"] != "edited":
                bad("reported done but the remote lacks the bundled title")
            if "--parent" in argv and remote[key].get("parent") != self.keys["P"]:
                bad(f"reported done but the remote parent is {remote[key].get('parent')}")
        elif claims and code == "ALREADY_CLAIMED":
            if (not held or held["owner"] == me) and not (local.get("claim") and local["claim"]["owner"] != me):
                bad(f"refused as already claimed but the remote owner is {held and held['owner']}")
        elif claims and code == "VALIDATION":
            if status not in ("closed", "tombstone") and local.get("status") not in ("closed", "tombstone"):
                bad(f"refused as done but the remote status is {status}")
        elif verb == "release" and (rc == 0 or code == "NOT_OWNER"):
            if held and held["owner"] == me:
                bad("the remote still has this actor as owner")
        elif verb in ("close", "delete") and rc == 0:
            want = "closed" if verb == "close" else "tombstone"
            if status != want:
                self.violations.append(f"UNPUBLISHED {step['step']} {' '.join(argv)} -> rc 0 (json carries no warning): "
                                       f"remote status {status}, local status {local.get('status')}")
        elif verb == "sync" and rc == 0:
            mine = self.call(machine, ["export"], step["thread"], label="export after sync")["raw"]
            theirs = self.call(self.obs, ["export"], "obs-thread", label="observer export")["raw"]
            if mine != theirs and not (earlier is not None and mine == earlier["export"]):
                bad("sync reported success but this machine's export differs from the remote's")
        elif rc != 0 and code not in ("NOT_OWNER", "ALREADY_CLAIMED", "CLAIMED_BY_OTHER", "VALIDATION"):
            bad("unexpected failure")

    # -- the schedule
    def follow(self, letter, op_argv):
        X, Z = self.keys["X"], self.keys["Z"]
        if letter == "R":
            return self.call(self.m1, op_argv, A1, label="R"), ("m1", A1)
        if letter == "D":
            return self.call(self.m1, ["claim", Z], A1, label="D"), ("m1", A1)
        if letter == "O":
            return self.call(self.m1, ["claim", X], A2, label="O"), ("m1", A2)
        return self.call(self.m2, ["sync"], B1, label="S"), ("m2", B1)

    def execute(self):
        self.prepare()
        op_argv = OPS[self.op][1](self.keys)
        fault_env, wrapper_fault = self.fault_args()
        first = self.call(self.m1, op_argv, A1, fault_env=fault_env, wrapper_fault=wrapper_fault, label="op")
        self.quiesce()
        before = self.observe("op")
        self.judge(first, before, ("m1", A1))
        if self.concurrent:  # both follow-ups at once: two processes, or a process and another machine
            earlier = {"remote": before, "export": self.call(self.obs, ["export"], "obs-thread", label="observer export")["raw"]}
            with concurrent.futures.ThreadPoolExecutor(2) as pool:
                done = list(pool.map(lambda letter: self.follow(letter, op_argv), self.pair))
            remote = self.observe("+".join(self.pair))
            for step, who in done:
                self.judge(step, remote, who, earlier)
        else:
            for letter in self.pair:
                step, who = self.follow(letter, op_argv)
                self.judge(step, self.observe(letter), who)
        self.settle()

    def settle(self):
        for _ in range(2):
            for machine, thread in ((self.m1, A1), (self.m2, B1), (self.m3, C1)):
                step = self.call(machine, ["sync"], thread, label="settle")
                if step["rc"] != 0:
                    self.violations.append(f"settling sync on {machine.name} failed: {step['code']} {step['err'][-200:]}")
        self.observe("settle")
        keys = [self.keys[k] for k in "QXYZP"]
        exports, shows, claims = {}, {}, {}
        for machine, thread in ((self.m1, A1), (self.m2, B1), (self.m3, C1)):
            exports[machine.name] = machine.export()
            shows[machine.name] = self.call(machine, ["show", *keys], thread, label="final show")["out"]
            listed = self.call(machine, ["claims"], thread, label="final claims")["out"] or {}
            claims[machine.name] = [{k: v for k, v in row.items() if k not in ("idle_minutes", "machine")}
                                    for row in listed.get("claims", [])]
            info = self.call(machine, ["info"], thread, label="final info")["out"] or {}
            if info.get("integrity") or info.get("waiting_events") or info.get("pending_ownership"):
                self.violations.append(f"{machine.name} after settling: integrity {info.get('integrity')}, "
                                       f"waiting {info.get('waiting_events')}, pending {info.get('pending_ownership')}")
        for name in ("m2", "m3"):
            if exports[name] != exports["m1"]:
                self.violations.append(f"export differs between m1 and {name}")
            if shows[name] != shows["m1"]:
                self.violations.append(f"show differs between m1 and {name}")
            if claims[name] != claims["m1"]:
                self.violations.append(f"claims differ between m1 and {name}")
        if self.forks:
            self.violations.append(f"forked_journals reported: {self.forks}")
        stray = subprocess.run(["ps", "-eo", "pid=,stat=,args="], capture_output=True, text=True).stdout
        left = [row for row in stray.splitlines() if str(self.box.root) in row and "ps -eo" not in row]
        if left:
            self.violations.append(f"processes left behind: {left}")
        self.final = shows["m1"]

    def report(self):
        return {"op": self.op, "fault": self.fault, "pair": self.pair, "gc": self.gc, "concurrent": self.concurrent,
                "ok": not self.violations,
                "violations": self.violations,
                "steps": [{k: s[k] for k in ("step", "machine", "thread", "argv", "rc", "code", "ms")}
                          for s in self.steps if s["step"] not in ("setup",) and not s["step"].startswith(("view", "observer"))]}


def step_machine(run, step):
    return {"m1": run.m1, "m2": run.m2, "m3": run.m3}[step["machine"]]


def run_schedule(op, fault, pair, gc=False, concurrent=False):
    run = Run(op, fault, pair, gc=gc, concurrent=concurrent)
    try:
        run.execute()
    except Exception as error:  # a schedule that cannot even be driven is a result too
        run.violations.append(f"driver error: {error!r}")
    finally:
        run.box.cleanup()
    return run.report()


# The slice check.sh can afford: every operation and boundary, with pairs that mix a retry
# with another actor or machine, including the owner's interrupted close/delete (G12).
SLICE = [("claim", "crash-after-push", "RO"), ("release", "crash-after-push", "OR"),
         ("takeover", "timeout-landed", "SR"), ("close", "crash-before-push", "DR"),
         ("delete", "crash-after-push", "SO"), ("update-edits", "kill-confirm", "RO"),
         ("update-parent", "kill-write-pending", "DR"), ("update-closed", "timeout-fenced", "OR"),
         ("sync", "kill-write-confirm", "SR"), ("claim", "timeout-landed", "OS"),
         ("close", "crash-before-push", "RS"), ("delete", "crash-before-push", "OR")]


class Sweep(unittest.TestCase):
    """Verified property (sweep): for every schedule, at most one confirmed owner, generations
    monotonic, every confirmed claim one the remote accepted, identical exports, `show` and
    `claims` everywhere after settling, no waiting events, integrity entries, pending attempts or
    forked journals, and each command's answer consistent with what the remote holds. Measured by the
    review: 660 schedules on each of three hosts, plus 660 with `git gc --prune=now` on the remote after
    every push and 330 + 330 with the two follow-ups run concurrently; the only failures were the ten
    G12 schedules, fixed since that review. BEAD_SWEEP=full runs all 660 (BEAD_SWEEP_GC=1 and
    BEAD_SWEEP_CONCURRENT=1 select the variants). Every selected schedule must pass."""

    def test_sweep(self):
        full = os.environ.get("BEAD_SWEEP") == "full"
        concurrent_mode = os.environ.get("BEAD_SWEEP_CONCURRENT") == "1"
        chosen = schedules() if full else SLICE
        if concurrent_mode:
            chosen = [s for s in chosen if s[2][0] < s[2][1]]
        failures = []
        for op, fault, pair in chosen:
            result = run_schedule(op, fault, pair, gc=os.environ.get("BEAD_SWEEP_GC") == "1",
                                  concurrent=concurrent_mode)
            if not result["ok"]:
                failures.append(result)
        self.assertEqual([], [(f["op"], f["fault"], f["pair"], f["violations"]) for f in failures])



# ---------------------------------------------------------------------- findings G01-G65
# One class (or more) per finding; each fails on e9eed92 because of the bug and passes once the
# finding's proposed diff is applied. Ranked order is in the report; here they follow the ids.


