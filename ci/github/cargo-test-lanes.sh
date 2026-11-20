#!/usr/bin/env bash
# GitHub Actions, from rust/ after `cargo test --workspace --no-run --locked`:
# `cargo test --workspace --locked --no-fail-fast` as two lanes that run at the same time.
#
#   lib    the libraries' unit tests, then the doc tests
#   other  the bins' unit tests and every integration test (`--test '*'`)
#
# Between them they select every target `cargo test --workspace` runs. Both select the whole
# workspace, so they resolve the same features as the build and compile nothing. Each test binary
# still runs alone in one process with libtest's threads, so a file that serializes its own tests
# (e0_05_bringup's SERIAL, dwcli's teardown::exclusive, dw-engine's runtime_tests) still does.
# Every output line carries its lane; `time` gives each lane's wall clock and CPU.
set -uo pipefail

run() {
  local name=$1
  shift
  { time cargo test --workspace --locked --no-fail-fast "$@"; } 2>&1 | sed -u "s/^/[$name] /"
  return "${PIPESTATUS[0]}"
}

lib_lane() {
  local status=0
  run lib --lib || status=1
  run doc --doc || status=1
  return "$status"
}

lib_lane &
lib=$!
run other --bins --test '*' &
other=$!

status=0
wait "$lib" || { echo "::error::the libraries' unit tests or the doc tests failed"; status=1; }
wait "$other" || { echo "::error::the bins' unit tests or the integration tests failed"; status=1; }
exit "$status"
