"""Read and write JSON with bead's Python implementation, for `cargo xtask diff json`
(docs/RUST-PORT.md §4.2 item 3): one case per line on stdin, one answer per line on stdout.

A case is {"bytes": BYTES} (latin-1: each code point a byte), read as a journal line is read
(`loads_line`: strict, UTF-8 with surrogates passed, each lone surrogate reported and read as
U+FFFD), then written in each dialect bead uses; or {"plus_one": DIGITS}, an integer built in memory
(DIGITS plus one), written in each dialect. A decode error is compared by its class only: its text
never reaches anyone (a journal line reports "line is not valid JSON").

Run it with PYTHONINTMAXSTRDIGITS=4300: it refuses to run under any other integer limit.
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

import beadroll as bead  # noqa: E402

LIMIT = 4300
DIALECTS = {
    "compact": {"ensure_ascii": False, "separators": (",", ":")},
    "spaced": {"ensure_ascii": False},
    "ascii": {},
    "indented": {"indent": 1},
    "canonical": {"sort_keys": True, "ensure_ascii": False, "separators": (",", ":")},
    "sorted_ascii": {"sort_keys": True},
}


def written(value):
    out = {}
    for name, options in DIALECTS.items():
        try:
            out[name] = json.dumps(value, **options)
        except ValueError as error:
            out[name] = {"error": str(error)}
    return out


def answer(case):
    if "plus_one" in case:
        return {"value": written(int(case["plus_one"]) + 1)}
    problems = []
    try:
        value = bead.loads_line(case["bytes"].encode("latin-1"), "case", 0, problems)
    except UnicodeDecodeError:
        return {"error": "UnicodeDecodeError", "message": None}
    except ValueError as error:
        return {"error": type(error).__name__, "message": str(error)}
    return {"value": written(value), "lone_surrogates": bool(problems)}


def main():
    if sys.get_int_max_str_digits() != LIMIT:
        sys.exit(f"json_dump.py: run with PYTHONINTMAXSTRDIGITS={LIMIT}, not {sys.get_int_max_str_digits()}")
    sys.setrecursionlimit(10_000)
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    for line in sys.stdin:
        sys.stdout.write(json.dumps(answer(json.loads(line)), ensure_ascii=False, separators=(",", ":")) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
