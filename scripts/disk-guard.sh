#!/usr/bin/env bash
# Abort (exit 1) when free disk space is below the threshold (default 15 GB).
#   scripts/disk-guard.sh [min_gb] [path]
# DWD_MIN_FREE_GB overrides the default threshold.
set -euo pipefail

min_gb="${1:-${DWD_MIN_FREE_GB:-15}}"
path="${2:-}"
if [[ -z "$path" ]]; then
  if [[ "$(uname -s)" == "Darwin" ]]; then
    path=/System/Volumes/Data
  else
    path="${CARGO_TARGET_DIR:-$PWD}"
    [[ -e "$path" ]] || path=/
  fi
fi

# POSIX df: 1024-byte blocks, available is column 4.
avail_kb=$(df -Pk "$path" | awk 'NR==2 {print $4}')
avail_gb=$(( avail_kb / 1024 / 1024 ))

if (( avail_gb < min_gb )); then
  echo "disk-guard: only ${avail_gb} GB free on ${path} (< ${min_gb} GB); aborting build" >&2
  exit 1
fi
echo "disk-guard: ${avail_gb} GB free on ${path} (>= ${min_gb} GB)"
