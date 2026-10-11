#!/usr/bin/env python3
"""Drive bead across real machines through ssh: cross-machine scenarios against a throwaway repository
on a real git host. Read-only outside that repository's clones and the beads-poc-test state directories.

Configuration (environment):
  BEAD_DRIVE_HOSTS       three ssh targets for the roles one, two, three: "one=user@host1,two=host2,three=user@host3"
  BEAD_TEST_REMOTE       the throwaway repository, e.g. git@github.com:owner/throwaway.git (never a real one)
  BEAD_TEST_LOCAL_REPO   a clone of it on this machine (default ~/beads-poc-test/repo)
  BEAD_UNDER_TEST        the bead to run here, an absolute path (default: the installed ~/.local/bin/bead;
                         infra/selected.py)
  BEAD_DRIVE_BEADS       the bead to run on the remote hosts, by role, absolute paths there:
                         "one=/path/bead,three=/path/bead" (default: each host's installed ~/.local/bin/bead)
  BEAD_EXPECT_VERSION    the version every machine's bead must report, e.g. 0.2.0; each must then also be a native
                         build, never a script (Python's launcher or installed entry point)
Before any test data is written, a preflight prints one `preflight:` JSON line per machine: its role, system,
the bead selected there, the first line its `--version` prints, its kind (native, script or missing) and a
digest; a machine that does not meet BEAD_EXPECT_VERSION stops the run. `drive.py --preflight` stops after it.
Setup (once): on each remote host a bead (installed at ~/.local/bin/bead, or staged anywhere and named in
BEAD_DRIVE_BEADS) and a clone of the throwaway repository at ~/beads-poc-test/repo (plus soak.py for the soak);
here, the clone above. To reset the tracker: `git push origin :refs/beads/journal`,
`rm -rf ~/.local/state/beads-poc-test` everywhere, then `bead init --prefix bpt --import seed.jsonl` from here
and `bead sync` on the others, all with BEAD_HOME=$HOME/.local/state/beads-poc-test."""
import concurrent.futures as cf
import hashlib
import json
import os
import shlex
import subprocess
import sys
import time

import selected

TAG = time.strftime("%H%M%S")

LOCAL = os.path.expanduser("~")
HOSTS = {"local": None}  # role -> ssh target; None runs here
for pair in filter(None, os.environ.get("BEAD_DRIVE_HOSTS", "").split(",")):
    role, _, target = pair.partition("=")
    HOSTS[role.strip()] = target.strip()
TEST_REMOTE = os.environ.get("BEAD_TEST_REMOTE", "")
if set(HOSTS) != {"local", "one", "two", "three"} or not TEST_REMOTE:
    sys.exit("drive.py needs BEAD_DRIVE_HOSTS=one=user@host,two=host,three=user@host and BEAD_TEST_REMOTE (see the docstring)")


def remote_beads(environ, roles):
    """The bead each remote role runs: BEAD_DRIVE_BEADS's absolute path for it, else its installed one."""
    chosen = {}
    for pair in filter(None, environ.get("BEAD_DRIVE_BEADS", "").split(",")):
        role, _, path = (part.strip() for part in pair.partition("="))
        if role not in roles:
            sys.exit(f"BEAD_DRIVE_BEADS names {role!r}, which is not one of {', '.join(sorted(roles))}")
        if not path.startswith("/"):
            sys.exit(f"BEAD_DRIVE_BEADS gives {role} {path!r}, which is not an absolute path")
        chosen[role] = path
    return {role: chosen.get(role, selected.INSTALLED) for role in roles}


BEADS = {"local": selected.local_bead(), **remote_beads(os.environ, set(HOSTS) - {"local"})}  # role -> the bead it runs
EXPECT = os.environ.get("BEAD_EXPECT_VERSION", "")
REPO_REMOTE = "~/beads-poc-test/repo"
REPO_LOCAL = os.environ.get("BEAD_TEST_LOCAL_REPO") or os.path.expanduser("~/beads-poc-test/repo")  # a clone of the throwaway repo
STATE = "$HOME/.local/state/beads-poc-test"
# A LAN that throttles bursts of ssh connections to github.com:22 can use GitHub's SSH-over-443 endpoint instead.
HOST_ENV = {}  # plain github.com:22 everywhere


def word(path):
    """`path` as one word of a bash command: quoted, its leading `~/` left for bash to expand."""
    return "~/" + shlex.quote(path[2:]) if path.startswith("~/") else shlex.quote(path)


def over(host, cmd):
    """The command that runs bash command `cmd` on `host`: here, or through ssh."""
    return ["bash", "-c", cmd] if HOSTS[host] is None else ["ssh", "-o", "BatchMode=yes", HOSTS[host], "bash -c " + shlex.quote(cmd)]


# What a machine's bead is, printed as six lines: role-free facts the preflight records.
PROBE = ('b={bead}; '
         'if [ -e "$b" ]; then v=$("$b" --version 2>&1 | head -n 1); else v=; fi; '
         'if [ ! -e "$b" ]; then k=missing; elif [ "$(head -c 2 "$b")" = "#!" ]; then k=script; else k=native; fi; '
         'd=$( (sha256sum "$b" 2>/dev/null || shasum -a 256 "$b" 2>/dev/null) | cut -c1-16); '
         'printf "%s\\n%s\\n%s\\n%s\\n%s\\n%s\\n" "$b" "$v" "$k" "$d" "$(uname -sm)" end')


def probe(host):
    """The preflight's record of `host`'s bead (selected.describe's fields, its role and its system)."""
    proc = subprocess.run(over(host, PROBE.format(bead=word(BEADS[host]))), capture_output=True, text=True, timeout=120)
    lines = proc.stdout.split("\n")
    if proc.returncode or len(lines) < 6 or lines[5] != "end":
        sys.exit(f"preflight: cannot reach {host} ({HOSTS[host] or 'here'}): {proc.stderr.strip()[-300:]}")
    path, version, kind, digest, system = lines[:5]
    return {"role": host, "system": system, "path": path, "version": version, "kind": kind, "sha256": digest}


def preflight():
    """Before any test data is written: every machine's record, printed; the run stops unless every bead is
    what BEAD_EXPECT_VERSION asks for."""
    rows = parallel(probe, list(HOSTS))
    for row in rows:
        print("preflight:", json.dumps(row, sort_keys=True), flush=True)
    wrong = [f"{row['role']}: {why}" for row in rows if (why := selected.verdict(row, EXPECT))]
    if wrong:
        sys.exit("preflight: " + "; ".join(wrong))
    return rows


def run(host, *args, thread=None, agent="codex", env="", raw=False, timeout=120):
    """Run one bead command on `host`; return parsed JSON (or an error dict with code)."""
    thread = thread or f"{host}-thread-0001"
    repo = REPO_LOCAL if HOSTS[host] is None else REPO_REMOTE
    cmd = (f"cd {repo} && BEAD_HOME={STATE} BEAD_AGENT={agent} BEAD_THREAD={thread} {HOST_ENV.get(host, '')} {env} "
           + word(BEADS[host]) + " " + " ".join(shlex.quote(a) for a in args) + ("" if raw else " --json"))
    full = over(host, cmd)
    t = time.time()
    proc = subprocess.run(full, capture_output=True, text=True, timeout=timeout)
    if proc.returncode == 255 and HOSTS[host] is not None and not proc.stderr.strip():
        proc = subprocess.run(full, capture_output=True, text=True, timeout=timeout)  # ssh itself dropped: once more
    ms = int((time.time() - t) * 1000)
    if raw:
        return {"rc": proc.returncode, "out": proc.stdout, "err": proc.stderr, "ms": ms}
    if proc.returncode:
        try:
            return {"error": json.loads(proc.stderr)["error"]["code"], "rc": proc.returncode, "ms": ms, "err": proc.stderr[-300:]}
        except Exception:
            return {"error": "CRASH", "rc": proc.returncode, "ms": ms, "err": proc.stderr[-600:]}
    return {"ok": json.loads(proc.stdout) if proc.stdout.strip() else None, "ms": ms}


def parallel(fn, items):
    with cf.ThreadPoolExecutor(len(items)) as pool:
        return list(pool.map(fn, items))


def export_hash(host):
    r = run(host, "export", raw=True)
    return hashlib.sha256(r["out"].encode()).hexdigest()[:12], r["out"].count("\n")


def settle():
    for _ in range(2):
        for host in HOSTS:
            r = run(host, "sync")
            assert "ok" in r, (host, r)
    hashes = {h: export_hash(h) for h in HOSTS}
    same = len({v for v in hashes.values()}) == 1
    return same, hashes


def ready_ids(host):
    return [i["id"] for i in run(host, "ready", "--type", "task")["ok"]]


def show(host, key):
    return run(host, "show", key)["ok"][0]


def section(title):
    print(f"\n### {title}", flush=True)


def main(argv):
    section("preflight: the bead each machine runs")
    preflight()
    if argv == ["--preflight"]:
        return 0
    if argv:
        sys.exit(f"drive.py takes no arguments but --preflight, not {argv}")
    results = {}
    section("S1 four machines claim the same issue at the same moment")
    target = ready_ids("local")[0]
    outcomes = parallel(lambda h: (h, run(h, "claim", target)), list(HOSTS))
    winners = [h for h, r in outcomes if "ok" in r]
    print("winner:", winners, "| others:", {h: r.get("error") for h, r in outcomes if "ok" not in r},
          "| latency ms:", {h: r["ms"] for h, r in outcomes})
    same, hashes = settle()
    owners = {h: show(h, target)["claim"]["owner"] for h in HOSTS}
    print("converged:", same, "| owner seen everywhere:", len(set(owners.values())) == 1, owners["local"])
    results["S1"] = len(winners) == 1 and same and len(set(owners.values())) == 1

    section("S2 everyone edits, then everyone syncs at once")
    for h in HOSTS:
        for n in range(3):
            assert "ok" in run(h, "create", f"{h} item {n}", "-l", h)
        assert "ok" in run(h, "comments", "add", target, f"note from {h} (run {os.getpid()})")
    syncs = parallel(lambda h: (h, run(h, "sync")), list(HOSTS))
    print("sync results:", {h: (r.get("ok", {}).get("attempts") if "ok" in r else r.get("error")) for h, r in syncs},
          "| ms:", {h: r["ms"] for h, r in syncs})
    same, hashes = settle()
    comments = [c["text"] for c in show("two", target)["comments"] if f"(run {os.getpid()})" in c["text"]]  # this run's
    print("converged:", same, hashes["local"], "| comments on target:", len(comments))
    results["S2"] = same and all("ok" in r for _, r in syncs) and len(comments) == 4

    section("S3 the same field changed on two machines without syncing")
    victim = ready_ids("one")[1]
    run("local", "update", victim, "--priority", "0")
    run("one", "update", victim, "--priority", "4")
    same, _ = settle()
    views = {h: show(h, victim) for h in HOSTS}
    flagged = all("priority" in v.get("conflicts", {}) for v in views.values())
    values = sorted(o["value"] for o in views["three"]["conflicts"]["priority"])
    print("converged:", same, "| flagged everywhere:", flagged, "| alternatives:", values, "| shown:", views["three"]["priority"])
    run("local", "update", victim, "--priority", "2")  # has seen both
    same, _ = settle()
    cleared = all("conflicts" not in show(h, victim) for h in HOSTS)
    print("settled by a later write:", cleared)
    results["S3"] = same and flagged and values == [0, 4] and cleared

    section("S4 takeover fences the previous owner across machines")
    issue = ready_ids("two")[2]
    before = run("one", "claim", issue)["ok"][0]["claim"]["generation"]
    taken = run("three", "takeover", issue, "--reason", "the agent on machine one looks dead", agent="human", thread="dev-at-bpc")
    gen = taken["ok"][0]["claim"]["generation"]
    print("takeover:", "ok" in taken, "generation", before, "->", gen)
    stale_close = run("one", "close", issue, "--reason", "actually finished")  # pop has not synced
    print("old owner's close locally:", "ok" in stale_close)
    same, _ = settle()
    views = {h: show(h, issue) for h in HOSTS}
    held = all(v["status"] == "in_progress" and v["claim"]["generation"] == gen and [s["op"] for s in v.get("stale", [])] == ["close"]
               for v in views.values())
    print("converged:", same, "| everywhere: in_progress, new generation, close marked stale:", held)
    done = run("three", "close", issue, "--reason", "done", agent="human", thread="dev-at-bpc")
    same, _ = settle()
    print("new owner closes:", "ok" in done, "| closed everywhere:", all(show(h, issue)["status"] == "closed" for h in HOSTS))
    results["S4"] = same and held and "ok" in done

    section("S5 an offline claim meets an online one")
    issue = ready_ids("local")[3]
    off = run("two", "claim", issue, "--offline")
    on = run("local", "claim", issue)
    same, _ = settle()
    views = {h: show(h, issue) for h in HOSTS}
    ok = all("local-thre" in v["claim"]["owner"] and len(v.get("provisional_claims", [])) == 1 for v in views.values())
    print("offline recorded:", "ok" in off, "| online owner wins everywhere, provisional listed:", ok)
    results["S5"] = same and ok

    section("S6 lost acknowledgement over the real network")
    issue = ready_ids("one")[4]
    crashed = run("one", "claim", issue, env="BEAD_TEST_CRASH_AFTER_PUSH=1", raw=True)
    print("first attempt exit code:", crashed["rc"], "(99 = pushed, then died before recording)")
    before = show("one", issue).get("claim")
    retry = run("one", "claim", issue)
    log = run("one", "log", issue)["ok"]
    print("locally unrecorded before retry:", before is None, "| retry ok:", "ok" in retry,
          "| claim events in journal:", sum(1 for e in log if e["op"] == "claim"))
    results["S6"] = crashed["rc"] == 99 and before is None and "ok" in retry and sum(1 for e in log if e["op"] == "claim") == 1

    section("S7 two machines add a child to the same parent")
    epic = [i["id"] for i in run("local", "list", "--type", "epic")["ok"]["issues"]][0]
    a = run("three", "create", f"child from bpc {TAG}", "--parent", epic)["ok"]["id"]
    b = run("two", "create", f"child from arch {TAG}", "--parent", epic)["ok"]["id"]
    print("minted locally:", a, b)
    same, _ = settle()
    kids = {h: sorted((i["id"], i["title"]) for i in run(h, "list", "--parent", epic, "--limit", "0")["ok"]["issues"]
                      if i["title"].endswith(TAG)) for h in HOSTS}
    distinct = len({k for k, _ in kids["local"]}) == 2
    print("converged:", same, "| both children present with distinct keys everywhere:", distinct and len(set(map(str, kids.values()))) == 1, kids["local"])
    results["S7"] = same and distinct and len(set(map(str, kids.values()))) == 1

    section("S8 latency against GitHub, per machine (median of 3)")
    for h in HOSTS:
        ms = {}
        for op in (("show", target), ("ready",), ("sync",)):
            ms[op[0]] = sorted(run(h, *op)["ms"] for _ in range(3))[1]
        k = run(h, "create", f"latency probe {h}")["ok"]["id"]
        ms["claim"] = run(h, "claim", k)["ms"]
        print(f"{h:4s} {ms}  (includes ssh round trip for remote hosts)")
    results["S8"] = True

    section("S11 a linked worktree on this machine sees the same tracker and stays clean")
    wt = REPO_LOCAL + "-wt"
    subprocess.run(["git", "-C", REPO_LOCAL, "worktree", "add", "-q", wt, "-b", f"feature-{TAG}"], check=True, capture_output=True)
    try:
        cmd = f"cd {wt} && BEAD_HOME={STATE} {word(BEADS['local'])} info --json"
        info = json.loads(subprocess.run(["bash", "-c", cmd], capture_output=True, text=True, check=True).stdout)
        status = subprocess.run(["git", "-C", wt, "status", "--porcelain"], capture_output=True, text=True).stdout
        print("same workspace:", info["workspace"] == run("local", "info")["ok"]["workspace"], "| worktree clean:", status == "")
        results["S11"] = info["workspace"] == run("local", "info")["ok"]["workspace"] and status == ""

        section("S12 a claim made in the worktree carries its branch to every machine")
        issue = ready_ids("local")[5]
        cmd = f"cd {wt} && BEAD_HOME={STATE} BEAD_AGENT=codex BEAD_THREAD=local-wt-thread {word(BEADS['local'])} update {shlex.quote(issue)} --claim --add-label from-worktree --json"
        claimed = subprocess.run(["bash", "-c", cmd], capture_output=True, text=True)
        same, _ = settle()
        views = {h: show(h, issue) for h in HOSTS}
        branches = {h: (v.get("claim") or {}).get("where", {}).get("branch") for h, v in views.items()}
        labels = {h: sorted(v.get("labels", [])) for h, v in views.items()}
        rows = [r for r in run("two", "claims")["ok"]["claims"] if r["id"] == issue]
        print("claimed from worktree:", claimed.returncode == 0, "| converged:", same, "| branch seen:", branches,
              "| label travelled with the claim:", all("from-worktree" in l for l in labels.values()),
              "| `bead claims` on arch shows branch:", rows and rows[0].get("branch"))
        results["S12"] = (claimed.returncode == 0 and same and all(b == f"feature-{TAG}" for b in branches.values())
                          and all("from-worktree" in l for l in labels.values()) and rows and rows[0].get("branch") == f"feature-{TAG}")
    finally:
        subprocess.run(["git", "-C", REPO_LOCAL, "worktree", "remove", "--force", wt], capture_output=True)

    print("\n### summary:", {k: ("PASS" if v else "FAIL") for k, v in results.items()})
    return 0 if all(results.values()) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
