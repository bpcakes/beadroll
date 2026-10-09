"""Test package for beadroll. Every test builds its own repositories, remotes and machine homes in a
temporary directory. The default store (~/.local/state/beadroll) is never touched: a scratch
BEAD_HOME is set here if the environment has none.

    python3 -m unittest discover -s tests -t .          # everything but the slow models
    python3 -m unittest tests.test_stress               # randomised multi-machine models
"""
import os
import tempfile

os.environ.setdefault("BEAD_HOME", tempfile.mkdtemp(prefix="bead-tests-home-"))
