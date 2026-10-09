"""Record every journal the Python tests build, as cases for `cargo xtask diff replay`
(docs/RUST-PORT.md §4.2 item 3), one JSON case per line, deduplicated:

- in-process tests: the operations that built each journal `replay` was called on (adds, readers'
  problems, settles), in order;
- CLI tests: every store a sandboxed machine left, read as `load_journal` reads it (each machine's
  segments, then its attempt files).

    python3 xtask/python/capture_fixtures.py OUT.jsonl [--per-test N] [--max-bytes B] [MODULE...]

Fixtures stream to OUT as the tests build them. --per-test keeps the first N of each test and
--max-bytes leaves out any larger than B: `--per-test 2 --max-bytes 64000` gives the sample checked
in for CI as xtask/fixtures/replay/tests.jsonl. Scratch paths read /tmp/. Run with
PYTHONINTMAXSTRDIGITS=4300, as the replay driver is. A journal the drivers cannot carry (a value JSON
cannot hold, a lone surrogate in memory) is counted and left out.
"""
import collections
import hashlib
import json
import re
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

import beadroll as bead  # noqa: E402
from tests import support  # noqa: E402

LIMIT = 4300
FAST = ["test_replay", "test_storage", "test_compat", "test_commands", "test_protocol", "test_hardening"]

# Where the tests' scratch directories live on macOS, which names the account: fixtures say /tmp/.
TEMPORARY = re.compile(r"(?:/private)?/var/folders/[^/\"]+/[^/\"]+/T/")

current = ["?"]
seen, skipped, per_test = set(), collections.Counter(), collections.Counter()
settings = {"out": None, "per_test": None, "max_bytes": None, "kept": 0}


def keep(kind, ops):
    try:
        text = json.dumps(ops, ensure_ascii=False, separators=(",", ":"), allow_nan=False)
        text.encode("utf-8")  # a lone surrogate in memory cannot reach a file, nor this case
    except (TypeError, ValueError, UnicodeEncodeError) as error:
        skipped[f"{kind}: {type(error).__name__}"] += 1
        return
    digest = hashlib.sha256(text.encode()).hexdigest()
    if digest in seen:
        return
    seen.add(digest)
    if settings["max_bytes"] is not None and len(text) > settings["max_bytes"]:
        skipped["larger than --max-bytes"] += 1
        return
    per_test[current[0]] += 1
    if settings["per_test"] is not None and per_test[current[0]] > settings["per_test"]:
        skipped["beyond --per-test"] += 1
        return
    case = json.dumps({"name": f"{kind} {current[0]}", "ops": ops}, ensure_ascii=False, separators=(",", ":"))
    settings["out"].write(TEMPORARY.sub("/tmp/", case) + "\n")
    settings["kept"] += 1


class LoggedList(list):
    """journal.invalid, recording what a reader appends to it."""

    def __init__(self, journal, items=()):
        super().__init__(items)
        self.journal = journal

    def append(self, entry):
        if not self.journal._adding:
            row = {"event": entry.get("event"), "origin": entry.get("origin"), "problem": entry.get("problem")}
            if isinstance(row["origin"], str) and isinstance(row["problem"], str):
                self.journal._log.append(["problem", row])
            else:
                self.journal._unloggable = True
        super().append(entry)


Journal = bead.model.Journal
original = {"init": Journal.__init__, "add": Journal.add, "settle": Journal.settle}


def init(self):
    original["init"](self)
    self._log, self._adding, self._unloggable = [], False, False
    self.invalid = LoggedList(self)


def add(self, event, replica, origin="", sequenced=True):
    self._log.append(["add", event, replica, origin, sequenced])
    self._adding = True
    try:
        original["add"](self, event, replica, origin, sequenced)
    finally:
        self._adding = False


def settle(self):
    if not self._settled:
        self._log.append(["settle"])
    original["settle"](self)
    if not isinstance(self.invalid, LoggedList):
        self.invalid = LoggedList(self, self.invalid)


Journal.__init__, Journal.add, Journal.settle = init, add, settle
replay = bead.model.replay


def recording_replay(journal):
    if getattr(journal, "_unloggable", True):
        skipped["journal: built outside the log"] += 1
    else:
        keep("journal", [list(op) for op in journal._log])
    return replay(journal)


for name, module in list(sys.modules.items()):
    if (name == "beadroll" or name.startswith("beadroll.")) and getattr(module, "replay", None) is replay:
        module.replay = recording_replay


def snapshot(root):
    """Every store under `root`, as load_journal reads it."""
    for events in sorted(Path(root).rglob("events")):
        workspace = events.parent
        if not events.is_dir() or not ((workspace / "lock").exists() or (workspace / "meta.json").exists()):
            continue
        ops = []
        try:
            machines = [d for d in sorted(events.iterdir()) if d.is_dir()]
            for machine in machines:
                for path in sorted(machine.iterdir()):
                    if path.suffix == ".jsonl" and path.is_file():
                        ops.append(["segment", machine.name, str(path.relative_to(events.parent)),
                                    path.read_bytes().decode("latin-1")])
            for machine in machines:
                claims = machine / "claims"
                for path in sorted(claims.iterdir()) if claims.is_dir() else ():
                    if path.suffix == ".json" and path.is_file():
                        ops.append(["attempt", machine.name, str(path.relative_to(events.parent)),
                                    path.read_bytes().decode("latin-1")])
        except OSError:
            skipped["store: unreadable"] += 1
            continue
        if ops:
            keep("store", ops)


cleanup = support.Sandbox.cleanup


def recording_cleanup(self):
    snapshot(self.root)
    cleanup(self)


support.Sandbox.cleanup = recording_cleanup


class Result(unittest.TextTestResult):
    def startTest(self, test):
        current[0] = test.id()
        super().startTest(test)


def main():
    if sys.get_int_max_str_digits() != LIMIT:
        sys.exit(f"capture_fixtures.py: run with PYTHONINTMAXSTRDIGITS={LIMIT}")
    args, modules = sys.argv[2:], []
    while args:
        arg = args.pop(0)
        if arg in ("--per-test", "--max-bytes"):
            settings[arg[2:].replace("-", "_")] = int(args.pop(0))
        else:
            modules.append(arg)
    suite = unittest.defaultTestLoader.loadTestsFromNames(f"tests.{m}" for m in modules or FAST)
    runner = unittest.TextTestRunner(resultclass=Result, verbosity=0, stream=sys.stderr)
    with open(sys.argv[1], "w", encoding="utf-8") as settings["out"]:
        result = runner.run(suite)
    print(f"{settings['kept']} fixtures from {result.testsRun} tests; left out: {dict(skipped)}", file=sys.stderr)
    sys.exit(0 if result.wasSuccessful() else 1)


if __name__ == "__main__":
    main()
