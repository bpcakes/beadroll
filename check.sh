#!/bin/sh
# The baseline: `cargo xtask check` with the options given (docs/RUST-PORT.md §4.9). It runs every
# step with a scratch BEAD_HOME of its own, one line per step, and exits non-zero when any fails.
#   ./check.sh            the workspace's tests through nextest (the sweep's slice among them), its
#                         doctests, and the tests of the infra/ drivers
#   ./check.sh --stress   also the stress models (several minutes each)
#   ./check.sh --real     also the real-export compatibility test with br and bv, and
#                         infra/roundtrip.py over the Beads exports on this machine
# BEAD_UNDER_TEST, set to an absolute path, names another bead for the tests and the round trip.
set -eu
cd "$(dirname "$0")"
exec cargo --locked xtask check "$@"
