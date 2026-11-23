#!/usr/bin/env sh
# Run functional tests one after another in the `functional` compose service and print one
# summary line per test. Exits non-zero if any test failed.
#
# Usage: run-functional.sh [test.py ...]   (default: dwd_mn_chainlock.py dwd_coinjoin_probe.py)
# Environment: DWD_FUNC_LOGDIR  directory for per-test logs (default: ./functional-logs)
set -u

here=$(cd "$(dirname "$0")/.." && pwd)
logdir=${DWD_FUNC_LOGDIR:-"$PWD/functional-logs"}
mkdir -p "$logdir"
[ "$#" -gt 0 ] || set -- dwd_mn_chainlock.py dwd_coinjoin_probe.py

failed=0
for test in "$@"; do
    start=$(date +%s)
    log="$logdir/${test%.py}-$start.log"
    docker compose -f "$here/docker-compose.yml" --profile functional run --rm functional \
        "$test" --loglevel=INFO >"$log" 2>&1
    rc=$?
    echo "$test exit=$rc seconds=$(($(date +%s) - start)) log=$log"
    [ "$rc" -eq 0 ] || failed=1
done
exit "$failed"
