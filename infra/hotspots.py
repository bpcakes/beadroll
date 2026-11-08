#!/usr/bin/env python3
"""Compare two bead binaries on a fixed local journal snapshot without touching its tracker.

The source must be a local bare repository, read only by git fetch. Every bead invocation
uses a private temporary home, checkout and mirror with GIT_ALLOW_PROTOCOL=file. The mirror
rejects pushes. No tracker payload is written into the JSON report, only hashes and timings.
"""

import argparse
import concurrent.futures
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import re
import statistics
import subprocess
import tempfile
import time


def digest(data):
    return hashlib.sha256(data).hexdigest()


def stats(values):
    ordered = sorted(values)
    return {"n": len(values), "median_ms": statistics.median(values),
            "p95_ms": ordered[math.ceil(.95 * len(ordered)) - 1], "samples_ms": values}


def execute(args, cwd, env):
    started = time.perf_counter_ns()
    process = subprocess.run(args, cwd=cwd, env=env, capture_output=True, timeout=120)
    elapsed = (time.perf_counter_ns() - started) / 1e6
    if process.returncode:
        raise RuntimeError(f"{args[0]} {args[1]} failed ({process.returncode}): "
                           f"{process.stderr.decode(errors='replace')}")
    return process.stdout, elapsed


def measure(args):
    source = args.snapshot.resolve(strict=True)
    if not source.is_dir():
        raise ValueError("--snapshot must be a local bare repository directory")
    binaries = {name: path.resolve(strict=True) for name, path in
                (("before", args.before), ("after", args.after))}
    env = {k: v for k, v in os.environ.items() if not k.startswith(("BEAD_", "GIT_"))}
    env.update(GIT_ALLOW_PROTOCOL="file", NO_COLOR="1", BEAD_NET_TIMEOUT="60")
    if execute(["git", "--git-dir=" + str(source), "rev-parse", "--is-bare-repository"],
               source, env)[0].strip() != b"true":
        raise ValueError("--snapshot must be a local bare repository")
    with tempfile.TemporaryDirectory(prefix="bead-hotspots-") as temporary:
        root = Path(temporary)
        mirror, checkout = root / "remote.git", root / "checkout"
        execute(["git", "init", "--bare", "--quiet", str(mirror)], root, env)
        execute(["git", "--git-dir=" + str(mirror), "-c", "gc.auto=0", "fetch", "--quiet",
                 str(source), args.ref + ":refs/beads/journal"], root, env)
        tip = execute(["git", "--git-dir=" + str(mirror), "rev-parse", "refs/beads/journal"], root, env)[0]
        manifest = json.loads(execute(["git", "--git-dir=" + str(mirror), "show",
                                      "refs/beads/journal:manifest.json"], root, env)[0])
        workspace = manifest["workspace"]
        if not isinstance(workspace, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", workspace):
            raise ValueError("snapshot has no usable workspace id")
        hook = mirror / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o700)
        execute(["git", "init", "--quiet", str(checkout)], root, env)
        execute(["git", "remote", "add", "origin", str(mirror)], checkout, env)
        env.update(BEAD_WORKSPACE=workspace, BEAD_REMOTE=str(mirror))
        report = {"snapshot": tip.decode().strip(), "method": {
            "warm_runs": args.runs, "fresh_sync_runs": args.sync_runs,
            "local_mirror": True, "os_cache_flushed": False,
            "history_invalidated_by_every_journal_change": True},
            "host": {"system": platform.system(), "release": platform.release(),
                     "machine": platform.machine(), "load_start": os.getloadavg()},
            "binaries": {}, "results": {}}
        baseline = {}
        commands = {"ready": ["ready", "--json"],
                    "log_one": ["log", "--limit", "1", "--json"],
                    "log_default": ["log", "--json"],
                    "log_all": ["log", "--limit", "0", "--json"],
                    "export": ["export"]}
        for name, binary in binaries.items():
            report["binaries"][name] = {"sha256": digest(binary.read_bytes()),
                "version": execute([str(binary), "--version"], checkout, env)[0].decode().strip()}
            report["results"][name] = {"warm": {key: [] for key in commands},
                                       "cold_history": [], "first_sync": []}
            home = root / (name + "-warm")
            run_env = dict(env, BEAD_HOME=str(home))
            execute([str(binary), "sync", "--json"], checkout, run_env)
            info = json.loads(execute([str(binary), "info", "--json"], checkout, run_env)[0])
            assert not info["integrity"] and not info["waiting_events"]
            report["results"][name]["shape"] = {key: info[key] for key in ("events", "issues", "segments")}
            # Both processes start warm; the new history cache is made before timing hits.
            for key, command in commands.items():
                output, _ = execute([str(binary), *command], checkout, run_env)
                checksum = digest(output)
                if key in baseline:
                    assert checksum == baseline[key], (name, key, "output changed")
                else:
                    baseline[key] = checksum
        rng = random.Random(3135)
        for _ in range(args.runs):
            names = list(binaries)
            rng.shuffle(names)
            for name in names:
                run_env = dict(env, BEAD_HOME=str(root / (name + "-warm")))
                for key, command in commands.items():
                    output, ms = execute([str(binaries[name]), *command], checkout, run_env)
                    assert digest(output) == baseline[key], (name, key, "output changed")
                    report["results"][name]["warm"][key].append(ms)
        for _ in range(args.runs):
            names = list(binaries)
            rng.shuffle(names)
            for name in names:
                home = root / (name + "-warm")
                (home / workspace / "history").unlink(missing_ok=True)
                output, ms = execute([str(binaries[name]), *commands["log_one"]], checkout,
                                     dict(env, BEAD_HOME=str(home)))
                assert digest(output) == baseline["log_one"], (name, "cold history differs")
                report["results"][name]["cold_history"].append(ms)
        # Readers may all populate an absent cache; their outputs must still be identical.
        home = root / "after-warm"
        (home / workspace / "history").unlink(missing_ok=True)
        def reader(_):
            output, _ = execute([str(binaries["after"]), *commands["log_all"]], checkout,
                                dict(env, BEAD_HOME=str(home)))
            return digest(output)
        with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
            hashes = list(pool.map(reader, range(64)))
        assert all(value == baseline["log_all"] for value in hashes)
        report["concurrent_history"] = {"readers": 16, "commands": 64,
                                         "cache_absent_at_start": True, "all_outputs_identical": True}
        for trial in range(args.sync_runs):
            names = list(binaries)
            rng.shuffle(names)
            for name in names:
                run_env = dict(env, BEAD_HOME=str(root / f"{name}-fresh-{trial}"))
                output, ms = execute([str(binaries[name]), "sync", "--json"], checkout, run_env)
                synced = json.loads(output)
                assert synced["pushed"] == 0 and synced["integrity"] == 0
                report["results"][name]["first_sync"].append(ms)
                for key in ("export", "log_all"):
                    output, _ = execute([str(binaries[name]), *commands[key]], checkout, run_env)
                    assert digest(output) == baseline[key], (name, key, "fresh copy differs")
        for result in report["results"].values():
            result["warm"] = {key: stats(values) for key, values in result["warm"].items()}
            result["first_sync"] = stats(result["first_sync"])
            result["cold_history"] = stats(result["cold_history"])
        after_tip = execute(["git", "--git-dir=" + str(mirror), "rev-parse", "refs/beads/journal"], root, env)[0]
        assert after_tip == tip
        report["output_sha256"] = baseline
        report["all_outputs_identical"] = True
        report["mirror_ref_unchanged"] = True
        report["host"]["load_end"] = os.getloadavg()
        return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", required=True, type=Path)
    parser.add_argument("--after", required=True, type=Path)
    parser.add_argument("--snapshot", required=True, type=Path)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--runs", type=int, default=30)
    parser.add_argument("--sync-runs", type=int, default=10)
    args = parser.parse_args()
    if args.runs < 1 or args.sync_runs < 1:
        parser.error("run counts must be positive")
    if not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", args.ref):
        parser.error("--ref must be a full pinned commit id")
    report = measure(args)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    for name, result in report["results"].items():
        print(name, "log_one_ms", round(result["warm"]["log_one"]["median_ms"], 2),
              "first_sync_ms", round(result["first_sync"]["median_ms"], 2))


if __name__ == "__main__":
    main()
