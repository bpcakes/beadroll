"""Which bead an infra driver runs on this machine, and the record of it.

The bead is the one BEAD_UNDER_TEST names, which must be an absolute path (a build of this checkout, such as
target/release/bead, or a staged one); else the installed ~/.local/bin/bead. Until the cutover this checkout's
Python launcher, ./bead, came between the two. A driver's helpers are found in this source tree, never beside
the bead. Before a driver writes test data it records the bead it selected: its path, the first line
`--version` prints, whether it is a script (Python's launcher or installed entry point) or a native build, and
a digest of it. With BEAD_EXPECT_VERSION set (0.2.0 at the cutover), a bead that reports another version or is
a script stops the driver there."""
import hashlib
import json
import os
import subprocess

INSTALLED = "~/.local/bin/bead"


def local_bead(environ=None):
    """The bead to run on this machine, as the module's docstring says."""
    environ = os.environ if environ is None else environ
    chosen = environ.get("BEAD_UNDER_TEST", "")
    if chosen:
        if not os.path.isabs(chosen):
            raise SystemExit(f"BEAD_UNDER_TEST must be an absolute path, not {chosen!r}")
        return chosen
    return os.path.expanduser(INSTALLED)


def describe(bead):
    """What `bead` is: its path, the first line `--version` prints, its kind (native, script or missing) and
    the first 16 hex digits of its SHA-256."""
    try:
        with open(bead, "rb") as file:
            data = file.read()
    except OSError:
        return {"path": bead, "version": "", "kind": "missing", "sha256": ""}
    try:
        proc = subprocess.run([bead, "--version"], capture_output=True, text=True, timeout=60)
        lines = (proc.stdout.strip() or proc.stderr.strip()).splitlines()
        version = lines[0] if lines else ""
    except (OSError, subprocess.TimeoutExpired) as error:
        version = f"(does not run: {error})"
    kind = "script" if data.startswith(b"#!") else "native"
    return {"path": bead, "version": version, "kind": kind, "sha256": hashlib.sha256(data).hexdigest()[:16]}


def verdict(row, expect):
    """Why the bead `row` describes is not the one expected, or None; an empty `expect` expects nothing."""
    if not expect:
        return None
    if row["kind"] == "missing":
        return f"{row['path']} does not exist"
    if row["version"] != f"bead {expect}":
        return f"{row['path']} reports {row['version']!r}, not 'bead {expect}'"
    if row["kind"] != "native":
        return f"{row['path']} is a {row['kind']}, not a native build"
    return None


def preflight(bead, environ=None):
    """Records `bead` as one `preflight:` JSON line and stops the driver unless it is what BEAD_EXPECT_VERSION
    asks for. Returns the record."""
    environ = os.environ if environ is None else environ
    row = describe(bead)
    print("preflight:", json.dumps(row, sort_keys=True), flush=True)
    wrong = verdict(row, environ.get("BEAD_EXPECT_VERSION", ""))
    if wrong:
        raise SystemExit(f"preflight: {wrong}")
    return row
