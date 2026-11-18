#!/usr/bin/env python3
"""Random agent-like traffic against the beads POC on one machine. Logs one JSON line per command.
Usage: soak.py BP REPO SECONDS SEED LOGFILE   (BEAD_HOME etc. come from the environment)
BP is the bead to run, as its caller selected it, by its absolute path: the preflight reads it from where
this starts and the traffic runs it in REPO, so a relative path could name two beads, and it is refused
before anything runs. Before any traffic, the log's first line records it (its path, the first line
`--version` prints, its kind, native, script or missing, and a digest). A missing bead stops the soak, and
with BEAD_EXPECT_VERSION set (0.2.0 at the cutover) so does one that reports another version or is a
script: infra/selected.py's rules, repeated here because this file runs on hosts that have only it."""
import hashlib
import json
import os
import random
import subprocess
import sys
import time

bead, repo, seconds, seed, logfile = sys.argv[1], sys.argv[2], float(sys.argv[3]), int(sys.argv[4]), sys.argv[5]
if not os.path.isabs(bead):
    sys.exit(f"soak.py: BP must be an absolute path, not {bead!r}")
random.seed(seed)
words = "ledger invoice export worker retry queue schema audit login token cache index proxy budget gate loop".split()
log = open(logfile, "a")
# seconds `--version` may take, and a command of the traffic, which waits for the other machines' writes
# under contention (seconds at the median, tens at the longest): one longer is hung
PROBE_TIMEOUT = 60
TIMEOUT = 180


def preflight():
    """The bead's record, logged, as infra/selected.py's describe makes it: a bead that is not there is
    recorded `missing`, one that does not run says why. The soak stops on a missing bead, which leaves it
    nothing to run, and unless the bead is what BEAD_EXPECT_VERSION asks for."""
    try:
        with open(bead, "rb") as file:
            data = file.read()
    except OSError:
        data = None
    version = ""
    if data is not None:
        try:
            proc = subprocess.run([bead, "--version"], capture_output=True, text=True, timeout=PROBE_TIMEOUT)
            lines = (proc.stdout.strip() or proc.stderr.strip()).splitlines()
            version = lines[0] if lines else ""
        except (OSError, subprocess.TimeoutExpired) as error:
            version = f"(does not run: {error})"
    kind = "missing" if data is None else "script" if data.startswith(b"#!") else "native"
    row = {"preflight": bead, "version": version, "kind": kind,
           "sha256": "" if data is None else hashlib.sha256(data).hexdigest()[:16]}
    log.write(json.dumps(row) + "\n")
    log.flush()
    if kind == "missing":
        sys.exit(f"soak.py: {bead} does not exist")
    expect = os.environ.get("BEAD_EXPECT_VERSION", "")
    if expect and (row["version"] != f"bead {expect}" or row["kind"] != "native"):
        sys.exit(f"soak.py: {bead} is {row['kind']}, reporting {row['version']!r}, not a native 'bead {expect}'")


preflight()
mine = []  # issues this session has claimed
EXPECTED = {"ALREADY_CLAIMED", "CLAIMED_BY_OTHER", "NOT_OWNER", "CYCLE", "STATUS_CONFLICT", "VALIDATION",
            "ISSUE_NOT_FOUND", "SYNC_CONFLICT", "REMOTE_UNREACHABLE", "UNRESOLVED"}


def run(*args):
    t = time.time()
    proc = subprocess.run([bead, *args, "--json"], cwd=repo, capture_output=True, text=True, timeout=TIMEOUT)
    ms = int((time.time() - t) * 1000)
    row = {"t": round(time.time(), 3), "op": args[0], "args": args[1:4], "rc": proc.returncode, "ms": ms}
    out = None
    if proc.returncode:
        try:
            row["code"] = json.loads(proc.stderr)["error"]["code"]
        except (ValueError, KeyError, TypeError):  # not bead's JSON error: its text, kept
            row["code"] = "CRASH"
            row["err"] = proc.stderr[-400:]
    elif proc.stdout.strip():
        out = json.loads(proc.stdout)
    log.write(json.dumps(row) + "\n")
    log.flush()
    return proc.returncode == 0, out


def text(n):
    return " ".join(random.choice(words) for _ in range(n))


def pick(items):
    return random.choice(items) if items else None


deadline = time.time() + seconds
n = 0
while time.time() < deadline:
    n += 1
    _, listed = run("list", "--limit", "0")
    open_ids = [i["id"] for i in (listed or {}).get("issues", [])]
    _, epics = run("list", "--type", "epic", "--limit", "0")
    epic_ids = [i["id"] for i in (epics or {}).get("issues", [])]
    roll = random.random()
    if roll < 0.25 and open_ids:
        run("show", pick(open_ids))
    elif roll < 0.33:
        run("ready", "--type", "task")
    elif roll < 0.43 and open_ids:
        field = random.choice(["--notes", "--description", "--acceptance-criteria", "--title"])
        run("update", pick(open_ids), field, text(8))
    elif roll < 0.53 and open_ids:
        run("comments", "add", pick(open_ids), text(10))
    elif roll < 0.61:
        args = ["create", f"soak {seed} {n} {text(3)}", "-t", random.choice(["task", "bug", "feature"]), "-p", str(random.randint(0, 4))]
        if epic_ids and random.random() < 0.3:
            args += ["--parent", pick(epic_ids)]
        run(*args)
    elif roll < 0.71:
        _, ready = run("ready", "--type", "task")
        target = pick([i["id"] for i in (ready or [])])
        if target:
            ok, _ = run("claim", target)
            if ok:
                mine.append(target)
    elif roll < 0.79 and mine:
        target = mine.pop(random.randrange(len(mine)))
        run("close", target, "--reason", text(4))
    elif roll < 0.82 and mine:
        target = mine.pop(random.randrange(len(mine)))
        run("release", target)
    elif roll < 0.85 and len(open_ids) >= 2:
        a, b = random.sample(open_ids, 2)
        run("dep", "add", a, b)
    elif roll < 0.88 and open_ids:
        run("update", pick(open_ids), random.choice(["--add-label", "--remove-label"]), random.choice(["soak", "hot", "cold"]))
    else:
        run("sync")
    if n % 5 == 0:
        run("sync")
run("sync")
log.write(json.dumps({"done": True, "ops": n, "still_holding": mine}) + "\n")
log.close()
