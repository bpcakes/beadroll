#!/bin/sh
# Run the test modules with a scratch store; one line per module, non-zero exit on any failure.
#   ./check.sh            fast modules (about 25 minutes)
#   ./check.sh --stress   also test_stress (randomised models) and test_sweep (the protocol enumeration)
#   ./check.sh --real     also infra/roundtrip.py over the real tracker exports on this machine
set -u
cd "$(dirname "$0")"
export BEAD_HOME="${BEAD_HOME:-$(mktemp -d "${TMPDIR:-/tmp}/bp-check-home.XXXXXX")}"
modules="test_hygiene test_replay test_storage test_compat test_commands test_protocol test_hardening"
real=0
for arg in "$@"; do
    case "$arg" in
        --stress) modules="$modules test_stress test_sweep" ;;
        --real) real=1 ;;
        *) echo "unknown option $arg" >&2; exit 2 ;;
    esac
done
status=0
for module in $modules; do
    start=$(date +%s)
    out=$(python3 -m unittest "tests.$module" 2>&1)
    rc=$?
    line=$(printf '%s\n' "$out" | grep -E '^(Ran|OK|FAILED)' | tr '\n' ' ')
    printf '%-14s %s(%ss)\n' "$module" "$line" "$(( $(date +%s) - start ))"
    if [ "$rc" -ne 0 ]; then
        status=1
        # each failure's heading and the line that says why (the assertion, or the exception)
        printf '%s\n' "$out" | grep -E '^(FAIL|ERROR):|^[A-Za-z_.]*(Error|Exception)(:|$)' | sed 's/^/    /'
    fi
done
if [ "$real" -eq 1 ]; then
    RT_HOME="$BEAD_HOME/roundtrip" python3 infra/roundtrip.py | tail -1 || status=1
fi
exit $status
