"""Run or list one module of the Python test suite for `cargo xtask` (docs/RUST-PORT.md §4.2).

    python3 xtask/python/oracle.py --skip xtask/oracle-skip.txt test_commands
    python3 xtask/python/oracle.py --list test_replay test_commands ...

Run from the repository root, which holds the `tests` package. Running a module runs every test in
it except those the skip list names, against the bead that BEAD_UNDER_TEST selects: each outcome
goes to standard error as the test finishes, the failures in full at the end, and one summary line
to standard output:

    test_commands ran=105 passed=105 failed=0 errors=0 skipped=0 excluded=6 fixture_errors=0 seconds=301

While the oracle targets another binary, BEADROLL_ORACLE_REFUSALS names a file in which Python bead
records every start it refuses (beadroll/cli.py). A test during which that file grows reached Python
bead by a path other than BEAD_UNDER_TEST, so it fails here whatever its own verdict: it would
otherwise count a Python run as a pass of the binary under test.

Exit status: 0 when every test that ran passed, 1 when one did not, 2 when the module or the skip
list could not be used. Standard library only.
"""
import argparse
import os
import sys
import time
import traceback
import unittest

GROUPS = ("driver", "in-process", "patched", "no-bead")  # the skip list's groups, docs/RUST-PORT.md §4.2
REFUSALS = "BEADROLL_ORACLE_REFUSALS"


def read_skip_list(path):
    """{test id: group} from lines of `GROUP TEST-ID`; `#` starts a comment."""
    skip = {}
    with open(path, encoding="utf-8") as lines:
        for number, line in enumerate(lines, 1):
            words = line.split("#", 1)[0].split()
            if not words:
                continue
            if len(words) != 2 or words[0] not in GROUPS:
                raise ValueError(f"{path}:{number}: expected GROUP TEST-ID, with GROUP one of {', '.join(GROUPS)}")
            if words[1] in skip:
                raise ValueError(f"{path}:{number}: {words[1]} is listed twice")
            skip[words[1]] = words[0]
    return skip


def short_id(test):
    """`test_commands.BpTest.test_x`: the id without the package, as the skip list and inventory name it."""
    return test.id().removeprefix("tests.")


def flatten(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from flatten(item)
        else:
            yield item


def load(module):
    """Every test of `tests.<module>`, in the order unittest runs them; None if it cannot be imported."""
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromName(f"tests.{module}")
    if loader.errors:
        for error in loader.errors:
            print(error, file=sys.stderr)
        return None
    return list(flatten(suite))


class Refusals:
    """What Python bead appended to the refusal file since the last look."""

    def __init__(self, path):
        self.path, self.seen = path, 0

    def new(self):
        if not self.path:
            return []
        try:
            with open(self.path, "rb") as log:
                log.seek(self.seen)
                data = log.read()
        except FileNotFoundError:
            return []
        self.seen += len(data)
        return data.decode("utf-8", "replace").splitlines()


class Result(unittest.TestResult):
    """One outcome per test, the worst it reached: error, failed, skipped or passed."""

    RANK = {"passed": 0, "skipped": 1, "failed": 2, "error": 3}

    def __init__(self, refusals):
        super().__init__()
        self.refusals, self.outcomes, self.reports, self.outside = refusals, {}, [], []

    def note(self, test, outcome, text=None):
        key = test.id()
        if self.RANK[outcome] >= self.RANK[self.outcomes.get(key, "passed")]:
            self.outcomes[key] = outcome
        if text:
            self.reports.append((outcome, key, text))

    @staticmethod
    def text(err):
        return "".join(traceback.format_exception(*err))

    def startTest(self, test):
        super().startTest(test)
        self.outside += self.refusals.new()  # a start no test was running for: a class or module fixture
        self.started = time.monotonic()
        self.outcomes[test.id()] = "passed"

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self.note(test, "failed", self.text(err))

    def addError(self, test, err):  # also class and module fixtures, whose `test` is a placeholder
        super().addError(test, err)
        self.note(test, "error", self.text(err))

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        self.note(test, "skipped")

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            failed = issubclass(err[0], test.failureException)
            self.note(test, "failed" if failed else "error", f"{subtest}\n{self.text(err)}")

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self.note(test, "failed", "unexpected success")

    def stopTest(self, test):
        refused = self.refusals.new()
        if refused:
            self.note(test, "failed", f"it started Python bead instead of BEAD_UNDER_TEST {len(refused)} time(s); "
                                      f"the first with arguments {refused[0]}\n")
        super().stopTest(test)
        seconds = time.monotonic() - self.started
        print(f"{self.outcomes[test.id()]:8} {seconds:7.1f}s  {short_id(test)}", file=sys.stderr, flush=True)


def run(module, skip):
    tests = load(module)
    if tests is None:
        return 2
    known = {short_id(test) for test in tests}
    unknown = sorted(name for name in skip if name.split(".", 1)[0] == module and name not in known)
    if unknown:
        print(f"the skip list names tests that {module} does not have: {', '.join(unknown)}", file=sys.stderr)
        return 2
    chosen = [test for test in tests if short_id(test) not in skip]
    result = Result(Refusals(os.environ.get(REFUSALS)))
    start = time.monotonic()
    unittest.TestSuite(chosen).run(result)
    outside = result.outside + result.refusals.new()
    for outcome, name, text in result.reports:
        print(f"\n{'=' * 70}\n{outcome.upper()}: {name.removeprefix('tests.')}\n{'-' * 70}\n{text}", file=sys.stderr)
    if outside:
        print(f"\nPython bead was started outside any test {len(outside)} time(s), "
              f"the first with arguments {outside[0]}", file=sys.stderr)
    # A test that never started had a class or module fixture fail under it: an error. The fixture
    # errors themselves, and Python bead started by a fixture, count apart from the tests.
    counts = {outcome: 0 for outcome in Result.RANK}
    for test in chosen:
        counts[result.outcomes.get(test.id(), "error")] += 1
    fixture_errors = sum(1 for name in result.outcomes if not name.startswith("tests.")) + bool(outside)
    print(f"{module} ran={len(chosen)} passed={counts['passed']} failed={counts['failed']} "
          f"errors={counts['error']} skipped={counts['skipped']} excluded={len(tests) - len(chosen)} "
          f"fixture_errors={fixture_errors} seconds={round(time.monotonic() - start)}", flush=True)
    return 0 if counts["failed"] == counts["error"] == fixture_errors == 0 else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--skip", help="the skip list (required to run a module)")
    parser.add_argument("--list", action="store_true", help="print the ids of the modules' tests instead")
    parser.add_argument("modules", nargs="+")
    args = parser.parse_args(argv)
    sys.path.insert(0, os.getcwd())  # a script's own directory comes first on the path, not the repository's
    if args.list:
        for module in args.modules:
            tests = load(module)
            if tests is None:
                return 2
            print("\n".join(short_id(test) for test in tests))
        return 0
    if args.skip is None or len(args.modules) != 1:
        parser.error("running needs --skip and exactly one module")
    if not os.environ.get("BEAD_UNDER_TEST"):
        print("BEAD_UNDER_TEST is not set: the suite would test the checkout's own launcher", file=sys.stderr)
        return 2
    try:
        skip = read_skip_list(args.skip)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 2
    return run(args.modules[0], skip)


if __name__ == "__main__":
    sys.exit(main())
