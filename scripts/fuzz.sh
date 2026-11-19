#!/usr/bin/env bash
# Fuzz the parsers of untrusted input with libFuzzer (H-02; docs/ci.md,
# "Fuzzing"): build rust/fuzz's targets once, seed each target from the test
# vectors, and run each for a time budget.
#
#   scripts/fuzz.sh <work dir> <seconds per target> [target...]
#
# With no target named, every target of rust/fuzz runs. Under <work dir>:
#   corpus/<target>/     the corpus libFuzzer grows; reuse it across runs
#   seeds/<target>/      the seeds, rewritten each run
#   artifacts/<target>/  what a run found: crash-*, timeout-*, oom-*, leak-*
#   logs/<target>.log    each run's output, ending with its stats
#
# Needs cargo-fuzz and a nightly toolchain. Env: DW_FUZZ_TOOLCHAIN (default
# nightly-2026-10-09), DW_FUZZ_TIMEOUT (seconds one input may take, default
# 30), DW_FUZZ_RSS_MB (memory limit, default 4096). Exits 1 when a target
# found something, after running the others.
set -euo pipefail

if [ $# -lt 2 ]; then
  sed -n '2,18p' "$0" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$1"
work="$(cd "$1" && pwd)"
seconds="$2"
shift 2
toolchain="${DW_FUZZ_TOOLCHAIN:-nightly-2026-10-09}"

cd "$ROOT/rust"
# Thin LTO over the platform tree makes the instrumented build much slower
# and finds nothing more.
export CARGO_PROFILE_RELEASE_LTO=false
fuzz=(cargo "+$toolchain" fuzz)
opts=(--fuzz-dir fuzz --target-dir target/fuzz --features libfuzzer --codegen-units 16)

if [ $# -gt 0 ]; then
  targets=("$@")
else
  mapfile -t targets < <("${fuzz[@]}" list --fuzz-dir fuzz)
fi

"${fuzz[@]}" build "${opts[@]}"
cargo run -p dw-fuzz --example write_seeds --locked -- "$work/seeds"

status=0
mkdir -p "$work/logs"
for target in "${targets[@]}"; do
  mkdir -p "$work/corpus/$target" "$work/artifacts/$target"
  log="$work/logs/$target.log"
  if "${fuzz[@]}" run "${opts[@]}" "$target" "$work/corpus/$target" "$work/seeds/$target" -- \
    -max_total_time="$seconds" \
    -timeout="${DW_FUZZ_TIMEOUT:-30}" \
    -rss_limit_mb="${DW_FUZZ_RSS_MB:-4096}" \
    -print_final_stats=1 \
    -artifact_prefix="$work/artifacts/$target/" >"$log" 2>&1; then
    runs="$(grep -o 'stat::number_of_executed_units: *[0-9]*' "$log" | grep -o '[0-9]*$' || true)"
    echo "$target: clean, ${runs:-?} inputs in ${seconds}s, corpus $(find "$work/corpus/$target" -type f | wc -l)"
  else
    echo "$target: FOUND SOMETHING (artifacts/$target/)"
    tail -n 80 "$log"
    status=1
  fi
done
exit "$status"
