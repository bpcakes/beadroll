"""Every process an infra driver starts has a deadline, so that a hung git, ssh or bead stops the run
instead of holding it: each subprocess call passes `timeout`, or is a wrapper's, forwarding a `timeout`
parameter that defaults to one (drive.py's `ran`)."""
import ast
import unittest
from pathlib import Path

INFRA = Path(__file__).resolve().parent
STARTS = {"run", "call", "check_call", "check_output", "Popen"}


def drivers():
    """The infra scripts that are not tests: the drivers and their shared modules."""
    return sorted(path for path in INFRA.glob("*.py") if not path.name.startswith("test_"))


def undated(source):
    """The lines of `source` that start a process with no deadline: `os.system`, or a subprocess call that
    passes no `timeout`, passes `None`, or forwards a parameter of its function with no default or `None`."""
    found = []

    def visit(node, params):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            args = node.args
            positional = args.posonlyargs + args.args
            params = {arg.arg: None for arg in positional + args.kwonlyargs}
            params.update(zip([a.arg for a in positional[len(positional) - len(args.defaults):]], args.defaults))
            params.update({a.arg: d for a, d in zip(args.kwonlyargs, args.kw_defaults) if d is not None})
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
            called = (ast.unparse(node.func.value), node.func.attr)
            if called == ("os", "system"):
                found.append(node.lineno)
            elif called[0] == "subprocess" and called[1] in STARTS:
                timeout = next((k.value for k in node.keywords if k.arg == "timeout"), None)
                if isinstance(timeout, ast.Name) and timeout.id in params:
                    timeout = params[timeout.id]  # forwarded: the parameter's default is the deadline
                if timeout is None or (isinstance(timeout, ast.Constant) and timeout.value is None):
                    found.append(node.lineno)
        for child in ast.iter_child_nodes(node):
            visit(child, params)

    visit(ast.parse(source), {})
    return found


class Deadlines(unittest.TestCase):
    def test_every_process_a_driver_starts_has_a_deadline(self):
        self.assertEqual({path.name: undated(path.read_text()) for path in drivers() if undated(path.read_text())}, {})

    def test_a_start_without_a_deadline_is_found(self):
        cases = {
            "import subprocess\nsubprocess.run(['git'])\n": [2],
            "import subprocess\ndef ran(a, timeout=None):\n    return subprocess.run(a, timeout=timeout)\n": [3],
            "import subprocess\ndef ran(a, timeout):\n    return subprocess.run(a, timeout=timeout)\n": [3],
            "import os\nos.system('git status')\n": [2],
            "import subprocess\nsubprocess.Popen(['ssh'])\n": [2],
        }
        for source, lines in cases.items():
            with self.subTest(source=source):
                self.assertEqual(undated(source), lines)

    def test_a_deadline_given_or_defaulted_is_accepted(self):
        for source in [
            "import subprocess\nsubprocess.run(['git'], timeout=60)\n",
            "import subprocess\nTIMEOUT = 5\nsubprocess.run(['git'], timeout=TIMEOUT)\n",
            "import subprocess\ndef ran(a, timeout=120):\n    return subprocess.run(a, timeout=timeout)\n",
        ]:
            with self.subTest(source=source):
                self.assertEqual(undated(source), [])


if __name__ == "__main__":
    unittest.main()
