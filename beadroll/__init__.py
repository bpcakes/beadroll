"""bead: proof-of-concept issue tracker for many agents, worktrees and machines.

One tracker per repository, kept outside every working tree:

    $BEAD_HOME/<workspace>/events/<machine>/<segment>.jsonl   append-only, one writer per machine
    $BEAD_HOME/<workspace>/sync.git                           bare repo used only to move the journal

Every event names the events its writer had already seen. Current state is the
replay of all journals in causal order, so two machines merge by exchanging
files and there is nothing to conflict. `bead sync` moves the journal through a
dedicated git ref; claims are settled by a conditional update of that ref.
"""
from __future__ import annotations

from .base import *  # noqa: F401,F403  (one namespace for tests and scripts)
from .machine import *  # noqa: F401,F403  (one namespace for tests and scripts)
from .model import *  # noqa: F401,F403  (one namespace for tests and scripts)
from .queries import *  # noqa: F401,F403  (one namespace for tests and scripts)
from .sync import *  # noqa: F401,F403  (one namespace for tests and scripts)
from .commands import *  # noqa: F401,F403  (one namespace for tests and scripts)
from .cli import *  # noqa: F401,F403  (one namespace for tests and scripts)
from .base import __version__  # noqa: F401
from .cli import main, run  # noqa: F401
