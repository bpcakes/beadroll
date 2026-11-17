"""The environment a driver runs bead and git in: the caller's, less whatever would send them somewhere else."""
import os

# Variables that would route bead to another store or tracker (BEAD_), point git at another repository or
# configuration (GIT_), or make bead take the caller's agent session for its own actor (CLAUDE, CODEX).
INHERITED = ("BEAD_", "GIT_", "CLAUDE", "CODEX")


def scrubbed(*more):
    """This process's environment without the variables `INHERITED` names, nor those beginning with any of
    `more` (the prefixes of another tool a driver also runs, such as br's and bv's)."""
    prefixes = INHERITED + more
    return {key: value for key, value in os.environ.items() if not key.startswith(prefixes)}
